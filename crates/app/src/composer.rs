use crate::{
    theme,
    workbench::{StopRun, SubmitDraft, Workbench},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Textarea,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use workbench_core::{
    ExecutionState, InputPart, PermissionResource, QueuedInputState, Run, SubmissionState,
};

impl Workbench {
    pub(crate) fn active_run(&self) -> Option<&Run> {
        let session = self.host.active_session()?;
        let id = session.record.current_run_id?;
        session.runs.iter().find(|run| {
            run.context.run_id == id
                && matches!(
                    run.state,
                    ExecutionState::Starting
                        | ExecutionState::Running
                        | ExecutionState::Cancelling { .. }
                )
        })
    }

    pub(crate) fn capture_composer_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "enter" {
            let composing = self.composer_input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            });
            // Observe without consuming native IME confirmation or Shift+Enter.
            self.enter_suppressed = event.is_held || composing;
        }
    }

    pub(crate) fn submit_draft(
        &mut self,
        _: &SubmitDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_fixture() || self.active_run().is_some() {
            return;
        }
        let Some(session) = self.host.active_session() else {
            return;
        };
        if !session.can_prepare_prompt {
            return;
        }
        let id = session.record.id();
        let (composing, text) = self.composer_input.update(cx, |input, cx| {
            (
                input.marked_text_range(window, cx).is_some(),
                input.value().to_string(),
            )
        });
        if composing || text.trim().is_empty() {
            return;
        }
        self.application
            .update(cx, |app, cx| app.send_text(id, text, cx));
    }

    pub(crate) fn stop_run(&mut self, _: &StopRun, _: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_fixture() {
            return;
        }
        let Some(run) = self.active_run() else {
            return;
        };
        if matches!(run.state, ExecutionState::Cancelling { .. }) {
            return;
        }
        let id = run.context.session_id;
        let run_id = run.context.run_id;
        let epoch = self.host.active_session().unwrap().record.connection.epoch;
        self.application
            .update(cx, |app, cx| app.stop_run(id, run_id, epoch, cx));
    }

    pub(crate) fn render_composer_region(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.host.active_session();
        let app = self.application.read(cx);
        let run = self.active_run();
        let queue = session
            .map(|s| {
                s.queue
                    .entries
                    .iter()
                    .filter(|q| {
                        matches!(
                            q.state,
                            QueuedInputState::Waiting
                                | QueuedInputState::Editing
                                | QueuedInputState::Dispatching { .. }
                        )
                    })
                    .count()
            })
            .unwrap_or(0);
        let sending = session.is_some_and(|s| app.sending(s.record.id()));
        let cancelling =
            run.is_some_and(|run| matches!(run.state, ExecutionState::Cancelling { .. }));
        let stop_pending =
            run.is_some_and(|run| app.stopping(run.context.session_id, run.context.run_id));
        let has_text = !self.composer_input.read(cx).value().trim().is_empty();
        let can_send = session.is_some_and(|s| s.can_prepare_prompt)
            && has_text
            && !sending
            && app.handle.is_some()
            && !self.host.is_fixture();
        let error = session.and_then(|s| app.send_error(s.record.id()).map(str::to_owned));
        let waiting = session.is_some_and(|s| {
            s.submissions.iter().any(|submission| {
                matches!(
                    submission.state,
                    SubmissionState::AwaitingAcknowledgement { .. }
                )
            })
        });
        let outcome_unknown = session.is_some_and(|s| {
            s.submissions.iter().any(|submission| {
                matches!(submission.state, SubmissionState::OutcomeUnknown { .. })
            })
        });
        let primary = if run.is_some() {
            Button::new("composer-stop")
                .outline()
                .small()
                .icon(IconName::Square)
                .label(if cancelling {
                    "正在停止…"
                } else if stop_pending {
                    "请求停止中…"
                } else {
                    "停止"
                })
                .disabled(cancelling || stop_pending || self.host.is_fixture())
                .on_click(cx.listener(|this, _, window, cx| this.stop_run(&StopRun, window, cx)))
                .into_any_element()
        } else {
            Button::new("composer-send")
                .primary()
                .small()
                .icon(IconName::ArrowUp)
                .label(if sending { "发送中…" } else { "发送" })
                .disabled(!can_send)
                .on_click(
                    cx.listener(|this, _, window, cx| this.submit_draft(&SubmitDraft, window, cx)),
                )
                .into_any_element()
        };
        v_flex().id("composer-region").w_full().flex_shrink_0().items_center().px_6().pb_4().pt_2()
            .child(v_flex().w_full().max_w(theme::conversation_max_width()).gap_2()
                .when_some(session, |col, session| {
                    let has_pending = session.permissions.iter().any(|p| p.is_actionable) || session.questions.iter().any(|q| q.is_actionable);
                    col.when(has_pending, |col| col.child(v_flex().id("pending-requests")
                        .max_h(rems(96.0 / theme::BASE_FONT)).overflow_y_scroll().gap_2()
                        .children(session.permissions.iter().filter(|p| p.is_actionable).map(|p| {
                            v_flex().gap_1().px_3().py_2().rounded(theme::panel_radius()).bg(theme::surface_bg(cx))
                                .child(h_flex().gap_2().child(Icon::new(IconName::TriangleAlert).small()).child(p.request.operation.clone()))
                                .when_some(p.request.scope.description.clone(), |row, description| row.child(description))
                                .children(p.request.scope.resources.iter().map(|resource| {
                                    let text = match resource {
                                        PermissionResource::File { path } => path.display().to_string(),
                                        PermissionResource::Command { command, working_directory } => working_directory.as_ref()
                                            .map(|path| format!("{command}\n{}", path.display())).unwrap_or_else(|| command.clone()),
                                        PermissionResource::Uri { uri } => uri.clone(),
                                        PermissionResource::AgentReported { description } => description.clone(),
                                    };
                                    div().text_sm().child(text)
                                }))
                        }))
                        .children(session.questions.iter().filter(|q| q.is_actionable).flat_map(|q| q.fields.iter()).map(|f|
                            div().text_sm().child(f.prompt.clone())))))
                })
                .when(queue > 0, |col| col.child(div().text_xs().text_color(theme::text_secondary(cx)).child(format!("排队 {queue}"))))
                .when_some(error, |col, error| col.child(div().text_sm().text_color(cx.theme().danger).child(error)))
                .when(waiting, |col| col.child(div().text_xs().text_color(theme::text_secondary(cx)).child("等待 Agent 接收")))
                .when(outcome_unknown, |col| col.child(div().text_sm().text_color(cx.theme().warning).child("发送结果待核对，输入已保留。")))
                .child(v_flex().id("composer-surface").w_full().rounded(theme::composer_radius()).bg(theme::surface_bg(cx))
                    .border_1().border_color(theme::subtle_border(cx))
                    .child(div().px_2().pt_2().capture_key_down(cx.listener(Self::capture_composer_key))
                        .child(Textarea::new(&self.composer_input).appearance(false).bordered(false).disabled(session.is_none()).aria_label("对话草稿")))
                    .child(h_flex().px_3().py_2().min_w_0().gap_3()
                        .child(h_flex().flex_1().min_w_0().gap_3().text_xs().text_color(theme::text_secondary(cx))
                            .when_some(session, |row, session| {
                                let effective = session.record.configuration.effective.as_ref().filter(|e| e.epoch == session.record.connection.epoch);
                                let model = effective.and_then(|e| e.model.as_ref()).map(|id| {
                                    session.record.capabilities.as_ref().and_then(|c| c.models.iter().find(|m| m.option.id == *id))
                                        .map(|m| m.option.label.clone()).unwrap_or_else(|| id.to_string())
                                });
                                let permission = effective.and_then(|e| e.permission.as_ref()).map(|id| {
                                    session.record.capabilities.as_ref().and_then(|c| c.permission_modes.iter().find(|p| p.option.id == *id))
                                        .map(|p| p.option.label.clone()).unwrap_or_else(|| id.to_string())
                                });
                                row.when_some(model, |row, label| row.child(div().min_w_0().truncate().child(label)))
                                    .when_some(effective.and_then(|e| e.reasoning_effort.as_ref()), |row, value| row.child(value.to_string()))
                                    .when_some(permission, |row, value| row.child(div().min_w_0().truncate().child(value)))
                                    .when(effective.is_none(), |row| row.child("配置未确认"))
                            })
                            .when(session.is_none(), |row| row.child("未连接会话")))
                        .child(primary)))
                .when_some(session, |col, session| col.children(session.draft.content.iter().filter_map(|part| match part {
                    InputPart::Attachment { attachment_id } => session.attachments.iter().find(|a| a.id == *attachment_id).map(|a| a.name.clone()),
                    InputPart::FileReference { reference } => Some(format!("@{}", reference.path.display())), _ => None,
                }).map(|name| div().text_xs().text_color(theme::text_secondary(cx)).child(name)))))
    }
}
