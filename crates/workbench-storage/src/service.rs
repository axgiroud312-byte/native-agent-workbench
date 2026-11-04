use crate::{
    SqliteStorage,
    database::{decode, effective_state, storage_error},
    paths::project_key,
};
use rusqlite::OptionalExtension;
use std::{fs, path::PathBuf};
use workbench_core::*;

/// Delivery metadata preserves core's committed journal/effect ordering without dispatching I/O.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommittedEffect {
    pub sequence: u64,
    pub ordinal: usize,
    pub effect: RuntimeEffect,
}
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommittedDecision {
    pub sequence: u64,
    pub disposition: ApplyDisposition,
}
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommittedUpdate {
    pub snapshot: WorkbenchSnapshot,
    /// Affected surviving sessions from the exact same committed state as snapshot/effects.
    pub sessions: Vec<SessionSnapshot>,
    pub decisions: Vec<CommittedDecision>,
    pub effects: Vec<CommittedEffect>,
    /// Business commit succeeded; failed blob cleanup remains catalogued for retry.
    pub maintenance_errors: Vec<BusinessError>,
}
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MessagePage {
    pub messages: Vec<MessageSnapshot>,
    pub next_before: Option<MessageId>,
}

/// App-facing queries and commands. All work, including path canonicalization, runs off UI.
/// Core remains the sole business reducer. Effects return only AFTER durable commit.
#[derive(Clone)]
pub struct WorkbenchService {
    storage: SqliteStorage,
}

