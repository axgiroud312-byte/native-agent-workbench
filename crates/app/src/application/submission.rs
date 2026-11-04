use super::{ApplicationState, DraftEdit};
use crate::host::{draft_plain_text, now};
use gpui_kit::Context;
use workbench_core::*;

#[derive(Clone)]
pub(super) struct PendingSend {
    text: String,
    edit_version: u64,
    epoch: ConnectionEpoch,
    effective: EffectiveConfiguration,
    configuration: RequestedConfiguration,
    command_id: CommandId,
    submission_id: SubmissionId,
    attempt_id: AttemptId,
    run_id: RunId,
    request_id: RequestId,
    in_flight: bool,
}

impl ApplicationState {
    pub(super) fn record_edit(&mut self, id: SessionId, text: String) -> Option<u64> {
        if let Some(edit) = self.draft_edits.get(&id).filter(|edit| edit.text == text) {
            return Some(edit.version);
        }
        let Some(version) = self.edit_clock.checked_add(1) else {
            self.draft_failures
                .insert(id, "编辑版本已耗尽，请保存内容后重新打开应用。".into());
            return None;
        };
        self.edit_clock = version;
        self.draft_edits.insert(id, DraftEdit { text, version });
        Some(version)
    }

    pub(crate) fn sending(&self, id: SessionId) -> bool {
        self.pending_sends.contains_key(&id)
    }
    pub(crate) fn stopping(&self, id: SessionId, run: RunId) -> bool {
        self.stopping_requests.get(&id) == Some(&run)
    }
    pub(crate) fn send_error(&self, id: SessionId) -> Option<&str> {
        self.send_errors.get(&id).map(String::as_str)
    }

    pub(crate) fn send_text(&mut self, id: SessionId, text: String, cx: &mut Context<Self>) {
        if self.quit_requested || self.pending_sends.contains_key(&id) || text.trim().is_empty() {
            return;
        }
        let Some(handle) = self.handle.as_ref() else {
            return;
        };
        let frame = handle.snapshot();
        let Some(session) = frame
            .active_session
            .as_ref()
            .filter(|session| session.record.id() == id)
        else {
            return;
        };
        if !session.can_prepare_prompt {
            return;
        }
        let Some(effective) = session.record.configuration.effective.clone() else {
            return;
        };
        let mut configuration = session.draft.configuration.clone();
        configuration.model = effective.model.clone();
        configuration.reasoning_effort = effective.reasoning_effort.clone();
        if let Some(permission) = &effective.permission {
            configuration.permission = PermissionSelection::AgentOption {
                id: permission.clone(),
            };
        }
        let Some(edit_version) = self.record_edit(id, text.clone()) else {
            cx.notify();
            return;
        };
        self.pending_sends.insert(
            id,
            PendingSend {
                text,
                edit_version,
                epoch: session.record.connection.epoch,
                effective,
                configuration,
                command_id: CommandId::new(),
                submission_id: SubmissionId::new(),
                attempt_id: AttemptId::new(),
                run_id: RunId::new(),
                request_id: RequestId::new(),
                in_flight: false,
            },
        );
        self.send_errors.remove(&id);
        self.draft_failures.remove(&id);
        self.flush_draft(id, cx);
        cx.notify();
    }

