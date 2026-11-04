use crate::{
    theme,
    workbench::{OpenProject, OpenSettings, Workbench},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    menu::{DropdownMenu as _, PopupMenuItem},
    tree::Tree,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use workbench_core::{ConnectionPhase, ExecutionState, QueueDispatchState, SessionSummary};

impl Workbench {
    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let projects = self.host.snapshot().projects.clone();
        let sessions = self.host.snapshot().sessions.clone();
        let selected = self.host.preferences().selected_project_id;
        let picker_label = self
            .host
            .selected_project()
            .map(|p| p.name.clone())
            .or_else(|| {
                self.host.temporary_directory().map(|p| {
                    p.file_name()
                        .unwrap_or(p.as_os_str())
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .unwrap_or_else(|| "打开项目…".into());
        let entity = cx.entity();
        let picker = Button::new("project-selector")
            .ghost()
            .small()
            .w_full()
            .icon(IconName::FolderOpen)
            .label(SharedString::from(picker_label))
            .disabled(self.picker_open);
        let picker = if projects.is_empty() {
            picker
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_project(&OpenProject, window, cx)),
                )
                .into_any_element()
        } else {
            picker
                .dropdown_menu(move |mut menu, _, _| {
                    for project in &projects {
                        let id = project.id;
                        let view = entity.clone();
                        menu = menu.item(
                            PopupMenuItem::new(project.name.clone())
                                .checked(selected == Some(id))
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.host.select_project(id);
                                        this.sync_session(window, cx);
                                        if let Some(id) =
                                            this.host.preferences().selected_session_id
                                        {
                                            this.session_tree.update(cx, |tree, cx| {
                                                tree.set_selected_item(
                                                    Some(
                                                        &gpui_kit::component::tree::TreeItem::new(
                                                            format!("session:{id}"),
                                                            "",
                                                        ),
                                                    ),
                                                    cx,
                                                )
                                            });
                                        }
                                        cx.notify();
                                    })
                                }),
                        );
                    }
                    menu.separator().item(
                        PopupMenuItem::new("打开项目…")
                            .icon(IconName::FolderOpen)
                            .action(Box::new(OpenProject)),
                    )
                })
                .into_any_element()
        };
        v_flex()
            .id("sidebar-surface")
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(theme::sidebar_bg(cx))
            .child(
                h_flex()
                    .h(theme::header_height())
                    .px(theme::sidebar_padding())
                    .flex_shrink_0()
                    .child(picker),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(Tree::new(
                &self.session_tree,
                move |_, entry, _, _, cx| {
                    let id = entry.item().id.clone();
                    let is_project = id.starts_with("project:");
                    let session = sessions
                        .iter()
                        .find(|s| id.as_ref() == format!("session:{}", s.id));
                    let status = session.map(|s| session_row_status(s, cx));
                    ListItem::new(ElementId::Name(format!("navigation-row:{id}").into()))
                        .accessibility_label(entry.item().label.clone())
                        .h(theme::session_row_height())
                        .px(theme::sidebar_padding())
                        .py_0()
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .when(!is_project, |row| row.pl(theme::child_indent()))
                                .child(
                                    Icon::new(if is_project {
                                        if entry.is_expanded() {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        }
                                    } else {
                                        status
                                            .as_ref()
                                            .map(|s| s.0.clone())
                                            .unwrap_or(IconName::Bot)
                                    })
                                    .small()
                                    .text_color(
                                        status
                                            .as_ref()
                                            .map(|s| s.1)
                                            .unwrap_or(theme::text_secondary(cx)),
                                    ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .child(entry.item().label.clone()),
                                )
                                .when_some(status.and_then(|s| s.2), |row, label| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(theme::text_secondary(cx))
                                            .child(label),
                                    )
                                }),
                        )
                },
            )))
            .child(
                h_flex()
                    .h(theme::header_height())
                    .px(theme::sidebar_padding())
                    .flex_shrink_0()
                    .child(
                        Button::new("open-settings")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .label("设置…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(&OpenSettings, window, cx)
                            })),
                    ),
            )
    }
}

pub(crate) fn session_row_status(
    session: &SessionSummary,
    cx: &App,
) -> (IconName, Hsla, Option<SharedString>) {
    if session.closing {
        return (
            IconName::LoaderCircle,
            theme::text_secondary(cx),
            Some("关闭中".into()),
        );
    }
    if session.waiting_requests > 0 {
        return (
            IconName::TriangleAlert,
            cx.theme().warning,
            Some(format!("待处理 {}", session.waiting_requests).into()),
        );
    }
    match &session.connection.phase {
        ConnectionPhase::Failed { .. } => {
            return (
                IconName::CircleAlert,
                cx.theme().danger,
                Some("失败".into()),
            );
        }
        ConnectionPhase::Disconnected { .. } => {
            return (
                IconName::CircleAlert,
                cx.theme().warning,
                Some("已断开".into()),
            );
        }
        ConnectionPhase::Starting | ConnectionPhase::Authenticating => {
            return (
                IconName::LoaderCircle,
                theme::text_secondary(cx),
                Some("连接中".into()),
            );
        }
        _ => {}
    }
    match &session.execution {
        Some(ExecutionState::Starting | ExecutionState::Running) => {
            (IconName::Loader, cx.theme().info, Some("运行中".into()))
        }
        Some(ExecutionState::Cancelling { .. }) => (
            IconName::LoaderCircle,
            cx.theme().warning,
            Some("停止中".into()),
        ),
        Some(ExecutionState::Failed { .. }) => (
            IconName::CircleAlert,
            cx.theme().danger,
            Some("失败".into()),
        ),
        Some(ExecutionState::Interrupted { .. } | ExecutionState::OutcomeUnknown { .. }) => (
            IconName::TriangleAlert,
            cx.theme().warning,
            Some("需核对".into()),
        ),
        _ if matches!(session.queue, QueueDispatchState::Paused { .. }) => (
            IconName::Pause,
            theme::text_secondary(cx),
            Some("队列暂停".into()),
        ),
        _ => (IconName::Bot, theme::text_secondary(cx), None),
    }
}
