use super::*;
use std::time::Duration;

fn command(scope: Scope, command: Command) -> CommandEnvelope {
    CommandEnvelope::new(CommandId::new(), now(), scope, command)
}
async fn session(service: &ApplicationService, id: SessionId) -> SessionSnapshot {
    service.query_session(id).await.unwrap().snapshot.unwrap()
}
async fn released(service: &ApplicationService, id: SessionId) {
    let mut updates = service.subscribe();
    loop {
        if session(service, id).await.resources_released {
            return;
        }
        updates.changed().await.unwrap();
    }
}

/// Real filesystem/SQLite/application/Runtime ownership test. A quiet owned subprocess
/// waits during initialize; it is deliberately not a real ACP Agent or a Codex login test.
#[test]
fn project_session_navigation_and_release_boundaries() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(55), async {
                let root = tempfile::tempdir().unwrap();
                let a_dir = tempfile::tempdir().unwrap();
                let b_dir = tempfile::tempdir().unwrap();
                let new_dir = tempfile::tempdir().unwrap();
                let old_path = a_dir.path().join("用户文件.txt");
                std::fs::write(&old_path, "保留项目文件").unwrap();
                let b_file = b_dir.path().join("b.txt");
                std::fs::write(&b_file, "B").unwrap();
                let options = ServiceOptions::new().with_data_root(root.path().to_path_buf());
                let service = ApplicationService::start(options.clone()).await.unwrap();
                let a = service
                    .open_project(a_dir.path().to_path_buf(), "项目 A".into())
                    .await
                    .unwrap();
                let b = service
                    .open_project(b_dir.path().to_path_buf(), "项目 B".into())
                    .await
                    .unwrap();
                assert_eq!(
                    service
                        .open_project(a_dir.path().to_path_buf(), "不覆盖名称".into())
                        .await
                        .unwrap()
                        .id,
                    a.id
                );
                assert_eq!(service.snapshot().workbench.projects.len(), 2);
                assert_eq!(
                    service
                        .open_project(root.path().join("missing"), "无效目录".into())
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::InvalidInput
                );
                #[cfg(windows)]
                let launch = AgentLaunchConfiguration::new(
                    PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                        .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
                    vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        "Start-Sleep -Seconds 90".into(),
                    ],
                );
                #[cfg(not(windows))]
                let launch =
                    AgentLaunchConfiguration::new(PathBuf::from("/bin/sleep"), vec!["90".into()]);
                let mut agent = AgentDefinition::new(
                    AgentId::new(),
                    "受控等待进程",
                    AgentSource::LocalConfiguration,
                );
                agent.launch = Some(launch);
                service
                    .execute(command(
                        Scope::default(),
                        Command::PutAgent {
                            agent: agent.clone(),
                        },
                    ))
                    .await
                    .unwrap();
                let missing = tempfile::tempdir().unwrap();
                let missing_project = service
                    .open_project(missing.path().to_path_buf(), "消失的目录".into())
                    .await
                    .unwrap();
                drop(missing);
                service
                    .select_project(Some(missing_project.id))
                    .await
                    .unwrap();
                assert!(
                    service
                        .snapshot()
                        .project_directories
                        .iter()
                        .find(|p| p.project_id == missing_project.id)
                        .unwrap()
                        .error
                        .is_some()
                );
                assert_eq!(
                    service
                        .create_session(missing_project.id, agent.id, "未创建".into())
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::InvalidInput
                );
                service.remove_project(missing_project.id).await.unwrap();
                let offline = SessionRecord::new(SessionId::new(), a.id, agent.id, "未连接", now());
                let other = SessionRecord::new(SessionId::new(), b.id, agent.id, "B 草稿", now());
                for record in [&offline, &other] {
                    service
                        .execute(command(
                            Scope::default(),
                            Command::CreateSession {
                                session: record.clone(),
                            },
                        ))
                        .await
                        .unwrap();
                }
                let mut b_draft = ComposerDraft::new(other.id(), now());
                b_draft.revision = 1;
                b_draft.content = vec![InputPart::Text {
                    text: "B 未发送内容".into(),
                }];
                service
                    .execute(command(
                        Scope::session(other.id(), ConnectionEpoch::INITIAL),
                        Command::SaveDraft {
                            draft: b_draft.clone(),
                            expected_revision: 0,
                        },
                    ))
                    .await
                    .unwrap();
                let connected = service
                    .create_session(a.id, agent.id, "A 已提交创建".into())
                    .await
                    .unwrap();
                let initial = session(&service, connected).await;
                assert!(initial.record.external.is_none());
                assert!(!initial.resources_released);
                let mut a_draft = initial.draft.clone();
                a_draft.revision = 1;
                a_draft.content = vec![InputPart::FileReference {
                    reference: FileReference::new(FileReferenceId::new(), a.id, old_path.clone()),
                }];
                service
                    .execute(command(
                        Scope::session(connected, initial.record.connection.epoch),
                        Command::SaveDraft {
                            draft: a_draft.clone(),
                            expected_revision: 0,
                        },
                    ))
                    .await
                    .unwrap();
                let stale = service.snapshot();
                service.select_project(Some(b.id)).await.unwrap();
                assert!(service.snapshot().active_session.is_none());
                assert_eq!(service.snapshot().preferences.selected_session_id, None);
                assert_eq!(
                    service.snapshot().preferences.selected_project_id,
                    Some(b.id)
                );
                assert_eq!(
                    service
                        .save_preferences(stale.preferences_revision, stale.preferences.clone())
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::Conflict
                );
                service.select_session(Some(other.id())).await.unwrap();
                assert_eq!(
                    service.snapshot().preferences.selected_project_id,
                    Some(b.id)
                );
                assert_eq!(
                    service.snapshot().active_session.as_ref().unwrap().draft,
                    b_draft
                );
                assert_eq!(
                    service
                        .relocate_project(a.id, new_dir.path().to_path_buf())
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::Busy
                );
                assert_eq!(
                    service
                        .snapshot()
                        .workbench
                        .projects
                        .iter()
                        .find(|p| p.id == a.id)
                        .unwrap()
                        .directory,
                    a.directory
                );

                // Close through the same public execute path; only the real Runtime release fact
                // permits relocation. The original reference path is never rebased to the new cwd.
                let close = command(
                    Scope::session(connected, initial.record.connection.epoch),
                    Command::CloseSession,
                );
                let close_id = close.id;
                service.execute(close.clone()).await.unwrap();
                released(&service, connected).await;
                assert_eq!(service.execute(close).await.unwrap().command_id, close_id);
                assert_eq!(
                    service
                        .relocate_project(a.id, b_dir.path().to_path_buf())
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::Conflict
                );
                let relocated = service
                    .relocate_project(a.id, new_dir.path().to_path_buf())
                    .await
                    .unwrap();
                assert_eq!(relocated.id, a.id);
                assert_eq!(session(&service, connected).await.draft, a_draft);
                assert_eq!(std::fs::read_to_string(&old_path).unwrap(), "保留项目文件");
                service.remove_session(offline.id()).await.unwrap();
                assert!(
                    service
                        .query_session(offline.id())
                        .await
                        .unwrap()
                        .snapshot
                        .is_none()
                );
                let epoch = session(&service, connected).await.record.connection.epoch;
                service
                    .execute(command(
                        Scope::session(connected, epoch),
                        Command::Connect { resume: false },
                    ))
                    .await
                    .unwrap();
                service.select_session(Some(other.id())).await.unwrap();
                let deleting = service.remove_session(connected);
                let waiting = session(&service, connected).await;
                assert!(waiting.closing);
                assert!(!waiting.resources_released);
                assert!(
                    service
                        .snapshot()
                        .pending_removals
                        .iter()
                        .any(|p| p.target == RemovalTarget::Session(connected))
                );
                assert_eq!(
                    service
                        .execute(command(
                            Scope::session(connected, waiting.record.connection.epoch),
                            Command::Connect { resume: false }
                        ))
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::Busy
                );
                assert_eq!(session(&service, other.id()).await.draft, b_draft);
                deleting.await.unwrap();
                assert!(
                    service
                        .query_session(connected)
                        .await
                        .unwrap()
                        .snapshot
                        .is_none()
                );
                assert_eq!(service.snapshot().selection.session_id, Some(other.id()));

                // A failure AFTER durable CreateSession must still return its local identity.
                let (reply, done) = oneshot::channel();
                service
                    .sender
                    .try_send(Message::FailNextPreferencesWrite { reply })
                    .ok()
                    .unwrap();
                done.await.unwrap().unwrap();
                let created = service
                    .create_session(a.id, agent.id, "偏好失败仍已创建".into())
                    .await
                    .unwrap();
                assert!(
                    service
                        .snapshot()
                        .workbench
                        .sessions
                        .iter()
                        .any(|s| s.id == created)
                );
                assert_eq!(
                    service.snapshot().last_error.as_ref().unwrap().kind,
                    ErrorKind::Storage
                );
                assert_eq!(service.snapshot().selection.session_id, Some(other.id()));
                let deletion = service.remove_project(a.id);
                service.query_session(created).await.unwrap();
                assert_eq!(
                    service
                        .create_session(a.id, agent.id, "禁止删除中重建".into())
                        .await
                        .unwrap_err()
                        .kind,
                    ErrorKind::Busy
                );
                deletion.await.unwrap();
                assert!(
                    !service
                        .snapshot()
                        .workbench
                        .projects
                        .iter()
                        .any(|p| p.id == a.id)
                );
                assert!(new_dir.path().is_dir());
                assert!(old_path.is_file());
                assert!(b_file.is_file());

                // Simulate the persisted Restart boundary while an old epoch still has an owned
                // process. A current-epoch close cannot claim that old process was released.
                let unknown = service
                    .create_session(b.id, agent.id, "未知旧连接".into())
                    .await
                    .unwrap();
                service
                    .execute(command(Scope::default(), Command::Restart))
                    .await
                    .unwrap();
                let failed = service.remove_session(unknown);
                service.select_session(Some(other.id())).await.unwrap();
                assert_eq!(
                    session(&service, other.id()).await.draft.content,
                    b_draft.content
                );
                let error = failed.await.unwrap_err();
                assert_eq!(error.retry, RetryAdvice::ReconcileFirst);
                assert!(
                    service
                        .query_session(unknown)
                        .await
                        .unwrap()
                        .snapshot
                        .is_some()
                );
                assert!(service.snapshot().pending_removals.is_empty());
                service.shutdown().await.unwrap();
                drop(service);
                let reopened = ApplicationService::start(options).await.unwrap();
                assert_eq!(reopened.snapshot().workbench.projects.len(), 1);
                assert_eq!(
                    reopened.snapshot().preferences.selected_project_id,
                    Some(b.id)
                );
                assert_eq!(
                    reopened
                        .snapshot()
                        .active_session
                        .as_ref()
                        .unwrap()
                        .draft
                        .content,
                    b_draft.content
                );
                assert!(session(&reopened, unknown).await.closing_handoff.is_some());
                assert_eq!(
                    reopened.remove_session(unknown).await.unwrap_err().retry,
                    RetryAdvice::ReconcileFirst
                );
                assert_eq!(
                    reopened
                        .create_session(b.id, agent.id, "不会凭空成功".into())
                        .await
                        .is_ok(),
                    true
                );
                reopened.shutdown().await.unwrap();
                assert!(old_path.is_file());
                assert!(b_file.is_file());
            })
            .await
            .expect("bounded navigation scenario timed out");
        });
}
