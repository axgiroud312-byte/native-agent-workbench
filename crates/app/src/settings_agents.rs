use crate::{
    settings::{SettingsPage, SettingsView},
    theme,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    text::TextView,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use workbench_core::{
    AgentDefinition, AgentDiscovery, AgentId, AgentSource, AuthenticationAction,
    AuthenticationFailure, AuthenticationInteractionAction, AuthenticationInteractionKind,
    AuthenticationMethodAvailability, AuthenticationScope, AuthenticationStatus,
    AuthenticationUnavailableReason,
};
use workbench_service::{
    AuthenticationView, Installability, ManualLaunch, RegistryState, ServiceFuture,
};

impl SettingsView {
    pub(crate) fn run_operation<T: 'static>(
        &mut self,
        key: String,
        build: impl FnOnce() -> ServiceFuture<'static, T>,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.busy.insert(key.clone()) {
            return;
        }
        let future = build();
        self.operation_error = None;
        let application = self.application(cx);
        cx.spawn(async move |this, cx| {
            let result = future.await;
            if let Some(application) = application {
                application.update(cx, |app, cx| app.read_latest(cx));
            }
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&key);
                match result {
                    Ok(value) => done(this, value, cx),
                    Err(error) => this.operation_error = Some(error.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn save_manual(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let SettingsPage::Manual(id) = self.page else {
            return;
        };
        let Some(service) = self.service(cx) else {
            return;
        };
        if self.form_pending {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        let executable = self.executable.read(cx).value().trim().to_owned();
        if name.is_empty() || executable.is_empty() {
            self.form_error = Some("请填写名称和可执行文件。".into());
            cx.notify();
            return;
        }
        let arguments = self.arguments.read(cx).value().to_string();
        let args = if arguments.is_empty() {
            Vec::new()
        } else {
            arguments.lines().map(str::to_owned).collect()
        };
        let future = service.configure_agent(id, name, ManualLaunch::new(executable.into(), args));
        self.form_pending = true;
        self.form_error = None;
        let application = self.application(cx);
        cx.spawn(async move |this, cx| {
            let result = future.await;
            if let Some(application) = application {
                application.update(cx, |app, cx| app.read_latest(cx));
            }
            let _ = this.update(cx, |this, cx| {
                this.form_pending = false;
                match result {
                    Ok(id) => {
                        this.page = SettingsPage::Agents;
                        this.expanded_agent = Some(id);
                    }
                    Err(error) => this.form_error = Some(error.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn check_agent(&mut self, id: AgentId, cx: &mut Context<Self>) {
        let key = format!("check:{id}");
        if self.busy.contains(&key) {
            return;
        }
        let Some(service) = self.service(cx) else {
            return;
        };
        self.expanded_agent = Some(id);
        self.run_operation(
            key,
            || service.discover_authentication(id),
            |_, _, _| {},
            cx,
        );
    }

    fn send_authentication(
        &mut self,
        scope: AuthenticationScope,
        action: AuthenticationAction,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self.service(cx) else {
            return;
        };
        let current = service.snapshot();
        let view = current
            .authentication
            .iter()
            .find(|view| view.scope == scope);
        let valid = view.is_some_and(|view| match &action {
            AuthenticationAction::Authenticate { method_id } => {
                !matches!(view.status, AuthenticationStatus::InProgress { .. })
                    && view.methods.iter().any(|m| {
                        m.id == *method_id
                            && m.availability == AuthenticationMethodAvailability::Available
                    })
            }
            AuthenticationAction::Cancel { request_id } => {
                view.request_id == Some(*request_id)
                    && matches!(view.status, AuthenticationStatus::InProgress { .. })
            }
            AuthenticationAction::Interact { interaction_id, .. } => {
                view.interactions.iter().any(|i| i.id == *interaction_id)
            }
            AuthenticationAction::Discover => true,
        });
        if !valid {
            self.operation_error = Some("认证状态已变化，请重新检查连接。".into());
            cx.notify();
            return;
        }
        let key = format!("auth:{:?}", scope);
        if self.busy.contains(&key) {
            return;
        }
        self.run_operation(
            key,
            || service.authentication(scope, action),
            |_, _, _| {},
            cx,
        );
    }

    fn prepare_installation(&mut self, registry_id: String, cx: &mut Context<Self>) {
        let key = format!("prepare:{registry_id}");
        if self.busy.contains(&key) {
            return;
        }
        let Some(service) = self.service(cx) else {
            return;
        };
        self.run_operation(
            key,
            || service.prepare_install(registry_id),
            |this, plan, _| {
                this.prepared_plan = Some(plan);
                this.page = SettingsPage::Install;
            },
            cx,
        );
    }

    pub(crate) fn render_agents(&self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        if owner.read(cx).host.is_fixture() {
            return div()
                .text_sm()
                .text_color(theme::text_secondary(cx))
                .child("开发样例不连接 Agent")
                .into_any_element();
        }
        let application = owner.read(cx).application.clone();
        let app = application.read(cx);
        let confirmed = app.confirmed.clone();
        let available = app.handle.is_some();
        let starting = app.starting;
        let startup_error = app.error.clone();
        let catalog = confirmed.as_ref().and_then(|s| s.catalog.clone());
        let busy = confirmed.as_ref().is_some_and(|s| s.catalog_busy)
            || self.busy.contains("refresh")
            || self.busy.contains("discover");
        let agents = confirmed
            .as_ref()
            .map(|s| s.workbench.agents.clone())
            .unwrap_or_default();
        let authentication = confirmed
            .as_ref()
            .map(|s| s.authentication.clone())
            .unwrap_or_default();
        let registry_agents = catalog
            .as_ref()
            .map(|c| c.available.clone())
            .unwrap_or_default();
        let local_issues = catalog
            .as_ref()
            .map(|c| c.local.tools.issues.clone())
            .unwrap_or_default();
        v_flex().gap_4()
            .child(h_flex().gap_2().justify_end()
                .child(Button::new("discover-agents").outline().small().icon(IconName::RefreshCw)
                    .label(if busy { "正在发现…" } else { "刷新" }).disabled(!available || busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(service) = this.service(cx) {
                            this.run_operation("refresh".into(), || service.refresh_agents(), |_, _, _| {}, cx);
                        }
                    })))
                .child(Button::new("manual-agent").outline().small().label("手动配置…").disabled(!available)
                    .on_click(cx.listener(|this, _, window, cx| this.edit_agent(None, window, cx)))))
            .when_some(self.operation_error.clone().or(startup_error), |col, error| col.child(
                div().text_sm().text_color(cx.theme().danger).child(error)))
            .when(starting, |col| col.child(div().text_sm().child("正在打开工作区…")))
            .when(!available && !starting, |col| col.child(Button::new("retry-settings-service").outline().small().label("重试")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(application) = this.application(cx) { application.update(cx, |app, cx| app.start(cx)); }
                }))))
            .when(confirmed.as_ref().is_some_and(|s| !s.running_installations.is_empty()), |col| col.child(div().text_sm().child("正在安装 Agent…")))
            .when(agents.is_empty() && available && !busy, |col| col.child(
                div().text_sm().text_color(theme::text_secondary(cx)).child("尚未配置 Agent")))
            .children(agents.iter().map(|agent| {
                let auth = authentication.iter().filter(|v| v.scope.agent_id() == agent.id && v.scope.session_id().is_none())
                    .max_by_key(|v| v.scope.epoch());
                self.render_agent_row(agent, auth, cx)
            }))
            .when_some(catalog.as_ref(), |col, catalog| {
                col.when_some(match &catalog.registry.state {
                    RegistryState::Live => None,
                    RegistryState::Cached { stale, offline } => Some(match (*stale, *offline) {
                        (true, true) => "目录离线，使用过期缓存；已配置 Agent 仍可使用。",
                        (_, true) => "目录离线，使用缓存；已配置 Agent 仍可使用。",
                        (true, false) => "当前目录缓存已过期，可刷新。",
                        _ => "使用本地目录缓存。",
                    }.to_owned()),
                    RegistryState::Unavailable { reason } => Some(format!("无法获取 Agent 目录：{reason}")),
                }, |col, message| col.child(div().text_sm().text_color(theme::text_secondary(cx)).child(message)))
            })
            .children(local_issues.into_iter().map(|issue| div().text_sm().text_color(theme::text_secondary(cx)).child(issue)))
            .when(!registry_agents.is_empty(), |col| col.child(
                v_flex().gap_2().pt_2().border_t_1().border_color(theme::subtle_border(cx))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("目录"))
                    .children(registry_agents.iter().filter(|entry| !agents.iter().any(|agent| {
                        matches!(&agent.source, AgentSource::Registry { registry_id } if *registry_id == entry.registry_id)
                            && matches!(agent.discovery, AgentDiscovery::Installed { .. })
                    })).map(|entry| {
                        let id = entry.registry_id.clone();
                        let pending = self.busy.contains(&format!("prepare:{id}"));
                        let reason = match &entry.installation { Installability::Supported => None, Installability::Unavailable { reason } => Some(reason.clone()) };
                        let unavailable = reason.is_some();
                        let expanded = self.expanded_registry_reasons.contains(&id);
                        v_flex().gap_1().py_2()
                            .child(h_flex().gap_3().min_w_0()
                                .child(div().flex_1().min_w_0().text_sm().child(format!("{} · {}", entry.name, entry.version)))
                                .child(Button::new(ElementId::Name(format!("install-agent:{id}").into())).outline().small()
                                    .label(if unavailable { "不可安装" } else if pending { "准备中…" } else { "安装…" })
                                    .when(unavailable, |button| button.icon(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }))
                                    .disabled(pending)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if unavailable {
                                            if !this.expanded_registry_reasons.remove(&id) {
                                                this.expanded_registry_reasons.insert(id.clone());
                                            }
                                            cx.notify();
                                        } else {
                                            this.prepare_installation(id.clone(), cx);
                                        }
                                    }))))
                            .when(expanded, |col| col.when_some(reason, |col, reason| col.child(div().text_sm().text_color(theme::text_secondary(cx)).child(reason))))
                    }))))
            .into_any_element()
    }

    fn render_agent_row(
        &self,
        agent: &AgentDefinition,
        auth: Option<&AuthenticationView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = agent.id;
        let checking = self.busy.contains(&format!("check:{id}"));
        let usable = matches!(agent.discovery, AgentDiscovery::Installed { .. });
        let status = auth
            .map(|view| auth_status(&view.status).to_owned())
            .unwrap_or_else(|| match &agent.discovery {
                AgentDiscovery::NotChecked => "待检查".into(),
                AgentDiscovery::NotInstalled => "未安装".into(),
                AgentDiscovery::Installed { version, .. } => version
                    .as_ref()
                    .map(|v| format!("已安装 {v}"))
                    .unwrap_or_else(|| "已安装".into()),
                AgentDiscovery::Incompatible { .. } => "不兼容".into(),
                AgentDiscovery::Failed { .. } => "检查失败".into(),
            });
        let reason = match &agent.discovery {
            AgentDiscovery::Incompatible { reason } | AgentDiscovery::Failed { reason } => {
                Some(reason.clone())
            }
            _ => None,
        };
        v_flex()
            .gap_2()
            .py_3()
            .border_b_1()
            .border_color(theme::subtle_border(cx))
            .child(
                h_flex()
                    .gap_3()
                    .min_w_0()
                    .child(Icon::new(IconName::Bot).small())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(agent.name.clone()),
                            )
                            .child(div().text_xs().text_color(theme::text_secondary(cx)).child(
                                if checking {
                                    "检查中…".to_owned()
                                } else {
                                    status
                                },
                            )),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("check-agent:{id}").into()))
                            .outline()
                            .small()
                            .label("检查连接")
                            .disabled(
                                !usable
                                    || checking
                                    || auth.is_some_and(|a| {
                                        matches!(a.status, AuthenticationStatus::InProgress { .. })
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.check_agent(id, cx))),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("configure-agent:{id}").into()))
                            .ghost()
                            .small()
                            .label("配置…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_agent(Some(id), window, cx)
                            })),
                    ),
            )
            .when_some(reason, |col, reason| {
                col.child(div().text_sm().text_color(cx.theme().danger).child(reason))
            })
            .when(self.expanded_agent == Some(id), |col| {
                col.when_some(auth, |col, auth| {
                    col.child(self.render_authentication(auth, cx))
                })
            })
            .into_any_element()
    }

    fn render_authentication(
        &self,
        view: &AuthenticationView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scope = view.scope;
        let in_progress = matches!(view.status, AuthenticationStatus::InProgress { .. });
        let busy = self.busy.contains(&format!("auth:{scope:?}"));
        v_flex().gap_2()
            .children(view.methods.iter().map(|method| {
                let id = method.id.clone();
                let reason = match method.availability {
                    AuthenticationMethodAvailability::Available => None,
                    AuthenticationMethodAvailability::Unavailable { reason } => Some(unavailable_reason(reason)),
                };
                h_flex().gap_3().min_w_0()
                    .child(Button::new(ElementId::Name(format!("auth-method:{}:{}", scope.agent_id(), id.as_str()).into())).outline().small()
                        .label(method.name.clone()).disabled(reason.is_some() || in_progress || busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.send_authentication(scope, AuthenticationAction::Authenticate { method_id: id.clone() }, cx))))
                    .when_some(reason, |row, reason| row.child(div().text_xs().text_color(theme::text_secondary(cx)).child(reason)))
            }))
            .when_some(if in_progress { view.request_id } else { None }, |col, request_id| col.child(
                Button::new(ElementId::Name(format!("cancel-auth:{}", scope.agent_id()).into())).ghost().small().label("取消登录").disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.send_authentication(scope, AuthenticationAction::Cancel { request_id }, cx)))))
            .children(view.interactions.iter().map(|interaction| {
                let id = interaction.id;
                let key = opaque_key(&id);
                h_flex().gap_2()
                    .when(interaction.kind == AuthenticationInteractionKind::OfficialUrl, |row| row.child(
                        Button::new(ElementId::Name(format!("auth-open:{key}").into())).primary().small().label("打开官方登录").disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.send_authentication(scope, AuthenticationAction::Interact { interaction_id: id, action: AuthenticationInteractionAction::OpenOfficialFlow }, cx)))))
                    .when(interaction.kind == AuthenticationInteractionKind::UnsupportedSecret, |row| row.child(
                        div().text_sm().child("此认证步骤需要当前不支持的保密输入。")))
                    .child(Button::new(ElementId::Name(format!("auth-decline:{key}").into())).outline().small().label("拒绝").disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.send_authentication(scope, AuthenticationAction::Interact { interaction_id: id, action: AuthenticationInteractionAction::Decline }, cx))))
                    .child(Button::new(ElementId::Name(format!("auth-cancel:{key}").into())).ghost().small().label("取消").disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.send_authentication(scope, AuthenticationAction::Interact { interaction_id: id, action: AuthenticationInteractionAction::Cancel }, cx))))
            }))
            .into_any_element()
    }

    pub(crate) fn render_install(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(plan) = &self.prepared_plan else {
            return div().into_any_element();
        };
        let current = self.service(cx).map(|service| service.snapshot());
        let active = current
            .as_ref()
            .is_some_and(|s| s.prepared_installations.iter().any(|p| p.id == plan.id));
        let running = current
            .as_ref()
            .is_some_and(|s| s.running_installations.contains(&plan.id))
            || self.busy.contains("install");
        let ready = active && !running && plan.prerequisites.iter().all(|p| p.satisfied);
        let plan_id = plan.id;
        let command = format!(
            "{}\n{}",
            plan.command.program.display(),
            plan.command
                .args
                .iter()
                .map(|arg| format!("  {arg}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        v_flex()
            .gap_4()
            .child(
                Button::new("install-back")
                    .ghost()
                    .small()
                    .icon(IconName::ArrowLeft)
                    .label("Agent")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = SettingsPage::Agents;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("安装 Codex"),
            )
            .child(detail("来源", plan.source_url.clone(), cx))
            .child(detail(
                "版本",
                format!(
                    "codex-acp {} · Codex {}",
                    plan.adapter_version, plan.native_agent_version
                ),
                cx,
            ))
            .child(detail(
                "目标目录",
                plan.target_directory.display().to_string(),
                cx,
            ))
            .child(detail(
                "工作目录",
                plan.command.working_directory.display().to_string(),
                cx,
            ))
            .child(
                v_flex().gap_2().child("安装命令").child(
                    TextView::markdown("install-command", format!("~~~text\n{command}\n~~~"))
                        .selectable(true),
                ),
            )
            .children(plan.prerequisites.iter().map(|item| {
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(if item.satisfied {
                            IconName::Check
                        } else {
                            IconName::TriangleAlert
                        })
                        .small(),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(format!("{}：{}", item.name, item.detail)),
                    )
            }))
            .when_some(self.operation_error.clone(), |col, error| {
                col.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .when(!active && !running, |col| {
                col.child(div().text_sm().child("安装计划已失效，请返回重新准备。"))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-install-plan")
                            .outline()
                            .label(if running { "返回" } else { "取消" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = SettingsPage::Agents;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("confirm-install-plan")
                            .primary()
                            .label(if running { "安装中…" } else { "安装" })
                            .disabled(!ready)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let Some(service) = this.service(cx) else {
                                    return;
                                };
                                if this.busy.contains("install")
                                    || !service
                                        .snapshot()
                                        .prepared_installations
                                        .iter()
                                        .any(|p| p.id == plan_id)
                                {
                                    return;
                                }
                                this.run_operation(
                                    "install".into(),
                                    || service.install(plan_id),
                                    |this, id, _| {
                                        this.page = SettingsPage::Agents;
                                        this.expanded_agent = Some(id);
                                        this.prepared_plan = None;
                                    },
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn detail(label: &'static str, value: String, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme::text_secondary(cx))
                .child(label),
        )
        .child(div().text_sm().child(value))
}

fn auth_status(status: &AuthenticationStatus) -> &'static str {
    match status {
        AuthenticationStatus::Unknown => "已连接，认证未确认",
        AuthenticationStatus::Discovering => "检查认证中…",
        AuthenticationStatus::Required => "需要登录",
        AuthenticationStatus::InProgress { .. } => "等待官方登录",
        AuthenticationStatus::Authenticated => "已认证",
        AuthenticationStatus::Cancelled => "登录已取消",
        AuthenticationStatus::Failed { failure } => match failure {
            AuthenticationFailure::NoLiveConnection => "认证连接已关闭，请重新检查",
            AuthenticationFailure::StaleScope | AuthenticationFailure::InteractionExpired => {
                "认证已过期，请重新检查"
            }
            AuthenticationFailure::MethodNotOffered | AuthenticationFailure::MethodUnavailable => {
                "认证方式不可用"
            }
            AuthenticationFailure::ProviderRejected => "Agent 拒绝认证",
            AuthenticationFailure::Transport => "认证连接失败",
            AuthenticationFailure::Timeout => "认证超时",
            AuthenticationFailure::OfficialFlowFailed => "无法打开官方登录",
        },
    }
}
fn unavailable_reason(reason: AuthenticationUnavailableReason) -> &'static str {
    match reason {
        AuthenticationUnavailableReason::CapabilityUnknown => "尚未确认能力",
        AuthenticationUnavailableReason::HostUnsupported => "当前工作台不支持此方式",
        AuthenticationUnavailableReason::UnsupportedMethodKind => "暂不支持此认证方式",
        AuthenticationUnavailableReason::SecretEntryUnsupported => "暂不支持密钥输入",
        AuthenticationUnavailableReason::NoLiveConnection => "没有可用连接",
    }
}

fn opaque_key(value: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher as _;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hash);
    hash.finish()
}
