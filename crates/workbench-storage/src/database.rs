use crate::{paths::project_key, schema};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeSet, fs, io::Write, path::PathBuf, time::Duration};
use workbench_core::*;

pub(crate) struct Database {
    pub connection: Connection,
    pub root: PathBuf,
    pub service_started: Option<CommandId>,
}

pub(crate) fn storage_error(error: impl std::fmt::Display) -> BusinessError {
    BusinessError::new(
        ErrorKind::Storage,
        format!("local storage operation failed: {error}"),
        RetryAdvice::AfterCorrection,
    )
}
pub(crate) fn encode<T: Serialize>(record: &T) -> Result<String, BusinessError> {
    serde_json::to_string(record).map_err(|_| storage_error("cannot encode business record"))
}
pub(crate) fn decode<T: DeserializeOwned>(text: &str) -> Result<T, BusinessError> {
    serde_json::from_str(text)
        .map_err(|_| storage_error("invalid or unsupported persisted business record"))
}
fn number(value: u64) -> Result<i64, BusinessError> {
    value.try_into().map_err(|_| {
        BusinessError::new(
            ErrorKind::Exhausted,
            "SQLite sequence range exhausted",
            RetryAdvice::Never,
        )
    })
}

pub(crate) fn restored(connection: &Connection) -> Result<RestoreData, BusinessError> {
    let (sequence, text): (i64, String) = connection
        .query_row(
            "SELECT sequence,snapshot_json FROM workbench_state WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(storage_error)?;
    let checkpoint: Versioned<WorkbenchState> = decode(&text)?;
    if number(checkpoint.record.sequence())? != sequence {
        return Err(storage_error(
            "checkpoint watermark does not match its state",
        ));
    }
    let mut statement = connection
        .prepare("SELECT record_json FROM journal WHERE sequence>?1 ORDER BY sequence")
        .map_err(storage_error)?;
    let rows = statement
        .query_map([sequence], |r| r.get::<_, Option<String>>(0))
        .map_err(storage_error)?;
    let mut result = RestoreData::default();
    result.checkpoint = Some(checkpoint);
    for row in rows {
        let text = row
            .map_err(storage_error)?
            .ok_or_else(|| storage_error("required replay record was redacted"))?;
        result.tail.push(decode(&text)?);
    }
    Ok(result)
}
pub(crate) fn effective_state(connection: &Connection) -> Result<WorkbenchState, BusinessError> {
    let data = restored(connection)?;
    replay(
        data.checkpoint.map(|v| v.record).unwrap_or_default(),
        &data.tail,
    )
}

/// Explicit SQL query projections are always derived from the sole core business state.
pub(crate) fn rebuild(
    connection: &Connection,
    state: &WorkbenchState,
) -> Result<(), BusinessError> {
    let old_refs: BTreeSet<String> = {
        let mut query = connection
            .prepare("SELECT DISTINCT blob_id FROM blob_refs")
            .map_err(storage_error)?;
        query
            .query_map([], |r| r.get(0))
            .map_err(storage_error)?
            .collect::<Result<_, _>>()
            .map_err(storage_error)?
    };
    let snapshot = state.snapshot();
    connection
        .execute_batch(
            "DELETE FROM blob_refs; DELETE FROM session_index; DELETE FROM project_index;",
        )
        .map_err(storage_error)?;
    for project in &snapshot.projects {
        connection
            .execute(
                "INSERT INTO project_index VALUES(?1,?2,?3,?4)",
                params![
                    project.id.to_string(),
                    project_key(&project.directory)?,
                    project.name,
                    encode(&Versioned::new(project.clone()))?
                ],
            )
            .map_err(storage_error)?;
    }
    let mut references = BTreeSet::new();
    for summary in &snapshot.sessions {
        let session = state
            .session_snapshot(summary.id)
            .ok_or_else(|| storage_error("session projection missing"))?;
        if !snapshot
            .agents
            .iter()
            .any(|a| a.id == session.record.agent_id())
        {
            return Err(storage_error("session references an unknown Agent"));
        }
        connection
            .execute(
                "INSERT INTO session_index VALUES(?1,?2,?3,?4)",
                params![
                    summary.id.to_string(),
                    summary.project_id.to_string(),
                    encode(&Versioned::new(session.record.clone()))?,
                    encode(&Versioned::new(session.clone()))?
                ],
            )
            .map_err(storage_error)?;
        for attachment in &session.attachments {
            if attachment.session_id != summary.id {
                return Err(storage_error("attachment owner does not match its session"));
            }
            if let AttachmentStorage::Managed { blob_id } = attachment.storage {
                let available: Option<i64> = connection
                    .query_row(
                        "SELECT pending_gc FROM blobs WHERE id=?1",
                        [blob_id.to_string()],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(storage_error)?;
                if available != Some(0) {
                    return Err(storage_error(
                        "managed attachment has no available owned blob",
                    ));
                }
                connection
                    .execute(
                        "INSERT INTO blob_refs VALUES(?1,?2,?3)",
                        params![
                            blob_id.to_string(),
                            summary.id.to_string(),
                            attachment.id.to_string()
                        ],
                    )
                    .map_err(storage_error)?;
                references.insert(blob_id.to_string());
            }
        }
    }
    for removed in old_refs.difference(&references) {
        connection
            .execute("UPDATE blobs SET pending_gc=1 WHERE id=?1", [removed])
            .map_err(storage_error)?;
    }
    // Checkpoint retains dedup IDs/tombstones. Deleted session/project payloads need not remain.
    connection.execute("UPDATE journal SET record_json=NULL WHERE session_id IS NOT NULL AND session_id NOT IN (SELECT id FROM session_index)", []).map_err(storage_error)?;
    connection.execute("UPDATE journal SET record_json=NULL WHERE project_id IS NOT NULL AND project_id NOT IN (SELECT id FROM project_index)", []).map_err(storage_error)?;
    Ok(())
}

pub(crate) type RecordScope = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
pub(crate) fn record_scope(record: &JournalRecord, state: &WorkbenchState) -> RecordScope {
    let (mut session, mut project, command_id, event_id) = match &record.entry {
        JournalEntry::Command(c) => {
            let session = match &c.command {
                Command::CreateSession { session } => Some(session.id()),
                _ => c.scope.session_id,
            };
            let project = match &c.command {
                Command::PutProject { project } => Some(project.id),
                Command::DeleteProject { project_id } => Some(*project_id),
                Command::CreateSession { session } => Some(session.project_id()),
                _ => None,
            };
            (session, project, Some(c.id.to_string()), None)
        }
        JournalEntry::Event(e) => (e.scope.session_id, None, None, Some(e.id.to_string())),
    };
    if let Some(id) = session {
        if let Some(s) = state.session_snapshot(id) {
            project = Some(s.record.project_id());
        }
    }
    (
        session.take().map(|id| id.to_string()),
        project.take().map(|id| id.to_string()),
        command_id,
        event_id,
    )
}

impl Database {
    pub fn open(root: PathBuf) -> Result<Self, BusinessError> {
        if !root.is_absolute() {
            return Err(storage_error("application data path must be absolute"));
        }
        fs::create_dir_all(&root).map_err(storage_error)?;
        let root = root.canonicalize().map_err(storage_error)?;
        fs::create_dir_all(root.join("blobs")).map_err(storage_error)?;
        let mut connection =
            Connection::open(root.join("workbench.sqlite3")).map_err(storage_error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(storage_error)?;
        schema::migrate(&mut connection)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(storage_error)?;
        let mut database = Self {
            connection,
            root,
            service_started: None,
        };
        database.cleanup_pending()?;
        Ok(database)
    }

    /// Re-derive the submitted checkpoint, rather than trusting caller-supplied JSON state.
    pub fn commit(&mut self, batch: StorageCommit) -> Result<(), BusinessError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let state = effective_state(&tx)?;
        if state.sequence() != batch.expected_sequence
            || batch.expected_sequence.checked_add(1) != Some(batch.record.sequence)
        {
            return Err(BusinessError::new(
                ErrorKind::SequenceConflict,
                "storage sequence changed; reload before committing",
                RetryAdvice::Never,
            ));
        }
        let transition = reduce(&state, &batch.record, ApplyMode::Live)?;
        if transition.state() != &batch.checkpoint.record {
            return Err(storage_error(
                "checkpoint does not match the shared reducer result",
            ));
        }
        insert_record(&tx, &batch.record, &state)?;
        save_state(&tx, &batch.checkpoint.record)?;
        tx.commit().map_err(storage_error)
    }

    /// Multiple text/semantic facts can be committed atomically; no effects escape on failure.
    pub fn apply(
        &mut self,
        entries: Vec<JournalEntry>,
    ) -> Result<crate::service::CommittedUpdate, BusinessError> {
        if entries.is_empty() {
            return Err(BusinessError::new(
                ErrorKind::InvalidInput,
                "empty persistence batch",
                RetryAdvice::AfterCorrection,
            ));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let mut state = effective_state(&tx)?;
        let mut decisions = Vec::new();
        let mut effects = Vec::new();
        let mut changed = BTreeSet::new();
        for entry in entries {
            match &entry {
                JournalEntry::Command(c) => match &c.command {
                    Command::Restart => {
                        changed.extend(state.snapshot().sessions.iter().map(|s| s.id))
                    }
                    Command::CreateSession { session } => {
                        changed.insert(session.id());
                    }
                    _ => {
                        if let Some(id) = c.scope.session_id {
                            changed.insert(id);
                        }
                    }
                },
                JournalEntry::Event(e) => {
                    if let Some(id) = e.scope.session_id {
                        changed.insert(id);
                    }
                }
            }
            let sequence = state
                .sequence()
                .checked_add(1)
                .ok_or_else(|| storage_error("sequence exhausted"))?;
            let record = JournalRecord::new(sequence, entry);
            let transition = reduce(&state, &record, ApplyMode::Live)?;
            insert_record(&tx, &record, &state)?;
            let (next, emitted, disposition) = transition.into_parts();
            decisions.push(crate::service::CommittedDecision {
                sequence,
                disposition,
            });
            for (ordinal, effect) in emitted.into_iter().enumerate() {
                effects.push(crate::service::CommittedEffect {
                    sequence,
                    ordinal,
                    effect,
                });
            }
            state = next;
        }
        save_state(&tx, &state)?;
        tx.commit().map_err(storage_error)?;
        let maintenance_errors = self.cleanup_pending().err().into_iter().collect();
        let sessions = changed
            .into_iter()
            .filter_map(|id| state.session_snapshot(id))
            .collect();
        Ok(crate::service::CommittedUpdate {
            snapshot: state.snapshot(),
            sessions,
            decisions,
            effects,
            maintenance_errors,
        })
    }

    pub fn preferences(&self) -> Result<Option<Versioned<WorkspacePreferences>>, BusinessError> {
        let text: Option<String> = self
            .connection
            .query_row("SELECT record_json FROM preferences WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(storage_error)?;
        text.map(|text| decode(&text)).transpose()
    }
    pub fn save_preferences(
        &mut self,
        record: Versioned<WorkspacePreferences>,
    ) -> Result<(), BusinessError> {
        self.save_preferences_checked(None, record).map(|_| ())
    }
    pub fn preference_revision(&self) -> Result<u64, BusinessError> {
        self.connection
            .query_row(
                "SELECT revision FROM preference_revision WHERE id=1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(storage_error)
            .and_then(|value| u64::try_from(value).map_err(storage_error))
    }
    pub fn save_preferences_checked(
        &mut self,
        expected: Option<u64>,
        record: Versioned<WorkspacePreferences>,
    ) -> Result<u64, BusinessError> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let revision = tx
            .query_row(
                "SELECT revision FROM preference_revision WHERE id=1",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(storage_error)?;
        let revision = u64::try_from(revision).map_err(storage_error)?;
        if expected.is_some_and(|value| value != revision) {
            return Err(BusinessError::new(
                ErrorKind::Conflict,
                "workspace preferences changed; use the confirmed revision",
                RetryAdvice::AfterCorrection,
            ));
        }
        let next = revision
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| storage_error("preference revision exhausted"))?;
        tx.execute("INSERT INTO preferences VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET record_json=excluded.record_json", [encode(&record)?]).map_err(storage_error)?;
        tx.execute(
            "UPDATE preference_revision SET revision=?1 WHERE id=1",
            [number(next)?],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(next)
    }
    pub fn history(&self, query: HistoryQuery) -> Result<HistoryPage, BusinessError> {
        if query.limit == 0 || query.limit > 500 {
            return Err(BusinessError::new(
                ErrorKind::InvalidInput,
                "history page limit must be 1..=500",
                RetryAdvice::AfterCorrection,
            ));
        }
        if self.session(query.session_id)?.is_none() {
            return Err(BusinessError::new(
                ErrorKind::NotFound,
                "session is not registered",
                RetryAdvice::Never,
            ));
        }
        let before = query
            .before_sequence
            .map(number)
            .transpose()?
            .unwrap_or(i64::MAX);
        let mut statement = self.connection.prepare("SELECT record_json FROM journal WHERE session_id=?1 AND sequence<?2 AND record_json IS NOT NULL ORDER BY sequence DESC LIMIT ?3").map_err(storage_error)?;
        let mut records = Vec::new();
        for row in statement
            .query_map(
                params![
                    query.session_id.to_string(),
                    before,
                    i64::from(query.limit) + 1
                ],
                |r| r.get::<_, String>(0),
            )
            .map_err(storage_error)?
        {
            records.push(decode::<JournalRecord>(&row.map_err(storage_error)?)?);
        }
        let more = records.len() > query.limit as usize;
        records.truncate(query.limit as usize);
        records.reverse();
        let mut page = HistoryPage::default();
        page.next_before_sequence = more.then(|| records[0].sequence);
        page.records = records;
        Ok(page)
    }
    pub fn projects(&self) -> Result<Vec<Project>, BusinessError> {
        let mut query = self
            .connection
            .prepare("SELECT record_json FROM project_index ORDER BY name,id")
            .map_err(storage_error)?;
        query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage_error)?
            .map(|row| decode::<Versioned<Project>>(&row.map_err(storage_error)?).map(|v| v.record))
            .collect()
    }
    pub fn project(&self, id: ProjectId) -> Result<Option<Project>, BusinessError> {
        let text: Option<String> = self
            .connection
            .query_row(
                "SELECT record_json FROM project_index WHERE id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        text.map(|v| decode::<Versioned<Project>>(&v).map(|v| v.record))
            .transpose()
    }
    pub fn session(&self, id: SessionId) -> Result<Option<SessionSnapshot>, BusinessError> {
        let text: Option<String> = self
            .connection
            .query_row(
                "SELECT snapshot_json FROM session_index WHERE id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        text.map(|v| decode::<Versioned<SessionSnapshot>>(&v).map(|v| v.record))
            .transpose()
    }
    pub fn sessions(&self, project: ProjectId) -> Result<Vec<SessionRecord>, BusinessError> {
        let mut query = self
            .connection
            .prepare("SELECT record_json FROM session_index WHERE project_id=?1 ORDER BY id")
            .map_err(storage_error)?;
        query
            .query_map([project.to_string()], |r| r.get::<_, String>(0))
            .map_err(storage_error)?
            .map(|row| {
                decode::<Versioned<SessionRecord>>(&row.map_err(storage_error)?).map(|v| v.record)
            })
            .collect()
    }

    fn blob_path(&self, id: AttachmentBlobId, extension: &str) -> Result<PathBuf, BusinessError> {
        let directory = self.root.join("blobs");
        if fs::symlink_metadata(&directory)
            .map_err(storage_error)?
            .file_type()
            .is_symlink()
            || directory.canonicalize().map_err(storage_error)? != directory
        {
            return Err(storage_error("attachment directory ownership changed"));
        }
        let path = directory.join(format!("{id}.{extension}"));
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(storage_error(
                    "attachment target is not an owned regular file",
                ));
            }
        }
        Ok(path)
    }
    pub fn write_blob(
        &mut self,
        id: AttachmentBlobId,
        bytes: Vec<u8>,
    ) -> Result<(), BusinessError> {
        let path = self.blob_path(id, "blob")?;
        let pending = self.blob_path(id, "pending")?;
        let length =
            i64::try_from(bytes.len()).map_err(|_| storage_error("attachment too large"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let stored: Option<i64> = tx
            .query_row(
                "SELECT pending_gc FROM blobs WHERE id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        if stored == Some(1) {
            return Err(storage_error("attachment is pending collection"));
        }
        if path.exists() {
            if fs::read(&path).map_err(storage_error)? != bytes {
                return Err(BusinessError::new(
                    ErrorKind::Conflict,
                    "blob identity cannot overwrite different bytes",
                    RetryAdvice::Never,
                ));
            }
        } else {
            if pending.exists() {
                fs::remove_file(&pending).map_err(storage_error)?;
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&pending)
                .map_err(storage_error)?;
            file.write_all(&bytes).map_err(storage_error)?;
            file.sync_all().map_err(storage_error)?;
            drop(file);
            fs::rename(&pending, &path).map_err(storage_error)?;
        }
        tx.execute("INSERT INTO blobs(id,byte_length,pending_gc) VALUES(?1,?2,0) ON CONFLICT(id) DO NOTHING", params![id.to_string(),length]).map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }
    pub fn read_blob(&self, id: AttachmentBlobId) -> Result<Vec<u8>, BusinessError> {
        let expected: Option<i64> = self
            .connection
            .query_row(
                "SELECT byte_length FROM blobs WHERE id=?1 AND pending_gc=0",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let expected = expected.ok_or_else(|| {
            BusinessError::new(
                ErrorKind::NotFound,
                "attachment is absent or being collected",
                RetryAdvice::Never,
            )
        })?;
        let bytes = fs::read(self.blob_path(id, "blob")?).map_err(storage_error)?;
        if i64::try_from(bytes.len()).ok() != Some(expected) {
            return Err(storage_error(
                "attachment length does not match its owned record",
            ));
        }
        Ok(bytes)
    }
    pub fn reference_count(&self, id: AttachmentBlobId) -> Result<u64, BusinessError> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT count(*) FROM blob_refs WHERE blob_id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        count
            .try_into()
            .map_err(|_| storage_error("invalid negative attachment reference count"))
    }
    pub fn collect_blob(&mut self, id: AttachmentBlobId) -> Result<(), BusinessError> {
        let path = self.blob_path(id, "blob")?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let owned: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blobs WHERE id=?1)",
                [id.to_string()],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        if !owned {
            return Ok(());
        } // No catalog ownership means no permission to delete a file.
        let refs: i64 = tx
            .query_row(
                "SELECT count(*) FROM blob_refs WHERE blob_id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .map_err(storage_error)?;
        if refs != 0 {
            return Err(BusinessError::new(
                ErrorKind::Conflict,
                "attachment remains referenced",
                RetryAdvice::AfterCorrection,
            ));
        }
        tx.execute(
            "UPDATE blobs SET pending_gc=1 WHERE id=?1",
            [id.to_string()],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(storage_error(e)),
        }
        // New references cannot target pending_gc. A failed unlink keeps its retryable catalog row.
        self.connection.execute("DELETE FROM blobs WHERE id=?1 AND pending_gc=1 AND NOT EXISTS(SELECT 1 FROM blob_refs WHERE blob_id=?1)", [id.to_string()]).map_err(storage_error)?;
        Ok(())
    }
    pub fn cleanup_pending(&mut self) -> Result<(), BusinessError> {
        let ids: Vec<String> = {
            let mut query = self
                .connection
                .prepare("SELECT id FROM blobs WHERE pending_gc=1")
                .map_err(storage_error)?;
            query
                .query_map([], |r| r.get(0))
                .map_err(storage_error)?
                .collect::<Result<_, _>>()
                .map_err(storage_error)?
        };
        for id in ids {
            self.collect_blob(
                id.parse()
                    .map_err(|_| storage_error("invalid owned blob identity"))?,
            )?;
        }
        Ok(())
    }
}

fn insert_record(
    connection: &Connection,
    record: &JournalRecord,
    old: &WorkbenchState,
) -> Result<(), BusinessError> {
    let (session, project, command, event) = record_scope(record, old);
    connection
        .execute(
            "INSERT INTO journal VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                number(record.sequence)?,
                session,
                project,
                command,
                event,
                encode(record)?
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}
fn save_state(connection: &Connection, state: &WorkbenchState) -> Result<(), BusinessError> {
    rebuild(connection, state)?;
    let text: Option<String> = connection
        .query_row("SELECT record_json FROM preferences WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage_error)?;
    if let Some(text) = text {
        let mut preferences: Versioned<WorkspacePreferences> = decode(&text)?;
        let snapshot = state.snapshot();
        if preferences
            .record
            .selected_project_id
            .is_some_and(|id| !snapshot.projects.iter().any(|p| p.id == id))
        {
            preferences.record.selected_project_id = None;
        }
        if preferences
            .record
            .selected_session_id
            .is_some_and(|id| !snapshot.sessions.iter().any(|s| s.id == id))
        {
            preferences.record.selected_session_id = None;
        }
        if let Some(session) = preferences
            .record
            .selected_session_id
            .and_then(|id| snapshot.sessions.iter().find(|s| s.id == id))
        {
            preferences.record.selected_project_id = Some(session.project_id);
        }
        preferences
            .record
            .expanded_projects
            .retain(|id| snapshot.projects.iter().any(|p| p.id == *id));
        let encoded = encode(&preferences)?;
        if encoded != text {
            connection
                .execute(
                    "UPDATE preferences SET record_json=?1 WHERE id=1",
                    [encoded],
                )
                .map_err(storage_error)?;
            connection
                .execute(
                    "UPDATE preference_revision SET revision=revision+1 WHERE id=1",
                    [],
                )
                .map_err(storage_error)?;
        }
    }
    connection
        .execute(
            "UPDATE workbench_state SET sequence=?1,snapshot_json=?2 WHERE id=1",
            params![
                number(state.sequence())?,
                encode(&Versioned::new(state.clone()))?
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}
