use crate::{
    theme,
    workbench::{OpenProject, Workbench},
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
        let Some(session) = self.host.active_session().cloned() else {
            let has_directory =
                self.host.temporary_directory().is_some() || self.host.selected_project().is_some();
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
                            .disabled(self.picker_open)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_project(&OpenProject, window, cx)
                            })),
                    )
                })
                .when(has_directory, |col| {
                    col.child(
                        div()
                            .text_sm()
                            .text_color(theme::text_secondary(cx))
                            .child("暂无会话"),
                    )
                })
                .when_some(self.picker_error.clone(), |col, error| {
                    col.child(div().text_sm().text_color(cx.theme().danger).child(error))
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
        let messages = session.messages.clone();
        let tools = session.tools.clone();
        let attachments = session.attachments.clone();
        let expanded = self.host.preferences().expanded_tools.clone();
        let entity = cx.entity();
        v_flex()
            .id("conversation-region")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
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
                        move |ix, _, cx| {
                            if let Some(message) = messages.get(ix) {
                                render_message_row(message, &attachments, cx).into_any_element()
                            } else if let Some(tool) = tools.get(ix.saturating_sub(messages.len()))
                            {
                                render_tool_row(
                                    tool,
                                    expanded.contains(&tool.id),
                                    entity.clone(),
                                    cx,
                                )
                                .into_any_element()
                            } else {
                                div().into_any_element()
                            }
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
    attachments: &[workbench_core::Attachment],
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
                                .map(|block| render_block(block, attachments, is_streaming, cx)),
                        ),
                )
            })
            .when(!is_user, |lane| {
                lane.items_start()
                    .children(
                        message
                            .blocks
                            .iter()
                            .map(|block| render_block(block, attachments, is_streaming, cx)),
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
    attachments: &[workbench_core::Attachment],
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
            let name = attachments
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
            let name = attachments
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
        _ => div().into_any_element(),
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
                                    this.host.toggle_tool_expanded(tid);
                                    this.message_scroller.update(cx, |s, cx| s.remeasure(cx));
                                    cx.notify();
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
