use crate::{ApplicationService, ManualLaunch, ServiceOptions};
use std::{path::PathBuf, time::Duration};
use workbench_core::*;

fn command(scope: Scope, command: Command) -> CommandEnvelope {
    CommandEnvelope::new(CommandId::new(), Timestamp::new(1), scope, command)
}

/// One real SQLite/background-thread scenario: unpolled admission order, selection
/// attribution, persisted preference CAS, rejected-command dedup and graceful reopen.
/// Optional explicit adapter paths extend this same scenario with real initialize-only
/// discovery; no prompt, package install, browser or credential input is performed.
#[test]
fn service_admission_selection_restart_and_optional_real_handshake() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(90), async {
                let root = tempfile::tempdir().unwrap();
                let project_dir = tempfile::tempdir().unwrap();
                let options = ServiceOptions::new().with_data_root(root.path().to_path_buf());
                let service = ApplicationService::start(options.clone()).await.unwrap();
                let project = service
                    .open_project(project_dir.path().to_path_buf(), "真实服务检查".into())
                    .await
                    .unwrap();
                let agent = AgentDefinition::new(
                    AgentId::new(),
                    "离线定义",
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
                let a = SessionRecord::new(
                    SessionId::new(),
                    project.id,
                    agent.id,
                    "A",
                    Timestamp::new(1),
                );
                let b = SessionRecord::new(
                    SessionId::new(),
                    project.id,
                    agent.id,
                    "B",
                    Timestamp::new(1),
                );
                for session in [&a, &b] {
                    service
                        .execute(command(
                            Scope::default(),
                            Command::CreateSession {
                                session: session.clone(),
                            },
                        ))
                        .await
                        .unwrap();
                }
                let scope = Scope::session(a.id(), ConnectionEpoch::INITIAL);
                let mut draft = ComposerDraft::new(a.id(), Timestamp::new(2));
                draft.revision = 1;
                draft.content = vec![InputPart::Text {
                    text: "第一份".into(),
                }];
                let first = service.execute(command(
                    scope,
                    Command::SaveDraft {
                        draft: draft.clone(),
                        expected_revision: 0,
                    },
                ));
                draft.revision = 2;
                draft.content = vec![InputPart::Text {
                    text: "第二份".into(),
                }];
                let second = service.execute(command(
                    scope,
                    Command::SaveDraft {
                        draft: draft.clone(),
                        expected_revision: 1,
                    },
                ));
                // Poll the second reply first. Admission happened at the two method calls above.
                assert_eq!(second.await.unwrap().result, CommandStatus::Applied);
                assert_eq!(first.await.unwrap().result, CommandStatus::Applied);
                let select_a = service.select_session(Some(a.id()));
                let select_b = service.select_session(Some(b.id()));
                let generation = select_b.await.unwrap();
                assert!(select_a.await.unwrap() < generation);
                let query = service.query_session(a.id()).await.unwrap();
                assert_eq!(query.snapshot.unwrap().draft, draft);
                assert_eq!(
                    service
                        .snapshot()
                        .active_session
                        .as_ref()
                        .unwrap()
                        .record
                        .id(),
                    b.id()
                );
                assert_eq!(service.snapshot().selection.generation, generation);
                let snapshot = service.snapshot();
                let mut preferences = snapshot.preferences.clone();
                preferences.zoom_percent = 125;
                let save =
                    service.save_preferences(snapshot.preferences_revision, preferences.clone());
                let stale = service
                    .save_preferences(snapshot.preferences_revision, snapshot.preferences.clone());
                assert_eq!(stale.await.unwrap_err().kind, ErrorKind::Conflict);
                let preference_revision = save.await.unwrap();
                assert_eq!(service.snapshot().preferences, preferences);
                let rejected = command(
                    scope,
                    Command::SaveDraft {
                        draft: draft.clone(),
                        expected_revision: 0,
                    },
                );
                assert!(matches!(
                    service.execute(rejected.clone()).await.unwrap().result,
                    CommandStatus::Rejected { .. }
                ));
                assert!(matches!(
                    service.execute(rejected).await.unwrap().result,
                    CommandStatus::Rejected { .. }
                ));

                if let (Some(node), Some(adapter)) = (
                    std::env::var_os("WORKBENCH_SMOKE_NODE"),
                    std::env::var_os("WORKBENCH_SMOKE_ADAPTER"),
                ) {
                    let id = service
                        .configure_agent(
                            None,
                            "真实 Codex adapter".into(),
                            ManualLaunch::new(
                                PathBuf::from(node),
                                vec![PathBuf::from(adapter).to_string_lossy().into_owned()],
                            ),
                        )
                        .await
                        .unwrap();
                    let mut subscription = service.subscribe();
                    let deadline = std::time::Instant::now() + Duration::from_secs(75);
                    loop {
                        let frame = subscription.current();
                        if let Some(auth) = frame.authentication.iter().find(|view| {
                            view.scope.agent_id() == id && view.scope.session_id().is_none()
                        }) {
                            if !auth.methods.is_empty()
                                && !matches!(auth.status, AuthenticationStatus::Discovering)
                            {
                                assert!(auth.methods.iter().any(|m| m.id.as_str() == "chat-gpt"));
                                assert!(matches!(
                                    auth.status,
                                    AuthenticationStatus::Unknown
                                        | AuthenticationStatus::Authenticated
                                ));
                                println!(
                                    "real initialize methods: {}",
                                    auth.methods
                                        .iter()
                                        .map(|m| m.id.as_str())
                                        .collect::<Vec<_>>()
                                        .join(",")
                                );
                                break;
                            }
                            assert!(
                                !matches!(auth.status, AuthenticationStatus::Failed { .. }),
                                "sanitized auth status: {:?}",
                                auth.status
                            );
                        }
                        assert!(
                            std::time::Instant::now() < deadline,
                            "authentication discovery timed out"
                        );
                        subscription.changed().await.unwrap();
                    }
                }
                let (reply, received) = tokio::sync::oneshot::channel();
                service
                    .sender
                    .try_send(super::Message::FailNextViewSync { reply })
                    .ok()
                    .unwrap();
                received.await.unwrap().unwrap();
                let connected = service
                    .execute(command(scope, Command::Connect { resume: false }))
                    .await
                    .unwrap();
                assert_eq!(connected.result, CommandStatus::Applied);
                let mut facts = service.subscribe();
                loop {
                    let state = service
                        .query_session(a.id())
                        .await
                        .unwrap()
                        .snapshot
                        .unwrap();
                    if matches!(
                        state.record.connection.phase,
                        ConnectionPhase::Failed { .. } | ConnectionPhase::Closed
                    ) {
                        assert!(
                            state
                                .errors
                                .iter()
                                .any(|error| error.kind == ErrorKind::NotFound)
                        );
                        break;
                    }
                    facts.changed().await.unwrap();
                }
                // Connect had no configured launch, so Runtime emits a real attributable failure.
                // Reaching this state proves an ancillary read failure did not swallow its effect.
                service.shutdown().await.unwrap();
                drop(service);
                let reopened = ApplicationService::start(options).await.unwrap();
                assert_eq!(
                    reopened.snapshot().preferences_revision,
                    preference_revision
                );
                assert_eq!(reopened.snapshot().preferences, preferences);
                assert!(reopened.snapshot().authentication.is_empty());
                assert_eq!(
                    reopened
                        .snapshot()
                        .active_session
                        .as_ref()
                        .unwrap()
                        .record
                        .id(),
                    b.id()
                );
                let restored = reopened
                    .query_session(a.id())
                    .await
                    .unwrap()
                    .snapshot
                    .unwrap();
                assert_eq!(restored.draft, draft);
                assert!(matches!(
                    restored.queue.dispatch,
                    QueueDispatchState::Paused { .. }
                ));
                reopened.shutdown().await.unwrap();
            })
            .await
            .expect("bounded service scenario timed out");
        });
}
