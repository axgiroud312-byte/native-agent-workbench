use crate::database::{decode, effective_state, encode, rebuild, record_scope, storage_error};
use rusqlite::{Connection, TransactionBehavior};
use workbench_core::*;

pub(crate) const APPLICATION_ID: i32 = 0x41574231;
pub(crate) const VERSION: i32 = 2;
pub(crate) const V1: &str = "
CREATE TABLE workbench_state (id INTEGER PRIMARY KEY CHECK(id=1), sequence INTEGER NOT NULL, snapshot_json TEXT NOT NULL);
CREATE TABLE journal (sequence INTEGER PRIMARY KEY, session_id TEXT, record_json TEXT NOT NULL);
CREATE TABLE preferences (id INTEGER PRIMARY KEY CHECK(id=1), record_json TEXT NOT NULL);
CREATE TABLE blobs (id TEXT PRIMARY KEY, byte_length INTEGER NOT NULL CHECK(byte_length>=0));
";

/// All version changes, including ALTER/rebuild/backfill, commit or roll back together.
pub(crate) fn migrate(connection: &mut Connection) -> Result<(), BusinessError> {
    let application: i32 = connection
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(storage_error)?;
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(storage_error)?;
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .map_err(storage_error)?;
    if application != APPLICATION_ID && !(application == 0 && version == 0 && tables == 0) {
        return Err(BusinessError::new(
            ErrorKind::Storage,
            "database is not owned by Agent Workbench",
            RetryAdvice::Never,
        ));
    }
    if !(0..=VERSION).contains(&version) {
        return Err(BusinessError::new(
            ErrorKind::Storage,
            "unsupported SQLite schema version",
            RetryAdvice::Never,
        ));
    }
    if version == VERSION {
        return Ok(());
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)?;
    if version == 0 {
        tx.execute_batch(V1).map_err(storage_error)?;
        let state = Versioned::new(WorkbenchState::new());
        tx.execute(
            "INSERT INTO workbench_state VALUES(1,0,?1)",
            [encode(&state)?],
        )
        .map_err(storage_error)?;
    }
    // A real v1 -> v2 migration, not CREATE IF NOT EXISTS against old columns.
    tx.execute_batch("
ALTER TABLE blobs ADD COLUMN pending_gc INTEGER NOT NULL DEFAULT 0 CHECK(pending_gc IN(0,1));
ALTER TABLE journal RENAME TO journal_v1;
CREATE TABLE journal (sequence INTEGER PRIMARY KEY, session_id TEXT, project_id TEXT, command_id TEXT, event_id TEXT, record_json TEXT);
INSERT INTO journal(sequence,session_id,record_json) SELECT sequence,session_id,record_json FROM journal_v1;
DROP TABLE journal_v1;
CREATE INDEX journal_session_sequence ON journal(session_id,sequence);
CREATE INDEX journal_command ON journal(command_id);
CREATE TABLE project_index (id TEXT PRIMARY KEY, path_key TEXT NOT NULL UNIQUE, name TEXT NOT NULL, record_json TEXT NOT NULL);
CREATE TABLE session_index (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project_index(id) ON DELETE CASCADE, record_json TEXT NOT NULL, snapshot_json TEXT NOT NULL);
CREATE INDEX session_project ON session_index(project_id);
CREATE TABLE blob_refs (blob_id TEXT NOT NULL REFERENCES blobs(id), session_id TEXT NOT NULL REFERENCES session_index(id) ON DELETE CASCADE, attachment_id TEXT NOT NULL, PRIMARY KEY(blob_id,session_id,attachment_id));
").map_err(storage_error)?;
    let state = effective_state(&tx)?;
    let records: Vec<(i64, String)> = {
        let mut statement = tx
            .prepare("SELECT sequence,record_json FROM journal ORDER BY sequence")
            .map_err(storage_error)?;
        statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(storage_error)?
            .collect::<Result<_, _>>()
            .map_err(storage_error)?
    };
    for (sequence, json) in records {
        let record: JournalRecord = decode(&json)?;
        let metadata = record_scope(&record, &state);
        tx.execute("UPDATE journal SET session_id=?1,project_id=?2,command_id=?3,event_id=?4 WHERE sequence=?5", rusqlite::params![metadata.0,metadata.1,metadata.2,metadata.3,sequence]).map_err(storage_error)?;
    }
    rebuild(&tx, &state)?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(storage_error)?;
    tx.pragma_update(None, "user_version", VERSION)
        .map_err(storage_error)?;
    tx.commit().map_err(storage_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    #[test]
    fn real_v1_upgrade_preserves_records_and_failed_migration_rolls_back() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = workbench_core::examples::interleaved_example();
        let state = replay(WorkbenchState::new(), &fixture.records).unwrap();
        let blob_id = state
            .session_snapshot(fixture.sessions[0])
            .unwrap()
            .attachments
            .iter()
            .find_map(|a| match a.storage {
                AttachmentStorage::Managed { blob_id } => Some(blob_id),
                _ => None,
            })
            .unwrap();
        std::fs::create_dir(directory.path().join("blobs")).unwrap();
        let bytes = b"legacy opaque attachment bytes";
        std::fs::write(
            directory
                .path()
                .join("blobs")
                .join(format!("{blob_id}.blob")),
            bytes,
        )
        .unwrap();
        let path = directory.path().join("workbench.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(V1).unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        let json = encode(&Versioned::new(state.clone())).unwrap();
        connection
            .execute(
                "INSERT INTO workbench_state VALUES(1,?1,?2)",
                rusqlite::params![state.sequence() as i64, json],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO blobs VALUES(?1,?2)",
                rusqlite::params![blob_id.to_string(), bytes.len() as i64],
            )
            .unwrap();
        for record in &fixture.records {
            connection
                .execute(
                    "INSERT INTO journal VALUES(?1,NULL,?2)",
                    rusqlite::params![record.sequence as i64, encode(record).unwrap()],
                )
                .unwrap();
        }
        let mut preferences = WorkspacePreferences::default();
        preferences.sidebar_width = 304;
        preferences.selected_session_id = Some(fixture.sessions[0]);
        connection
            .execute(
                "INSERT INTO preferences VALUES(1,?1)",
                [encode(&Versioned::new(preferences.clone())).unwrap()],
            )
            .unwrap();
        drop(connection);
        let db = Database::open(directory.path().to_path_buf()).unwrap();
        assert_eq!(
            db.connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            2
        );
        assert_eq!(effective_state(&db.connection).unwrap(), state);
        assert_eq!(db.projects().unwrap()[0].name, "中文项目");
        assert_eq!(db.preferences().unwrap().unwrap().record, preferences);
        assert_eq!(db.reference_count(blob_id).unwrap(), 1);
        assert_eq!(db.read_blob(blob_id).unwrap(), bytes);
        assert_eq!(
            db.connection
                .query_row("SELECT count(*) FROM journal", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            fixture.records.len() as i64
        );
        drop(db);

        let failed = tempfile::tempdir().unwrap();
        let path = failed.path().join("workbench.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(V1).unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        let original = encode(&Versioned::new(WorkbenchState::new())).unwrap();
        connection
            .execute("INSERT INTO workbench_state VALUES(1,0,?1)", [&original])
            .unwrap();
        // A real SQL failure occurs after ALTER and table-rebuild work has begun.
        connection.execute_batch("CREATE TABLE project_index(sentinel TEXT); INSERT INTO project_index VALUES('保留');").unwrap();
        drop(connection);
        assert!(Database::open(failed.path().to_path_buf()).is_err());
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM pragma_table_info('blobs') WHERE name='pending_gc'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT snapshot_json FROM workbench_state", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            original
        );
        assert_eq!(
            connection
                .query_row("SELECT sentinel FROM project_index", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "保留"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='journal_v1'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
