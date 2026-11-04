use crate::{
    theme,
    workbench::{NewSession, OpenProject, Workbench},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    message_scroller::MessageScroller,
    text::TextView,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use workbench_core::{
    BlockSnapshot, ConnectionPhase, ContentPresentation, ExecutionState, MessageRole,
    MessageSnapshot, MessageState, SessionSnapshot, ToolSnapshot, ToolState,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationRowId {
    Message(workbench_core::MessageId),
    Tool(workbench_core::ToolCallId),
}

#[derive(Clone)]
pub(crate) enum ConversationRow {
    Message(Entity<MessageView>),
    Tool(std::sync::Arc<ToolSnapshot>),
}

#[derive(Default)]
struct MessageContext {
    attachments: Vec<workbench_core::Attachment>,
    plans: Vec<workbench_core::Plan>,
    changes: Vec<workbench_core::FileChange>,
    tool_names: Vec<(workbench_core::ToolCallId, String)>,
}

pub(crate) struct MessageView {
    message: MessageSnapshot,
    context: std::sync::Arc<MessageContext>,
}
impl Render for MessageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        render_message_row(&self.message, &self.context, cx).into_any_element()
    }
}

#[derive(Default)]
pub(crate) struct ConversationCache {
    order: Vec<ConversationRowId>,
    messages: std::collections::BTreeMap<workbench_core::MessageId, Entity<MessageView>>,
    tools: std::collections::BTreeMap<workbench_core::ToolCallId, std::sync::Arc<ToolSnapshot>>,
    context: std::sync::Arc<MessageContext>,
}
impl ConversationCache {
    pub(crate) fn rows(&self) -> Vec<ConversationRow> {
        self.order
            .iter()
            .filter_map(|id| match id {
                ConversationRowId::Message(id) => {
                    self.messages.get(id).cloned().map(ConversationRow::Message)
                }
                ConversationRowId::Tool(id) => {
                    self.tools.get(id).cloned().map(ConversationRow::Tool)
                }
            })
            .collect()
    }
    pub(crate) fn sync(
        &mut self,
        session: &SessionSnapshot,
        scroller: &Entity<gpui_kit::component::message_scroller::MessageScrollerState>,
        cx: &mut Context<Workbench>,
    ) {
        let names: Vec<_> = session
            .tools
            .iter()
            .map(|tool| (tool.id, tool.name.clone()))
            .collect();
        let context_changed = self.context.attachments != session.attachments
            || self.context.plans != session.plans
            || self.context.changes != session.changes
            || self.context.tool_names != names;
        if context_changed {
            self.context = std::sync::Arc::new(MessageContext {
                attachments: session.attachments.clone(),
                plans: session.plans.clone(),
                changes: session.changes.clone(),
                tool_names: names,
            });
        }
        let next: Vec<_> = session
            .messages
            .iter()
            .map(|m| ConversationRowId::Message(m.id))
            .chain(session.tools.iter().map(|t| ConversationRowId::Tool(t.id)))
            .collect();
        let prefix = self
            .order
            .iter()
            .zip(&next)
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = self.order[prefix..]
            .iter()
            .rev()
            .zip(next[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if prefix != self.order.len() || prefix != next.len() {
            let old_end = self.order.len() - suffix;
            let count = next.len() - prefix - suffix;
            scroller.update(cx, |scroller, cx| {
                scroller.splice(prefix..old_end, count, cx);
            });
        }
        let mut changed = Vec::new();
        for (ix, message) in session.messages.iter().enumerate() {
            if let Some(view) = self.messages.get(&message.id) {
                let has_references = message
                    .blocks
                    .iter()
                    .any(|b| !matches!(b.content, ContentPresentation::Text { .. }));
                if view.read(cx).message != *message || context_changed && has_references {
                    let context = self.context.clone();
                    view.update(cx, |view, cx| {
                        view.message = message.clone();
                        view.context = context;
                        cx.notify();
                    });
                    changed.push(ix);
                }
            } else {
                let context = self.context.clone();
                let message = message.clone();
                self.messages
                    .insert(message.id, cx.new(|_| MessageView { message, context }));
            }
        }
        for (ix, tool) in session.tools.iter().enumerate() {
            if self
                .tools
                .get(&tool.id)
                .is_none_or(|old| old.as_ref() != tool)
            {
                self.tools
                    .insert(tool.id, std::sync::Arc::new(tool.clone()));
                changed.push(session.messages.len() + ix);
            }
        }
        self.messages
            .retain(|id, _| session.messages.iter().any(|m| m.id == *id));
        self.tools
            .retain(|id, _| session.tools.iter().any(|t| t.id == *id));
        self.order = next;
        if !changed.is_empty() {
            scroller.update(cx, |scroller, cx| {
                for ix in changed {
                    scroller.remeasure_items(ix..ix + 1, cx);
                }
            });
        }
    }
}

impl Workbench {
    pub(crate) fn render_workspace_header(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.host.active_session();
        h_flex()
            .id("workspace-header")
            .h(theme::header_height())
            .px_6()
            .gap_3()
            .flex_shrink_0()
            .min_w_0()
            .when_some(session, |row, session| {
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(session.record.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_secondary(cx))
                        .child(self.host.agent_name(session.record.agent_id())),
                )
                .when_some(
                    workspace_header_status(session, cx),
                    |row, (label, color, _)| {
                        row.child(div().text_xs().text_color(color).child(label))
                    },
                )
            })
    }

    pub(crate) fn render_conversation_region(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(session) = self.host.active_session() else {
            let has_directory = self.host.selected_project().is_some();
            return v_flex()
                .id("conversation-empty")
                .flex_1()
                .min_h_0()
                .min_w_0()
                .items_center()
                .justify_center()
                .p_6()
                .gap_3()
                .when(!has_directory, |col| {
                    col.child(
                        Button::new("workspace-open-project")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("打开项目…")
                            .disabled(
                                self.picker_open
                                    || self.host.is_fixture()
                                    || !self.service_ready(cx),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_project(&OpenProject, window, cx)
                            })),
                    )
                })
                .when(has_directory, |col| {
                    col.child(
                        Button::new("empty-new-session")
                            .outline()
                            .icon(IconName::Plus)
                            .label("新建会话…")
                            .disabled(
                                self.host.is_fixture()
                                    || !self.service_ready(cx)
                                    || self.host.preferences().selected_project_id.is_some_and(
                                        |id| {
                                            self.application.read(cx).removing(
                                                workbench_service::RemovalTarget::Project(id),
                                            )
                                        },
                                    ),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_session(&NewSession, window, cx)
                            })),
                    )
                })
                .when_some(self.picker_error.clone(), |col, error| {
                    col.child(div().text_sm().text_color(cx.theme().danger).child(error))
                })
                .when(self.application.read(cx).starting, |col| {
                    col.child(div().text_sm().child("正在打开工作区…"))
                })
                .when_some(self.service_error(cx), |col, error| {
                    col.child(
                        v_flex()
                            .gap_2()
                            .child(div().text_sm().text_color(cx.theme().danger).child(error))
                            .when(!self.service_ready(cx), |col| {
                                col.child(
                                    Button::new("retry-workspace")
                                        .outline()
                                        .label("重试")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.application.update(cx, |app, cx| app.start(cx))
                                        })),
                                )
                            }),
                    )
                })
                .into_any_element();
        };
        let banner = match &session.record.connection.phase {
            ConnectionPhase::Failed { reason } | ConnectionPhase::Disconnected { reason } => {
                Some(reason.clone())
            }
            _ => self
                .host
                .last_error()
                .or_else(|| session.errors.last())
                .map(|e| e.message.clone()),
        };
        let draft_error = self
            .application
            .read(cx)
            .draft_error(session.record.id())
            .map(str::to_owned);
        let rows = self
            .conversations
            .get(&session.record.id())
            .map(ConversationCache::rows)
            .unwrap_or_default();
        let expanded = self.host.preferences().expanded_tools.clone();
        let entity = cx.entity();
        v_flex()
            .id("conversation-region")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .when_some(draft_error, |col, reason| {
                col.child(
                    div()
                        .px_6()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(reason),
                )
            })
            .when_some(banner, |col, reason| {
                col.child(
                    div()
                        .px_6()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(reason),
                )
            })
            .child(
                div().flex_1().min_h_0().min_w_0().w_full().child(
                    MessageScroller::new(
                        ElementId::Name(format!("session-scroller:{}", session.record.id()).into()),
                        self.message_scroller.clone(),
                        move |ix, _, cx| match rows.get(ix) {
                            Some(ConversationRow::Message(message)) => {
                                message.clone().into_any_element()
                            }
                            Some(ConversationRow::Tool(tool)) => render_tool_row(
                                tool,
                                expanded.contains(&tool.id),
                                entity.clone(),
                                cx,
                            )
                            .into_any_element(),
                            None => div().into_any_element(),
                        },
                    )
                    .with_jump_button_label("回到最新")
                    .with_bottom_fade(theme::workspace_bg(cx)),
                ),
            )
            .into_any_element()
    }
}