impl WorkbenchService {
    /// Rebuild local history, then durably apply Restart before accepting new app work.
    /// Repeating the same startup identity is idempotent; another owner on this handle is rejected.
    pub fn start(
        storage: SqliteStorage,
        restart_id: CommandId,
        at: Timestamp,
    ) -> PortFuture<'static, Self> {
        let job = storage.call(move |db| {
            if let Some(owner) = db.service_started {
                if owner == restart_id {
                    return Ok(());
                }
                return Err(BusinessError::new(
                    ErrorKind::Conflict,
                    "workbench service already owns this storage worker",
                    RetryAdvice::Never,
                ));
            }
            if effective_state(&db.connection)?
                .command_result(restart_id)
                .is_some()
            {
                return Err(BusinessError::new(
                    ErrorKind::Conflict,
                    "a new app instance requires a fresh Restart command identity",
                    RetryAdvice::Never,
                ));
            }
            let update = db.apply(vec![JournalEntry::Command(CommandEnvelope::new(
                restart_id,
                at,
                Scope::default(),
                Command::Restart,
            ))])?;
            ensure_applied(&update)?;
            db.service_started = Some(restart_id);
            Ok(())
        });
        Box::pin(async move {
            job.await?;
            Ok(Self { storage })
        })
    }
    pub fn storage(&self) -> &SqliteStorage {
        &self.storage
    }
    pub fn snapshot(&self) -> PortFuture<'static, WorkbenchSnapshot> {
        self.storage
            .call(|db| Ok(effective_state(&db.connection)?.snapshot()))
    }
    pub fn command_result(&self, id: CommandId) -> PortFuture<'static, Option<CommandResult>> {
        self.storage
            .call(move |db| Ok(effective_state(&db.connection)?.command_result(id).cloned()))
    }
    pub fn preferences(&self) -> PortFuture<'_, Option<Versioned<WorkspacePreferences>>> {
        self.storage.read_preferences()
    }
    pub fn save_preferences(&self, preferences: WorkspacePreferences) -> PortFuture<'_, ()> {
        self.storage.write_preferences(Versioned::new(preferences))
    }
    /// Actual persisted edit revision, independent of Versioned's model schema version.
    pub fn preferences_with_revision(&self) -> PortFuture<'static, (u64, WorkspacePreferences)> {
        self.storage.call(|db| {
            Ok((
                db.preference_revision()?,
                db.preferences()?.map(|v| v.record).unwrap_or_default(),
            ))
        })
    }
    pub fn save_preferences_if_revision(
        &self,
        expected: u64,
        preferences: WorkspacePreferences,
    ) -> PortFuture<'static, u64> {
        self.storage.call(move |db| {
            db.save_preferences_checked(Some(expected), Versioned::new(preferences))
        })
    }
    pub fn execute(&self, command: CommandEnvelope) -> PortFuture<'static, CommittedUpdate> {
        self.storage
            .call(move |db| db.apply(vec![JournalEntry::Command(command)]))
    }
    pub fn ingest(&self, event: EventEnvelope) -> PortFuture<'static, CommittedUpdate> {
        self.ingest_batch(vec![event])
    }
    /// Text deltas may batch; semantic events are retained in full and effects keep host order.
    pub fn ingest_batch(&self, events: Vec<EventEnvelope>) -> PortFuture<'static, CommittedUpdate> {
        self.storage
            .call(move |db| db.apply(events.into_iter().map(JournalEntry::Event).collect()))
    }
    pub fn list_projects(&self) -> PortFuture<'static, Vec<Project>> {
        self.storage.call(|db| db.projects())
    }
    pub fn project(&self, id: ProjectId) -> PortFuture<'static, Option<Project>> {
        self.storage.call(move |db| db.project(id))
    }
    /// Filesystem evidence is obtained only on the storage worker, never by the UI.
    pub fn validate_project_directory(&self, id: ProjectId) -> PortFuture<'static, PathBuf> {
        self.storage.call(move |db| {
            let project = db.project(id)?.ok_or_else(|| {
                BusinessError::new(ErrorKind::NotFound, "project is absent", RetryAdvice::Never)
            })?;
            checked_directory(project.directory)
        })
    }
    /// Explicit relocation keeps all historical objects/bindings intact. Core gates resources.
    pub fn relocate_project(
        &self,
        command_id: CommandId,
        id: ProjectId,
        directory: PathBuf,
        at: Timestamp,
    ) -> PortFuture<'static, CommittedUpdate> {
        self.storage.call(move |db| {
            let directory = checked_directory(directory)?;
            let key = project_key(&directory)?;
            let occupant: Option<String> = db
                .connection
                .query_row(
                    "SELECT id FROM project_index WHERE path_key=?1",
                    [key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            if occupant.is_some_and(|owner| owner != id.to_string()) {
                return Err(BusinessError::new(
                    ErrorKind::Conflict,
                    "directory already belongs to another project; projects were not merged",
                    RetryAdvice::AfterCorrection,
                ));
            }
            let mut project = db.project(id)?.ok_or_else(|| {
                BusinessError::new(ErrorKind::NotFound, "project is absent", RetryAdvice::Never)
            })?;
            project.directory = directory;
            db.apply(vec![JournalEntry::Command(CommandEnvelope::new(
                command_id,
                at,
                Scope::default(),
                Command::PutProject { project },
            ))])
        })
    }
    pub fn list_sessions(&self, project: ProjectId) -> PortFuture<'static, Vec<SessionRecord>> {
        self.storage.call(move |db| db.sessions(project))
    }
    pub fn session(&self, id: SessionId) -> PortFuture<'static, Option<SessionSnapshot>> {
        self.storage.call(move |db| db.session(id))
    }

    /// Canonical directory identity is centralized here. Existing user names are not overwritten.
    pub fn open_project(
        &self,
        command_id: CommandId,
        directory: PathBuf,
        name: String,
        at: Timestamp,
    ) -> PortFuture<'static, Project> {
        self.storage.call(move |db| {
            let directory = checked_directory(directory)?;
            let key = project_key(&directory)?;
            let existing: Option<String> = db
                .connection
                .query_row(
                    "SELECT record_json FROM project_index WHERE path_key=?1",
                    [key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            if let Some(json) = existing {
                return decode::<Versioned<Project>>(&json).map(|v| v.record);
            }
            let project = Project::new(ProjectId::new(), name, directory, at);
            let update = db.apply(vec![JournalEntry::Command(CommandEnvelope::new(
                command_id,
                at,
                Scope::default(),
                Command::PutProject {
                    project: project.clone(),
                },
            ))])?;
            ensure_applied(&update)?;
            db.project(project.id)?.ok_or_else(|| {
                BusinessError::new(
                    ErrorKind::Conflict,
                    "command identity was already used for another operation",
                    RetryAdvice::Never,
                )
            })
        })
    }
    /// Caller keeps these IDs across retries, rather than accidentally creating a second session.
    pub fn create_session(
        &self,
        command_id: CommandId,
        session: SessionRecord,
    ) -> PortFuture<'static, SessionRecord> {
        self.storage.call(move |db| {
            let id = session.id();
            let at = session.created_at;
            let update = db.apply(vec![JournalEntry::Command(CommandEnvelope::new(
                command_id,
                at,
                Scope::default(),
                Command::CreateSession { session },
            ))])?;
            ensure_applied(&update)?;
            db.session(id)?.map(|s| s.record).ok_or_else(|| {
                BusinessError::new(
                    ErrorKind::NotFound,
                    "session identity is absent",
                    RetryAdvice::Never,
                )
            })
        })
    }
    pub fn select_project(&self, id: ProjectId) -> PortFuture<'static, ()> {
        self.storage.call(move |db| {
            if db.project(id)?.is_none() {
                return Err(BusinessError::new(
                    ErrorKind::NotFound,
                    "project is not registered",
                    RetryAdvice::Never,
                ));
            }
            let mut preferences = db
                .preferences()?
                .unwrap_or_else(|| Versioned::new(WorkspacePreferences::default()));
            preferences.record.selected_project_id = Some(id);
            if let Some(session_id) = preferences.record.selected_session_id {
                if db
                    .session(session_id)?
                    .is_none_or(|s| s.record.project_id() != id)
                {
                    preferences.record.selected_session_id = None;
                }
            }
            db.save_preferences(preferences)
        })
    }
    pub fn select_session(&self, id: SessionId) -> PortFuture<'static, ()> {
        self.storage.call(move |db| {
            let session = db.session(id)?;
            if session.is_none() {
                return Err(BusinessError::new(
                    ErrorKind::NotFound,
                    "session is not registered",
                    RetryAdvice::Never,
                ));
            }
            let mut preferences = db
                .preferences()?
                .unwrap_or_else(|| Versioned::new(WorkspacePreferences::default()));
            preferences.record.selected_session_id = Some(id);
            preferences.record.selected_project_id = Some(session.unwrap().record.project_id());
            db.save_preferences(preferences)
        })
    }
    pub fn remove_session(
        &self,
        command_id: CommandId,
        id: SessionId,
        at: Timestamp,
    ) -> PortFuture<'static, CommittedUpdate> {
        self.storage.call(move |db| {
            let session = db.session(id)?.ok_or_else(|| {
                BusinessError::new(
                    ErrorKind::NotFound,
                    "session is not registered",
                    RetryAdvice::Never,
                )
            })?;
            let update = db.apply(vec![JournalEntry::Command(CommandEnvelope::new(
                command_id,
                at,
                Scope::session(id, session.record.connection.epoch),
                Command::DeleteSession,
            ))])?;
            ensure_applied(&update)?;
            Ok(update)
        })
    }
    pub fn remove_project(
        &self,
        command_id: CommandId,
        id: ProjectId,
        at: Timestamp,
    ) -> PortFuture<'static, CommittedUpdate> {
        self.storage.call(move |db| {
            let update = db.apply(vec![JournalEntry::Command(CommandEnvelope::new(
                command_id,
                at,
                Scope::default(),
                Command::DeleteProject { project_id: id },
            ))])?;
            ensure_applied(&update)?;
            Ok(update)
        })
    }
    /// UI history contains typed messages, not arbitrary internal event JSON.
    pub fn messages(
        &self,
        id: SessionId,
        before: Option<MessageId>,
        limit: u32,
    ) -> PortFuture<'static, MessagePage> {
        self.storage.call(move |db| {
            if limit == 0 || limit > 500 {
                return Err(BusinessError::new(
                    ErrorKind::InvalidInput,
                    "message page limit must be 1..=500",
                    RetryAdvice::AfterCorrection,
                ));
            }
            let snapshot = db.session(id)?.ok_or_else(|| {
                BusinessError::new(
                    ErrorKind::NotFound,
                    "session is not registered",
                    RetryAdvice::Never,
                )
            })?;
            let end = match before {
                Some(id) => snapshot
                    .messages
                    .iter()
                    .position(|m| m.id == id)
                    .ok_or_else(|| {
                        BusinessError::new(
                            ErrorKind::NotFound,
                            "history cursor is not in this session",
                            RetryAdvice::Never,
                        )
                    })?,
                None => snapshot.messages.len(),
            };
            let start = end.saturating_sub(limit as usize);
            Ok(MessagePage {
                next_before: (start > 0).then(|| snapshot.messages[start].id),
                messages: snapshot.messages[start..end].to_vec(),
            })
        })
    }
}

fn checked_directory(directory: PathBuf) -> Result<PathBuf, BusinessError> {
    let directory = fs::canonicalize(directory).map_err(|_| {
        BusinessError::new(
            ErrorKind::InvalidInput,
            "project directory is unavailable; select its actual location",
            RetryAdvice::AfterCorrection,
        )
    })?;
    if !directory.is_dir() {
        return Err(BusinessError::new(
            ErrorKind::InvalidInput,
            "project path is not a directory",
            RetryAdvice::AfterCorrection,
        ));
    }
    Ok(directory)
}

fn ensure_applied(update: &CommittedUpdate) -> Result<(), BusinessError> {
    for decision in &update.decisions {
        if let ApplyDisposition::Rejected(error) = &decision.disposition {
            return Err(error.clone());
        }
    }
    Ok(())
}
