use futures_executor::block_on;
use std::{collections::BTreeSet, fs};
use workbench_core::*;
use workbench_storage::*;

// Real one-pixel transparent PNG; tests verify opaque byte persistence, not image rendering.
const IMAGE: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 11, 73, 68, 65, 84, 120, 156, 99, 96, 0, 2, 0, 0, 5, 0, 1,
    122, 94, 171, 63, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn command(scope: Scope, command: Command) -> CommandEnvelope {
    CommandEnvelope::new(CommandId::new(), Timestamp::new(1000), scope, command)
}
fn event(scope: Scope, event: Event) -> EventEnvelope {
    EventEnvelope::new(
        EventId::new(),
        Timestamp::new(2000),
        scope,
        EventSource::new(
            EventSourceId::new(),
            EventOrigin::Runtime {
                adapter: "contract-fixture".into(),
                version: "1".into(),
            },
        ),
        event,
    )
}

#[test]
fn service_round_trip_real_transaction_rollback_and_bounded_project_deletion() {
    block_on(async {
        assert!(default_data_directory().unwrap().is_absolute());
        let isolated = tempfile::tempdir().unwrap();
        let data = isolated.path().join("应用数据");
        let source = isolated.path().join("用户项目 中文");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("源码.txt"), "保留我的源码").unwrap();
        let outside = isolated.path().join("其他客户端历史.json");
        fs::write(&outside, "external history").unwrap();
        let storage = SqliteStorage::open_at(data.clone()).await.unwrap();
        let service = WorkbenchService::start(storage.clone(), CommandId::new(), Timestamp::new(1))
            .await
            .unwrap();
        let before = service.snapshot().await.unwrap().sequence;
        let connection = rusqlite::Connection::open(data.join("workbench.sqlite3")).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON workbench_state BEGIN SELECT RAISE(ABORT,'forced checkpoint failure'); END;").unwrap();
        let create_id = CommandId::new();
        assert!(
            service
                .open_project(
                    create_id,
                    source.clone(),
                    "我的项目".into(),
                    Timestamp::new(2)
                )
                .await
                .is_err()
        );
        assert!(service.list_projects().await.unwrap().is_empty());
        assert_eq!(service.snapshot().await.unwrap().sequence, before);
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM journal", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            before as i64
        );
        connection
            .execute_batch("DROP TRIGGER fail_checkpoint;")
            .unwrap();
        drop(connection);
        let project = service
            .open_project(
                create_id,
                source.clone(),
                "我的项目".into(),
                Timestamp::new(2),
            )
            .await
            .unwrap();
        let same = service
            .open_project(
                CommandId::new(),
                source.join("."),
                "不覆盖已有名字".into(),
                Timestamp::new(3),
            )
            .await
            .unwrap();
        assert_eq!(same.id, project.id);
        assert_eq!(same.name, "我的项目");
        assert_eq!(service.list_projects().await.unwrap().len(), 1);
        let agent = AgentDefinition::new(
            AgentId::new(),
            "本地 Agent",
            AgentSource::LocalConfiguration,
        );
        service
            .execute(command(
                Scope::default(),
                Command::PutAgent {
                    agent: agent.clone(),
                },
            ))
            .await
            .unwrap();
        let session = SessionRecord::new(
            SessionId::new(),
            project.id,
            agent.id,
            "中文会话",
            Timestamp::new(4),
        );
        let create_session_id = CommandId::new();
        service
            .create_session(create_session_id, session.clone())
            .await
            .unwrap();
        assert_eq!(
            service
                .create_session(create_session_id, session.clone())
                .await
                .unwrap()
                .id(),
            session.id()
        );
        assert_eq!(service.list_sessions(project.id).await.unwrap().len(), 1);
        service.select_project(project.id).await.unwrap();
        service.select_session(session.id()).await.unwrap();
        let mut preferences = service.preferences().await.unwrap().unwrap().record;
        preferences.sidebar_width = 300;
        preferences.zoom_percent = 125;
        preferences.expanded_projects.insert(project.id);
        service.save_preferences(preferences.clone()).await.unwrap();
        let scope = Scope::session(session.id(), ConnectionEpoch::INITIAL);
        let mut draft = ComposerDraft::new(session.id(), Timestamp::new(5));
        draft.revision = 1;
        draft.content = vec![
            InputPart::Text {
                text: "发送前的中文草稿".into(),
            },
            InputPart::FileReference {
                reference: FileReference::new(
                    FileReferenceId::new(),
                    project.id,
                    "源码.txt".into(),
                ),
            },
        ];
        service
            .execute(command(
                scope,
                Command::SaveDraft {
                    draft: draft.clone(),
                    expected_revision: 0,
                },
            ))
            .await
            .unwrap();
        let queued = QueuedInput::from_draft(QueuedInputId::new(), &draft, Timestamp::new(6));
        service
            .execute(command(
                scope,
                Command::Enqueue {
                    input: queued.clone(),
                },
            ))
            .await
            .unwrap();
        drop(service);
        storage.shutdown().await.unwrap();

        let storage = SqliteStorage::open_at(data.clone()).await.unwrap();
        let service =
            WorkbenchService::start(storage.clone(), CommandId::new(), Timestamp::new(10))
                .await
                .unwrap();
        let recovered = service.session(session.id()).await.unwrap().unwrap();
        assert_eq!(recovered.draft, draft);
        assert_eq!(recovered.queue.entries[0].content, queued.content);
        assert!(matches!(
            recovered.queue.dispatch,
            QueueDispatchState::Paused {
                reason: QueuePauseReason::ApplicationRestart,
                ..
            }
        ));
        assert_eq!(
            service.preferences().await.unwrap().unwrap().record,
            preferences
        );
        assert_eq!(
            service.project(project.id).await.unwrap().unwrap().name,
            "我的项目"
        );
        let removed = service
            .remove_project(CommandId::new(), project.id, Timestamp::new(11))
            .await
            .unwrap();
        assert!(removed.effects.is_empty());
        assert!(service.project(project.id).await.unwrap().is_none());
        assert!(service.session(session.id()).await.unwrap().is_none());
        let preferences = service.preferences().await.unwrap().unwrap().record;
        assert_eq!(preferences.selected_session_id, None);
        assert_eq!(preferences.selected_project_id, None);
        assert_eq!(
            fs::read_to_string(source.join("源码.txt")).unwrap(),
            "保留我的源码"
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), "external history");
        drop(service);
        storage.shutdown().await.unwrap();
    });
}