fn workspace_header_status(
    session: &SessionSnapshot,
    cx: &App,
) -> Option<(SharedString, Hsla, bool)> {
    match &session.record.connection.phase {
        ConnectionPhase::NotStarted => {
            return Some(("未连接".into(), theme::text_muted(cx), false));
        }
        ConnectionPhase::Starting => {
            return Some(("连接中".into(), theme::text_secondary(cx), true));
        }
        ConnectionPhase::Authenticating => {
            return Some(("认证中".into(), theme::text_secondary(cx), true));
        }
        ConnectionPhase::Failed { .. } => {
            return Some(("连接失败".into(), cx.theme().danger, false));
        }
        ConnectionPhase::Disconnected { .. } => {
            return Some(("已断开".into(), cx.theme().warning, false));
        }
        ConnectionPhase::Closed => return Some(("已关闭".into(), theme::text_muted(cx), false)),
        ConnectionPhase::Ready => {}
    }

    if let Some(run_id) = session.record.current_run_id
        && let Some(run) = session.runs.iter().find(|r| r.context.run_id == run_id)
    {
        match &run.state {
            ExecutionState::Starting | ExecutionState::Running => {
                return Some(("运行中".into(), cx.theme().info, true));
            }
            ExecutionState::Cancelling { .. } => {
                return Some(("正在停止".into(), cx.theme().warning, true));
            }
            _ => {}
        }
    }
    None
}

