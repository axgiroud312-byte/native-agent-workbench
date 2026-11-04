use crate::{theme, workbench::Workbench};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, input::Textarea, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use workbench_core::{InputPart, PermissionResource, QueuedInputState};

impl Workbench {
    pub(crate) fn render_composer_region(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.host.active_session();
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
        v_flex().id("composer-region").w_full().flex_shrink_0().items_center().px_6().pb_4().pt_2()
            .child(v_flex().w_full().max_w(theme::conversation_max_width()).gap_2()
                .when_some(session, |col, session| {
                    let has_pending = session.permissions.iter().any(|p| p.is_actionable)
                        || session.questions.iter().any(|q| q.is_actionable);
                    col.when(has_pending, |col| col.child(v_flex().id("pending-requests")
                        .max_h(rems(96.0 / theme::BASE_FONT)).overflow_y_scroll().gap_2()
                        .children(session.permissions.iter().filter(|p| p.is_actionable).map(|p| {
                        v_flex().gap_1().px_3().py_2().rounded(theme::panel_radius()).bg(theme::surface_bg(cx))
                            .child(h_flex().gap_2().child(Icon::new(IconName::TriangleAlert).small())
                                .child(p.request.operation.clone()))
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
                    .children(session.questions.iter().filter(|q| q.is_actionable).flat_map(|q| q.fields.iter()).map(|f| {
                        div().text_sm().child(f.prompt.clone())
                    }))))
                })
                .when(queue > 0, |col| col.child(div().text_xs().text_color(theme::text_secondary(cx)).child(format!("排队 {queue}"))))
                .child(v_flex().id("composer-surface").w_full().rounded(theme::composer_radius())
                    .bg(theme::surface_bg(cx)).border_1().border_color(theme::subtle_border(cx))
                    .child(div().px_2().pt_2().child(Textarea::new(&self.composer_input)
                        .appearance(false).bordered(false).disabled(session.is_none()).aria_label("对话草稿")))
                    .child(h_flex().px_3().py_2().min_w_0().gap_3()
                        .when_some(session, |row, session| {
                            let effective = session.record.configuration.effective.as_ref();
                            let model = effective.and_then(|e| e.model.as_ref()).map(|id| {
                                session.record.capabilities.as_ref().and_then(|c| c.models.iter().find(|m| m.option.id == *id))
                                    .map(|m| m.option.label.clone()).unwrap_or_else(|| id.to_string())
                            });
                            row.when_some(model, |r, label| r.child(div().text_xs().text_color(theme::text_secondary(cx)).child(label)))
                                .when_some(effective.and_then(|e| e.reasoning_effort.as_ref()), |r, effort| r.child(div().text_xs().child(effort.to_string())))
                        })
                        .when(session.is_none(), |row| row.child(div().text_xs().text_color(theme::text_secondary(cx)).child("未连接会话")))
                    )
                )
                .when_some(session, |col, session| col.children(session.draft.content.iter().filter_map(|p| {
                    match p {
                        InputPart::Attachment { attachment_id } => session.attachments.iter().find(|a| a.id == *attachment_id).map(|a| a.name.clone()),
                        InputPart::FileReference { reference } => Some(format!("@{}", reference.path.display())),
                        _ => None,
                    }
                }).map(|name| div().text_xs().text_color(theme::text_secondary(cx)).child(name))))
            )
    }
}
