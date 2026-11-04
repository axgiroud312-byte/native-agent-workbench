use crate::{
    application::ApplicationState,
    theme,
    workbench::{NewSession, OpenSettings, Workbench},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};
use workbench_core::{AgentDiscovery, AgentId, ConnectionPhase, ProjectId, SessionId};
use workbench_service::RemovalTarget;

impl Workbench {
    pub(crate) fn confirm_close_connection(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_fixture() || !self.service_ready(cx) {
            return;
        }
        let Some(session) = self
            .host
            .snapshot()
            .sessions
            .iter()
            .find(|session| session.id == id)
        else {
            return;
        };
        let title = format!("关闭“{}”的连接？", session.title);
        let application = self.application.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let application = application.clone();
            dialog
                .title(title.clone())
                .description(
                    "运行中的任务会停止，本地会话记录保留。资源释放结果以 Agent 返回状态为准。",
                )
                .confirm()
                .ok_text("关闭连接")
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    application.update(cx, |app, cx| app.connection_action(id, false, cx));
                    true
                })
        });
    }
    pub(crate) fn new_session(
        &mut self,
        _: &NewSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.host.preferences().selected_project_id {
            self.open_new_session(id, window, cx);
        }
    }

    pub(crate) fn open_new_session(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_fixture()
            || !self.service_ready(cx)
            || self
                .application
                .read(cx)
                .removing(RemovalTarget::Project(id))
        {
            return;
        }
        let Some(project) = self.host.snapshot().projects.iter().find(|p| p.id == id) else {
            return;
        };
        let project_name = project.name.clone();
        let initial_agent = self.host.active_session().map(|s| s.record.agent_id());
        let application = self.application.clone();
        let owner = cx.weak_entity();
        let open = Rc::new(Cell::new(true));
        let dialog_open = open.clone();
        let form = cx.new(|cx| {
            CreateSessionView::new(
                owner,
                application,
                id,
                project_name,
                initial_agent,
                open,
                window,
                cx,
            )
        });
        window.open_dialog(cx, move |dialog, window, _| {
            let open = dialog_open.clone();
            dialog
                .title("新建会话")
                .w(theme::settings_width(window))
                .overlay_closable(false)
                .child(form.clone())
                .on_close(move |_, _, _| open.set(false))
        });
    }

    pub(crate) fn confirm_removal(
        &mut self,
        target: RemovalTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_fixture()
            || !self.service_ready(cx)
            || self.application.read(cx).removing(target)
        {
            return;
        }
        let (title, description, action) = match target {
            RemovalTarget::Session(id) => {
                let Some(session) = self.host.snapshot().sessions.iter().find(|s| s.id == id)
                else {
                    return;
                };
                (
                    format!("删除“{}”？", session.title),
                    "删除本工作台中的该会话记录。运行中的任务会先停止；项目文件和其他客户端历史保留。",
                    "删除会话",
                )
            }
            RemovalTarget::Project(id) => {
                let Some(project) = self.host.snapshot().projects.iter().find(|p| p.id == id)
                else {
                    return;
                };
                (
                    format!("移除“{}”？", project.name),
                    "移除项目索引及它在本工作台中的全部会话记录。运行中的任务会先停止；磁盘目录和其他客户端历史保留。",
                    "移除项目",
                )
            }
        };
        let application = self.application.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let application = application.clone();
            dialog
                .title(title.clone())
                .description(description)
                .confirm()
                .ok_text(action)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("取消")
                .on_ok(move |_, _, cx| {
                    application.update(cx, |app, cx| app.remove(target, cx));
                    true
                })
        });
    }

    pub(crate) fn pick_project_directory(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_fixture() || !self.service_ready(cx) {
            return;
        }
        let app = self.application.read(cx);
        if app.removing(RemovalTarget::Project(id)) || app.relocating_projects.contains(&id) {
            return;
        }
        let Some(project) = self
            .host
            .snapshot()
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
        else {
            return;
        };
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(format!("重新定位“{}”", project.name).into()),
        });
        let application = self.application.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = response.await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(Ok(Some(paths))) => if let Some(path) = paths.into_iter().next() {
                    let title = format!("重新定位“{}”", project.name);
                    let previous = project.directory.display().to_string();
                    let next = path.display().to_string();
                    window.open_alert_dialog(cx, move |dialog, _, _| {
                        let application = application.clone();
                        let path = path.clone();
                        dialog.title(title.clone()).confirm().ok_text("重新定位").cancel_text("取消")
                            .description("后续工作使用新目录。已有消息和差异保留原路径，原远端会话身份保持不变；旧资源未释放时无法更改。")
                            .child(v_flex().gap_3()
                                .child(v_flex().gap_1().child("原目录").child(previous.clone()))
                                .child(v_flex().gap_1().child("新目录").child(next.clone())))
                            .on_ok(move |_, _, cx| { application.update(cx, |app, cx| app.relocate(id, path.clone(), cx)); true })
                    });
                },
                Ok(Ok(None)) => {},
                Ok(Err(error)) => application.update(cx, |app, cx| {
                    app.navigation_errors.insert(RemovalTarget::Project(id), format!("无法选择目录：{error}")); cx.notify();
                }),
                Err(_) => application.update(cx, |app, cx| {
                    app.navigation_errors.insert(RemovalTarget::Project(id), "文件夹选择器已断开，请重试。".into()); cx.notify();
                }),
            });
        }).detach();
    }

    pub(crate) fn render_navigation_notice(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project_id = self.host.preferences().selected_project_id;
        let session_id = self.host.preferences().selected_session_id;
        let app = self.application.read(cx);
        let removing_project =
            project_id.is_some_and(|id| app.removing(RemovalTarget::Project(id)));
        let removing_session =
            session_id.is_some_and(|id| app.removing(RemovalTarget::Session(id)));
        let relocating = project_id.is_some_and(|id| app.relocating_projects.contains(&id));
        let directory_error = project_id.and_then(|id| {
            app.confirmed
                .as_ref()
                .and_then(|frame| {
                    frame
                        .project_directories
                        .iter()
                        .find(|d| d.project_id == id)
                })
                .and_then(|directory| directory.error.as_ref().map(|error| error.message.clone()))
        });
        let operation_error = session_id
            .and_then(|id| {
                app.navigation_errors
                    .get(&RemovalTarget::Session(id))
                    .cloned()
            })
            .or_else(|| {
                project_id.and_then(|id| {
                    app.navigation_errors
                        .get(&RemovalTarget::Project(id))
                        .cloned()
                })
            });
        v_flex()
            .w_full()
            .flex_shrink_0()
            .when(removing_project || removing_session || relocating, |col| {
                col.child(div().px_6().py_2().text_sm().child(if removing_project {
                    "正在移除项目…"
                } else if removing_session {
                    "正在删除会话…"
                } else {
                    "正在重新定位目录…"
                }))
            })
            .when_some(operation_error, |col, error| {
                col.child(
                    div()
                        .px_6()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .when_some(directory_error, |col, error| {
                col.child(
                    h_flex()
                        .px_6()
                        .py_2()
                        .gap_3()
                        .min_w_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                        .when_some(project_id, |row, id| {
                            row.child(
                                Button::new("repair-project-directory")
                                    .outline()
                                    .small()
                                    .label("重新选择目录…")
                                    .disabled(removing_project || relocating)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.pick_project_directory(id, window, cx)
                                    })),
                            )
                        }),
                )
            })
    }
}