fn render_message_row(
    message: &MessageSnapshot,
    context: &MessageContext,
    cx: &App,
) -> impl IntoElement {
    let is_user = message.role == MessageRole::User;
    let is_streaming = matches!(message.state, MessageState::Streaming);

    v_flex().w_full().items_center().px_3().child(
        v_flex()
            .w_full()
            .max_w(theme::conversation_max_width())
            .gap_2()
            .when(is_user, |lane| {
                lane.items_end().child(
                    v_flex()
                        .max_w_full()
                        .px_3p5()
                        .py_2p5()
                        .gap_2()
                        .rounded(theme::panel_radius())
                        .bg(theme::surface_bg(cx))
                        .border_1()
                        .border_color(theme::subtle_border(cx))
                        .children(
                            message
                                .blocks
                                .iter()
                                .map(|block| render_block(block, context, is_streaming, cx)),
                        ),
                )
            })
            .when(!is_user, |lane| {
                lane.items_start()
                    .children(
                        message
                            .blocks
                            .iter()
                            .map(|block| render_block(block, context, is_streaming, cx)),
                    )
                    .when_some(
                        match &message.state {
                            MessageState::Interrupted { reason } => Some(reason.clone()),
                            _ => None,
                        },
                        |col, reason| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().warning)
                                    .child(format!("已中断：{reason}")),
                            )
                        },
                    )
            }),
    )
}

