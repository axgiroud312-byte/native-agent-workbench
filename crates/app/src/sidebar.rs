use crate::{
    navigation::object_menu,
    theme,
    workbench::{NewSession, OpenProject, OpenSettings, Workbench},
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
use workbench_service::RemovalTarget;

impl Workbench {
    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let projects = self.host.snapshot().projects.clone();
        let sessions = self.host.snapshot().sessions.clone();
        let selected = self.host.preferences().selected_project_id;
        let picker_label = self
            .host
            .selected_project()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "打开项目…".into());
        let owner = cx.weak_entity();
        let menu_owner = owner.clone();
        let tree_owner = owner.clone();
        let row_owner = owner.clone();
        let application = self.application.clone();
        let can_create = !self.host.is_fixture()
            && self.service_ready(cx)
            && selected.is_some_and(|id| {
                !self
                    .application
                    .read(cx)
                    .removing(RemovalTarget::Project(id))
            });
        let picker = Button::new("project-selector")
            .ghost()
            .small()
            .w_full()
            .icon(IconName::FolderOpen)
            .label(picker_label)
            .disabled(self.picker_open || !self.host.is_fixture() && !self.service_ready(cx));
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
                        let view = menu_owner.clone();
                        menu = menu.item(
                            PopupMenuItem::new(project.name.clone())
                                .checked(selected == Some(id))
                                .on_click(move |_, window, cx| {
                                    let _ = view
                                        .update(cx, |this, cx| this.choose_project(id, window, cx));
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
        let tree = Tree::new(&self.session_tree, move |_, entry, _, _, cx| {
            let id = entry.item().id.clone();
            let target = id
                .strip_prefix("project:")
                .and_then(|id| id.parse().ok())
                .map(RemovalTarget::Project)
                .or_else(|| {
                    id.strip_prefix("session:")
                        .and_then(|id| id.parse().ok())
                        .map(RemovalTarget::Session)
                });
            let is_project = matches!(target, Some(RemovalTarget::Project(_)));
            let app = application.read(cx);
            let mut status = match target {
                Some(RemovalTarget::Session(sid)) => sessions
                    .iter()
                    .find(|s| s.id == sid)
                    .map(|s| session_row_status(s, cx)),
                Some(RemovalTarget::Project(pid)) => {
                    if app.relocating_projects.contains(&pid) {
                        Some((
                            IconName::LoaderCircle,
                            theme::text_secondary(cx),
                            Some("定位中".into()),
                        ))
                    } else if app.confirmed.as_ref().is_some_and(|frame| {
                        frame
                            .project_directories
                            .iter()
                            .any(|d| d.project_id == pid && d.error.is_some())
                    }) {
                        Some((
                            IconName::CircleAlert,
                            cx.theme().warning,
                            Some("目录失效".into()),
                        ))
                    } else {
                        None
                    }
                }
                None => None,
            };
            if let Some(target) = target {
                if app.removing(target) {
                    status = Some((
                        IconName::LoaderCircle,
                        theme::text_secondary(cx),
                        Some(if is_project { "移除中" } else { "删除中" }.into()),
                    ));
                } else if app.navigation_errors.contains_key(&target) {
                    status = Some((
                        IconName::CircleAlert,
                        cx.theme().danger,
                        Some("操作失败".into()),
                    ));
                }
            }
            let owner = row_owner.clone();
            let label = entry.item().label.clone();
            ListItem::new(ElementId::Name(format!("navigation-row:{id}").into()))
                .accessibility_label(label.clone())
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
                                .child(label.clone()),
                        )
                        .when_some(status.and_then(|s| s.2), |row, label| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(theme::text_secondary(cx))
                                    .child(label),
                            )
                        })
                        .when_some(target, |row, target| {
                            row.child(
                                Button::new(ElementId::Name(format!("object-menu:{id}").into()))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Ellipsis)
                                    .accessibility_label(format!("{}的操作", label))
                                    .tooltip("操作")
                                    .dropdown_caret(false)
                                    .dropdown_menu(move |menu, _, cx| {
                                        object_menu(menu, target, owner.clone(), cx)
                                    }),
                            )
                        }),
                )
        })
        .context_menu(move |_, entry, menu, _, cx| {
            let id = &entry.item().id;
            let target = id
                .strip_prefix("project:")
                .and_then(|id| id.parse().ok())
                .map(RemovalTarget::Project)
                .or_else(|| {
                    id.strip_prefix("session:")
                        .and_then(|id| id.parse().ok())
                        .map(RemovalTarget::Session)
                });
            match target {
                Some(target) => object_menu(menu, target, tree_owner.clone(), cx),
                None => menu,
            }
        });
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
                    .gap_1()
                    .flex_shrink_0()
                    .child(div().flex_1().min_w_0().child(picker))
                    .child(
                        Button::new("new-session")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .accessibility_label("新建会话")
                            .tooltip("新建会话 (Ctrl+N)")
                            .disabled(!can_create)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_session(&NewSession, window, cx)
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(tree))
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