#[test]
fn journal_checkpoint_cas_history_and_shared_owned_blob_cleanup_use_real_storage() {
    block_on(async {
        let isolated = tempfile::tempdir().unwrap();
        let data = isolated.path().join("data");
        let source = isolated.path().join("用户图片.png");
        fs::write(&source, IMAGE).unwrap();
        let outside = isolated.path().join("external-history.txt");
        fs::write(&outside, "keep").unwrap();
        let storage = SqliteStorage::open_at(data.clone()).await.unwrap();
        let fixture = workbench_core::examples::interleaved_example();
        let mut state = WorkbenchState::new();
        let blob = fixture
            .records
            .iter()
            .find_map(|r| match &r.entry {
                JournalEntry::Command(c) => match &c.command {
                    Command::PutAttachment { attachment } => match attachment.storage {
                        AttachmentStorage::Managed { blob_id } => Some(blob_id),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        storage
            .write_attachment(blob, IMAGE.to_vec())
            .await
            .unwrap();
        for record in &fixture.records {
            let transition = reduce(&state, record, ApplyMode::Live).unwrap();
            assert_eq!(transition.disposition(), &ApplyDisposition::Applied);
            storage
                .commit(StorageCommit::new(
                    state.sequence(),
                    record.clone(),
                    transition.state().clone(),
                ))
                .await
                .unwrap();
            state = transition.into_parts().0;
        }
        let rename = JournalRecord::new(
            state.sequence() + 1,
            JournalEntry::Command(command(
                Scope::session(fixture.sessions[0], ConnectionEpoch::new(1)),
                Command::RenameSession {
                    title: "持久名字".into(),
                },
            )),
        );
        let renamed = reduce(&state, &rename, ApplyMode::Live)
            .unwrap()
            .into_parts()
            .0;
        let batch = StorageCommit::new(state.sequence(), rename.clone(), renamed.clone());
        storage.commit(batch.clone()).await.unwrap();
        assert_eq!(
            storage.commit(batch).await.unwrap_err().kind,
            ErrorKind::SequenceConflict
        );
        let recovered = storage.restore().await.unwrap();
        assert_eq!(
            replay(recovered.checkpoint.unwrap().record, &recovered.tail).unwrap(),
            renamed
        );
        let expected = fixture.records.iter().filter(|r| match &r.entry {
            JournalEntry::Command(c) => c.scope.session_id == Some(fixture.sessions[0]) || matches!(&c.command,Command::CreateSession { session } if session.id()==fixture.sessions[0]),
            JournalEntry::Event(e) => e.scope.session_id == Some(fixture.sessions[0]),
        }).count()+1;
        let mut seen = BTreeSet::new();
        let mut before = None;
        loop {
            let page = storage
                .history(HistoryQuery::new(fixture.sessions[0], before, 3))
                .await
                .unwrap();
            assert!(
                page.records
                    .windows(2)
                    .all(|pair| pair[0].sequence < pair[1].sequence)
            );
            for record in page.records {
                assert!(seen.insert(record.sequence));
            }
            match page.next_before_sequence {
                Some(next) => before = Some(next),
                None => break,
            }
        }
        assert_eq!(seen.len(), expected);
        assert_eq!(storage.read_attachment(blob).await.unwrap(), IMAGE);
        assert!(storage.write_attachment(blob, vec![1]).await.is_err());
        assert_eq!(storage.attachment_reference_count(blob).await.unwrap(), 1);
        assert!(storage.collect_attachment(blob).await.is_err());
        let service =
            WorkbenchService::start(storage.clone(), CommandId::new(), Timestamp::new(5000))
                .await
                .unwrap();
        let first = service.session(fixture.sessions[0]).await.unwrap().unwrap();
        assert!(matches!(
            first.runs[0].state,
            ExecutionState::Interrupted { .. }
        ));
        assert!(first.permissions.iter().all(|r| !r.is_actionable));
        assert!(matches!(
            first.queue.dispatch,
            QueueDispatchState::Paused {
                reason: QueuePauseReason::ApplicationRestart,
                ..
            }
        ));
        assert_eq!(
            service
                .messages(fixture.sessions[0], None, 1)
                .await
                .unwrap()
                .messages
                .len(),
            1
        );
        let mut attachment = first.attachments[0].clone();
        attachment.id = AttachmentId::new();
        attachment.session_id = fixture.sessions[1];
        attachment.source = AttachmentSource::ImportedFile {
            original_path: source.clone(),
        };
        let epoch = service
            .session(fixture.sessions[1])
            .await
            .unwrap()
            .unwrap()
            .record
            .connection
            .epoch;
        service
            .execute(command(
                Scope::session(fixture.sessions[1], epoch),
                Command::PutAttachment { attachment },
            ))
            .await
            .unwrap();
        assert_eq!(storage.attachment_reference_count(blob).await.unwrap(), 2);
        // Deletion cannot invent a runtime stop: release evidence is a typed test fact only.
        assert!(
            service
                .remove_session(CommandId::new(), fixture.sessions[0], Timestamp::new(5001))
                .await
                .is_err()
        );
        let mut releases = Vec::new();
        for id in fixture.sessions {
            let epoch = service
                .session(id)
                .await
                .unwrap()
                .unwrap()
                .record
                .connection
                .epoch;
            releases.push(event(Scope::session(id, epoch), Event::ResourcesReleased));
        }
        let update = service.ingest_batch(releases).await.unwrap();
        assert_eq!(update.sessions.len(), 2);
        assert!(update.effects.is_empty());
        service
            .remove_session(CommandId::new(), fixture.sessions[0], Timestamp::new(5002))
            .await
            .unwrap();
        assert_eq!(storage.attachment_reference_count(blob).await.unwrap(), 1);
        assert_eq!(storage.read_attachment(blob).await.unwrap(), IMAGE);
        service
            .remove_session(CommandId::new(), fixture.sessions[1], Timestamp::new(5003))
            .await
            .unwrap();
        assert_eq!(storage.attachment_reference_count(blob).await.unwrap(), 0);
        assert!(storage.read_attachment(blob).await.is_err());
        assert!(!data.join("blobs").join(format!("{blob}.blob")).exists());
        assert_eq!(fs::read(&source).unwrap(), IMAGE);
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
        let connection = rusqlite::Connection::open(data.join("workbench.sqlite3")).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM journal WHERE session_id=?1 AND record_json IS NOT NULL",
                    [fixture.sessions[0].to_string()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(connection);
        drop(service);
        storage.shutdown().await.unwrap();
    });
}