fn render_block(
    block: &BlockSnapshot,
    context: &MessageContext,
    is_streaming: bool,
    cx: &App,
) -> AnyElement {
    match &block.content {
        ContentPresentation::Text { text, .. } => TextView::markdown(
            ElementId::Name(format!("msg-block-md:{}", block.id).into()),
            SharedString::from(text.clone()),
        )
        .selectable(true)
        .stream_fade(is_streaming)
        .into_any_element(),
        ContentPresentation::Image { attachment_id, alt } => {
            let name = context
                .attachments
                .iter()
                .find(|a| a.id == *attachment_id)
                .map(|a| a.name.clone())
                .or_else(|| alt.clone())
                .unwrap_or_else(|| "图片附件".into());
            h_flex()
                .px_2p5()
                .py_1()
                .gap_1p5()
                .rounded(theme::control_radius())
                .bg(theme::window_bg(cx))
                .border_1()
                .border_color(theme::subtle_border(cx))
                .text_xs()
                .text_color(theme::text_secondary(cx))
                .child(Icon::new(IconName::Frame).xsmall())
                .child(name)
                .into_any_element()
        }
        ContentPresentation::Attachment { attachment_id } => {
            let name = context
                .attachments
                .iter()
                .find(|a| a.id == *attachment_id)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "文件附件".into());
            h_flex()
                .px_2p5()
                .py_1()
                .gap_1p5()
                .rounded(theme::control_radius())
                .bg(theme::window_bg(cx))
                .border_1()
                .border_color(theme::subtle_border(cx))
                .text_xs()
                .text_color(theme::text_secondary(cx))
                .child(Icon::new(IconName::File).xsmall())
                .child(name)
                .into_any_element()
        }
        ContentPresentation::FileReference { reference } => {
            let label = match reference.lines {
                Some(lines) => format!(
                    "@{} ({}:{})",
                    reference.path.display(),
                    lines.start(),
                    lines.end()
                ),
                None => format!("@{}", reference.path.display()),
            };
            h_flex()
                .px_2p5()
                .py_1()
                .gap_1p5()
                .rounded(theme::control_radius())
                .bg(theme::window_bg(cx))
                .border_1()
                .border_color(theme::subtle_border(cx))
                .text_xs()
                .text_color(theme::text_secondary(cx))
                .child(Icon::new(IconName::FileText).xsmall())
                .child(label)
                .into_any_element()
        }
        ContentPresentation::AgentCommand {
            command_id,
            arguments,
        } => div()
            .text_xs()
            .text_color(theme::text_secondary(cx))
            .child(if arguments.is_empty() {
                format!("/{command_id}")
            } else {
                format!("/{command_id} {arguments}")
            })
            .into_any_element(),
        ContentPresentation::Unsupported { type_name } => div()
            .px_2p5()
            .py_1p5()
            .rounded(theme::control_radius())
            .bg(theme::surface_bg(cx))
            .text_xs()
            .text_color(theme::text_muted(cx))
            .child(format!("暂不支持呈现该内容块类型：{type_name}"))
            .into_any_element(),
        ContentPresentation::ToolCall { tool_call_id } => div()
            .text_sm()
            .text_color(theme::text_secondary(cx))
            .child(
                context
                    .tool_names
                    .iter()
                    .find(|(id, _)| id == tool_call_id)
                    .map(|(_, name)| format!("工具：{name}"))
                    .unwrap_or_else(|| "工具记录暂不可用".into()),
            )
            .into_any_element(),
        ContentPresentation::Plan { plan_id } => {
            let Some(plan) = context.plans.iter().find(|plan| plan.id == *plan_id) else {
                return div().child("计划记录暂不可用").into_any_element();
            };
            v_flex()
                .gap_1()
                .children(plan.steps.iter().map(|step| {
                    let state = match step.state {
                        workbench_core::PlanStepState::Pending => "待办",
                        workbench_core::PlanStepState::InProgress => "进行中",
                        workbench_core::PlanStepState::Completed => "完成",
                        workbench_core::PlanStepState::Cancelled => "已取消",
                    };
                    div().text_sm().child(format!("{state} · {}", step.text))
                }))
                .into_any_element()
        }
        ContentPresentation::FileChange { file_change_id } => {
            let Some(change) = context
                .changes
                .iter()
                .find(|change| change.id == *file_change_id)
            else {
                return div().child("文件变更记录暂不可用").into_any_element();
            };
            let detail = match &change.diff {
                workbench_core::FileDiff::Unified { text } => text.clone(),
                workbench_core::FileDiff::Binary => "二进制文件变更".into(),
                workbench_core::FileDiff::Unavailable { reason } => reason.clone(),
            };
            v_flex()
                .gap_2()
                .child(change.path.display().to_string())
                .child(
                    TextView::markdown(
                        ElementId::Name(format!("file-change-raw:{}", change.id).into()),
                        format!("~~~~diff\n{detail}\n~~~~"),
                    )
                    .selectable(true),
                )
                .into_any_element()
        }
    }
}

fn render_tool_row(
    tool: &ToolSnapshot,
    is_expanded: bool,
    entity: Entity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let tid = tool.id;
    let (label, color) = match &tool.state {
        ToolState::Pending => ("等待".to_string(), theme::text_secondary(cx)),
        ToolState::Running => ("执行中".to_string(), cx.theme().info),
        ToolState::Completed { .. } => ("完成".to_string(), theme::text_secondary(cx)),
        ToolState::Failed { reason, .. } => (format!("失败：{reason}"), cx.theme().danger),
        ToolState::Cancelled { .. } => ("已取消".to_string(), theme::text_secondary(cx)),
        ToolState::Interrupted { reason, .. } => (format!("中断：{reason}"), cx.theme().warning),
    };
    v_flex().w_full().items_center().px_3().child(
        v_flex()
            .w_full()
            .max_w(theme::conversation_max_width())
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Button::new(ElementId::Name(format!("tool-toggle:{tid}").into()))
                            .ghost()
                            .small()
                            .icon(if is_expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label(SharedString::from(tool.name.clone()))
                            .on_click(move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.toggle_tool(tid, cx);
                                })
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(color)
                            .child(label),
                    ),
            )
            .when(is_expanded, |col| {
                col.when_some(tool.arguments_text.clone(), |col, text| {
                    col.child(
                        TextView::markdown(
                            ElementId::Name(format!("tool-args:{tid}").into()),
                            format!("~~~text\n{text}\n~~~"),
                        )
                        .selectable(true),
                    )
                })
                .when_some(tool.result_text.clone(), |col, text| {
                    col.child(
                        TextView::markdown(
                            ElementId::Name(format!("tool-result:{tid}").into()),
                            format!("~~~text\n{text}\n~~~"),
                        )
                        .selectable(true),
                    )
                })
            }),
    )
}