    pub(super) fn begin_send(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if self.drafts_in_flight.contains(&id) {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let Some(intent) = self
            .pending_sends
            .get_mut(&id)
            .filter(|intent| !intent.in_flight)
        else {
            return;
        };
        intent.in_flight = true;
        let intent = intent.clone();
        let query = handle.query_session(id);
        cx.spawn(async move |this, cx| {
            let result = async {
                // Selection generation is intentionally not consulted: this accepted
                // intent still belongs to A after the user navigates to B.
                let reply = query.await?;
                let session = checked_target(reply.session_id, reply.snapshot, id, &intent)?;
                let mut draft = session.draft.clone();
                draft
                    .content
                    .retain(|part| !matches!(part, InputPart::Text { .. }));
                draft.content.insert(
                    0,
                    InputPart::Text {
                        text: intent.text.clone(),
                    },
                );
                draft.configuration = intent.configuration.clone();
                if draft.content != session.draft.content
                    || draft.configuration != session.draft.configuration
                {
                    let expected_revision = session.draft.revision;
                    draft.revision = expected_revision
                        .checked_add(1)
                        .ok_or_else(|| unavailable("草稿版本已耗尽"))?;
                    draft.updated_at = now();
                    let saved = handle
                        .execute(CommandEnvelope::new(
                            CommandId::new(),
                            now(),
                            Scope::session(id, intent.epoch),
                            Command::SaveDraft {
                                draft,
                                expected_revision,
                            },
                        ))
                        .await?;
                    check_receipt(saved.result)?;
                }
                let reply = handle.query_session(id).await?;
                let session = checked_target(reply.session_id, reply.snapshot, id, &intent)?;
                if !session.can_start_prompt
                    || draft_plain_text(&session.draft) != intent.text
                    || session.draft.configuration != intent.configuration
                {
                    return Err(unavailable("发送前状态或草稿已变化，输入已保留。"));
                }
                let submission = InputSubmission::from_draft(
                    intent.submission_id,
                    &session.draft,
                    SubmissionIntent::StartRun {
                        run_id: intent.run_id,
                    },
                    now(),
                );
                let command = CommandEnvelope::new(
                    intent.command_id,
                    now(),
                    Scope::session(id, intent.epoch)
                        .with_run(intent.run_id)
                        .with_request(intent.request_id),
                    Command::Submit {
                        submission,
                        attempt_id: intent.attempt_id,
                    },
                );
                let receipt = handle.execute(command).await?;
                check_receipt(receipt.result)
            }
            .await;
            let _ = this.update(cx, |state, cx| {
                state.pending_sends.remove(&id);
                let newer_edit = state
                    .draft_edits
                    .get(&id)
                    .is_some_and(|edit| edit.version > intent.edit_version);
                match result {
                    Ok(()) => {
                        // This only discards the already persisted local editing buffer.
                        // The visible text follows core's exact Accepted draft clearing.
                        if !newer_edit {
                            state.draft_edits.remove(&id);
                        }
                        state.send_errors.remove(&id);
                    }
                    Err(error) => {
                        if !newer_edit {
                            state.draft_edits.insert(
                                id,
                                DraftEdit {
                                    text: intent.text.clone(),
                                    version: intent.edit_version,
                                },
                            );
                        }
                        state.send_errors.insert(id, error.message);
                    }
                }
                state.publish(handle.snapshot(), cx);
                // Later edits are a different draft even when the user retyped the same text.
                if newer_edit {
                    state.flush_draft(id, cx);
                }
                state.maybe_shutdown(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn stop_run(
        &mut self,
        id: SessionId,
        run_id: RunId,
        epoch: ConnectionEpoch,
        cx: &mut Context<Self>,
    ) {
        if self.quit_requested || self.stopping_requests.contains_key(&id) {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        self.stopping_requests.insert(id, run_id);
        self.send_errors.remove(&id);
        let future = handle.execute(CommandEnvelope::new(
            CommandId::new(),
            now(),
            Scope::session(id, epoch)
                .with_run(run_id)
                .with_request(RequestId::new()),
            Command::Stop,
        ));
        cx.spawn(async move |this, cx| {
            let result = future
                .await
                .and_then(|receipt| check_receipt(receipt.result));
            let _ = this.update(cx, |state, cx| {
                state.stopping_requests.remove(&id);
                state.publish(handle.snapshot(), cx);
                if let Err(error) = result {
                    state.send_errors.insert(id, error.message);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn checked_target(
    reply_id: SessionId,
    snapshot: Option<SessionSnapshot>,
    id: SessionId,
    intent: &PendingSend,
) -> Result<SessionSnapshot, BusinessError> {
    let session = snapshot.ok_or_else(|| unavailable("目标会话已不存在，输入已保留。"))?;
    if reply_id != id
        || session.record.id() != id
        || session.record.connection.epoch != intent.epoch
    {
        return Err(unavailable("目标会话连接已变化，输入已保留。"));
    }
    if session.record.configuration.effective.as_ref() != Some(&intent.effective) {
        return Err(unavailable("Agent 的确认配置已变化，输入已保留。"));
    }
    Ok(session)
}
fn check_receipt(status: CommandStatus) -> Result<(), BusinessError> {
    match status {
        CommandStatus::Applied => Ok(()),
        CommandStatus::Rejected { error } => Err(error),
    }
}
fn unavailable(message: &str) -> BusinessError {
    BusinessError::new(ErrorKind::Conflict, message, RetryAdvice::AfterCorrection)
}