/// Both the row button and the native context menu use this exact target identity.
pub(crate) fn object_menu(
    mut menu: PopupMenu,
    target: RemovalTarget,
    owner: WeakEntity<Workbench>,
    cx: &App,
) -> PopupMenu {
    let Some(view) = owner.upgrade() else {
        return menu;
    };
    let workbench = view.read(cx);
    let app = workbench.application.read(cx);
    let unavailable = workbench.host.is_fixture() || app.handle.is_none() || app.removing(target);
    let error = app.navigation_errors.get(&target).cloned();
    match target {
        RemovalTarget::Project(id) => {
            let new_owner = owner.clone();
            let relocate_owner = owner.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("新建会话…")
                        .icon(IconName::Plus)
                        .disabled(unavailable)
                        .on_click(move |_, window, cx| {
                            let _ = new_owner
                                .update(cx, |this, cx| this.open_new_session(id, window, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("重新定位目录…")
                        .icon(IconName::FolderOpen)
                        .disabled(unavailable || app.relocating_projects.contains(&id))
                        .on_click(move |_, window, cx| {
                            let _ = relocate_owner
                                .update(cx, |this, cx| this.pick_project_directory(id, window, cx));
                        }),
                )
                .separator();
        }
        RemovalTarget::Session(id) => {
            if let Some(session) = workbench
                .host
                .snapshot()
                .sessions
                .iter()
                .find(|s| s.id == id)
            {
                let project = session.project_id;
                let new_owner = owner.clone();
                let close_owner = owner.clone();
                let connect_app = workbench.application.clone();
                let connecting = matches!(
                    session.connection.phase,
                    ConnectionPhase::Ready
                        | ConnectionPhase::Starting
                        | ConnectionPhase::Authenticating
                );
                let request_pending = app.connection_requests.contains(&id);
                menu = menu
                    .item(
                        PopupMenuItem::new("连接会话")
                            .icon(IconName::Play)
                            .disabled(unavailable || connecting || request_pending)
                            .on_click(move |_, _, cx| {
                                connect_app
                                    .update(cx, |app, cx| app.connection_action(id, true, cx))
                            }),
                    )
                    .item(
                        PopupMenuItem::new("关闭连接…")
                            .icon(IconName::Square)
                            .disabled(unavailable || request_pending)
                            .on_click(move |_, window, cx| {
                                let _ = close_owner.update(cx, |this, cx| {
                                    this.confirm_close_connection(id, window, cx)
                                });
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("使用其他 Agent 新建…")
                            .icon(IconName::Plus)
                            .disabled(unavailable)
                            .on_click(move |_, window, cx| {
                                let _ = new_owner.update(cx, |this, cx| {
                                    this.open_new_session(project, window, cx)
                                });
                            }),
                    )
                    .separator();
            }
        }
    }
    if let Some(error) = error {
        menu = menu
            .item(
                PopupMenuItem::new("查看操作错误…")
                    .icon(IconName::CircleAlert)
                    .on_click(move |_, window, cx| {
                        let error = error.clone();
                        window.open_dialog(cx, move |dialog, _, _| {
                            dialog.title("操作未完成").child(error.clone()).footer(
                                Button::new("close-operation-error")
                                    .primary()
                                    .label("关闭")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                        });
                    }),
            )
            .separator();
    }
    let label = match target {
        RemovalTarget::Project(_) => "移除项目…",
        RemovalTarget::Session(_) => "删除会话…",
    };
    menu.item(
        PopupMenuItem::new(label)
            .icon(IconName::Delete)
            .disabled(unavailable)
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| this.confirm_removal(target, window, cx));
            }),
    )
}

struct CreateSessionView {
    owner: WeakEntity<Workbench>,
    application: Entity<ApplicationState>,
    project_id: ProjectId,
    project_name: String,
    agent_id: Option<AgentId>,
    title: Entity<InputState>,
    pending: bool,
    error: Option<String>,
    open: Rc<Cell<bool>>,
    _subscription: Subscription,
}
impl CreateSessionView {
    fn new(
        owner: WeakEntity<Workbench>,
        application: Entity<ApplicationState>,
        project_id: ProjectId,
        project_name: String,
        initial_agent: Option<AgentId>,
        open: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let agents = application
            .read(cx)
            .confirmed
            .as_ref()
            .map(|frame| &frame.workbench.agents);
        let agent_id = initial_agent
            .filter(|id| {
                agents.is_some_and(|agents| {
                    agents.iter().any(|a| {
                        a.id == *id && matches!(a.discovery, AgentDiscovery::Installed { .. })
                    })
                })
            })
            .or_else(|| {
                agents
                    .and_then(|agents| {
                        agents
                            .iter()
                            .find(|a| matches!(a.discovery, AgentDiscovery::Installed { .. }))
                    })
                    .map(|a| a.id)
            });
        let subscription = cx.observe(&application, |_, _, cx| cx.notify());
        Self {
            owner,
            application,
            project_id,
            project_name,
            agent_id,
            title: cx.new(|cx| InputState::new(window, cx).placeholder("新会话")),
            pending: false,
            error: None,
            open,
            _subscription: subscription,
        }
    }

    fn create(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(agent) = self.agent_id else {
            self.error = Some("请选择可用的 Agent。".into());
            cx.notify();
            return;
        };
        let Some(handle) = self.application.read(cx).handle.clone() else {
            return;
        };
        let raw = self.title.read(cx).value().to_string();
        let title = if raw.trim().is_empty() {
            "新会话".into()
        } else {
            raw.trim().to_owned()
        };
        let future = handle.create_session(self.project_id, agent, title);
        self.pending = true;
        self.error = None;
        cx.notify();
        let application = self.application.clone();
        let open = self.open.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            application.update(cx, |app, cx| app.read_latest(cx));
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending = false;
                match result {
                    Ok(_) => {
                        if open.get() {
                            window.close_dialog(cx);
                        }
                    }
                    Err(error) => this.error = Some(error.message),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for CreateSessionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.application.read(cx);
        let frame = app.confirmed.clone();
        let agents = frame
            .as_ref()
            .map(|s| s.workbench.agents.clone())
            .unwrap_or_default();
        let project_exists = frame
            .as_ref()
            .is_some_and(|s| s.workbench.projects.iter().any(|p| p.id == self.project_id));
        let directory_error = frame
            .as_ref()
            .and_then(|s| {
                s.project_directories
                    .iter()
                    .find(|d| d.project_id == self.project_id)
            })
            .and_then(|d| d.error.as_ref().map(|e| e.message.clone()));
        let unavailable = self.pending
            || !project_exists
            || app.removing(RemovalTarget::Project(self.project_id));
        let selected = agents.iter().find(|a| Some(a.id) == self.agent_id);
        let valid_agent =
            selected.is_some_and(|a| matches!(a.discovery, AgentDiscovery::Installed { .. }));
        let label = selected
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "选择 Agent…".into());
        let view = cx.entity();
        let selected_id = self.agent_id;
        let picker = Button::new("new-session-agent")
            .outline()
            .label(label)
            .disabled(unavailable)
            .dropdown_menu(move |mut menu, _, _| {
                for agent in &agents {
                    let view = view.clone();
                    let id = agent.id;
                    menu = menu.item(
                        PopupMenuItem::new(agent.name.clone())
                            .checked(selected_id == Some(id))
                            .disabled(!matches!(agent.discovery, AgentDiscovery::Installed { .. }))
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.agent_id = Some(id);
                                    cx.notify();
                                })
                            }),
                    );
                }
                menu
            });
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .child("项目")
                    .child(self.project_name.clone()),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_3()
                    .child("Agent")
                    .child(picker),
            )
            .child(
                v_flex().gap_2().child("名称").child(
                    Input::new(&self.title)
                        .aria_label("会话名称")
                        .disabled(unavailable),
                ),
            )
            .when(!valid_agent, |col| {
                col.child(
                    Button::new("configure-session-agent")
                        .outline()
                        .label("配置 Agent…")
                        .disabled(self.pending)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let _ = this.owner.update(cx, |owner, cx| {
                                owner.open_settings(&OpenSettings, window, cx)
                            });
                        })),
                )
            })
            .when(!project_exists, |col| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child("项目已移除。"),
                )
            })
            .when_some(
                directory_error.clone().or(self.error.clone()),
                |col, error| col.child(div().text_sm().text_color(cx.theme().danger).child(error)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-create-session")
                            .outline()
                            .label(if self.pending { "关闭" } else { "取消" })
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("create-session")
                            .primary()
                            .label(if self.pending {
                                "正在创建…"
                            } else {
                                "创建会话"
                            })
                            .disabled(unavailable || !valid_agent || directory_error.is_some())
                            .on_click(cx.listener(Self::create)),
                    ),
            )
    }
}
