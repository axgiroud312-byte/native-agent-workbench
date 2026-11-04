use crate::{theme, workbench::Workbench};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeSet;
use workbench_core::AgentId;
use workbench_service::{ApplicationService, InstallPlan};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    Agents,
    Appearance,
    Workspace,
    Manual(Option<AgentId>),
    Install,
}

/// Only form editing and navigation live here. Confirmed settings and authentication
/// belong to the application service and are read through the workbench owner.
pub(crate) struct SettingsView {
    pub(crate) owner: WeakEntity<Workbench>,
    pub(crate) page: SettingsPage,
    pub(crate) name: Entity<InputState>,
    pub(crate) executable: Entity<InputState>,
    pub(crate) arguments: Entity<TextareaState>,
    pub(crate) form_error: Option<String>,
    pub(crate) form_pending: bool,
    pub(crate) operation_error: Option<String>,
    pub(crate) busy: BTreeSet<String>,
    pub(crate) prepared_plan: Option<InstallPlan>,
    pub(crate) expanded_agent: Option<AgentId>,
    pub(crate) expanded_registry_reasons: BTreeSet<String>,
    _owner_subscription: Option<Subscription>,
}

impl SettingsView {
    pub(crate) fn new(
        owner: WeakEntity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner_subscription = owner
            .upgrade()
            .map(|owner| cx.observe(&owner, |_, _, cx| cx.notify()));
        Self {
            owner,
            page: SettingsPage::Agents,
            name: cx.new(|cx| InputState::new(window, cx)),
            executable: cx.new(|cx| InputState::new(window, cx)),
            arguments: cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 5)),
            form_error: None,
            form_pending: false,
            operation_error: None,
            busy: BTreeSet::new(),
            prepared_plan: None,
            expanded_agent: None,
            expanded_registry_reasons: BTreeSet::new(),
            _owner_subscription: owner_subscription,
        }
    }

    pub(crate) fn application(
        &self,
        cx: &App,
    ) -> Option<Entity<crate::application::ApplicationState>> {
        self.owner
            .upgrade()
            .map(|owner| owner.read(cx).application.clone())
    }
    pub(crate) fn service(&self, cx: &App) -> Option<ApplicationService> {
        self.application(cx)
            .and_then(|app| app.read(cx).handle.clone())
    }
    pub(crate) fn on_open(&mut self, cx: &mut Context<Self>) {
        if let Some(service) = self.service(cx) {
            if service.snapshot().catalog.is_none() && !service.snapshot().catalog_busy {
                self.run_operation(
                    "discover".into(),
                    || service.discover_agents(),
                    |_, _, _| {},
                    cx,
                );
            }
        }
    }

    pub(crate) fn edit_agent(
        &mut self,
        id: Option<AgentId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = self.owner.upgrade().and_then(|owner| {
            id.and_then(|id| {
                owner
                    .read(cx)
                    .host
                    .snapshot()
                    .agents
                    .iter()
                    .find(|a| a.id == id)
                    .cloned()
            })
        });
        self.name.update(cx, |input, cx| {
            input.set_value(
                agent.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
                window,
                cx,
            )
        });
        self.executable.update(cx, |input, cx| {
            input.set_value(
                agent
                    .as_ref()
                    .and_then(|a| a.launch.as_ref())
                    .map(|l| l.executable.display().to_string())
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.arguments.update(cx, |input, cx| {
            input.set_value(
                agent
                    .as_ref()
                    .and_then(|a| a.launch.as_ref())
                    .map(|l| l.args.join("\n"))
                    .unwrap_or_default(),
                window,
                cx,
            )
        });
        self.page = SettingsPage::Manual(id);
        self.form_error = None;
        self.name.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn pick_executable(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择可执行文件".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = response.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            this.executable.update(cx, |input, cx| {
                                input.set_value(path.display().to_string(), window, cx)
                            });
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.form_error = Some(format!("无法选择文件：{error}")),
                    Err(_) => this.form_error = Some("文件选择器已断开，请重试。".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let preferences = owner.read(cx).host.preferences().clone();
        let application = owner.read(cx).application.clone();
        let app = application.read(cx);
        let disabled =
            !owner.read(cx).host.is_fixture() && (app.handle.is_none() || app.preferences_pending);
        v_flex()
            .gap_5()
            .child(setting_row("主题", div().child("炭灰").into_any_element()))
            .child(setting_row(
                "界面缩放",
                h_flex()
                    .gap_2()
                    .children([100u16, 125, 150].map(|percent| {
                        let owner = owner.clone();
                        Button::new(("setting-zoom", usize::from(percent)))
                            .outline()
                            .small()
                            .selected(preferences.zoom_percent == percent)
                            .disabled(disabled)
                            .label(format!("{percent}%"))
                            .on_click(move |_, _, cx| {
                                owner.update(cx, |this, cx| this.set_zoom(percent, cx))
                            })
                    }))
                    .into_any_element(),
            ))
            .into_any_element()
    }

    fn render_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let width = owner.read(cx).host.preferences().sidebar_width;
        let application = owner.read(cx).application.clone();
        let app = application.read(cx);
        let disabled =
            !owner.read(cx).host.is_fixture() && (app.handle.is_none() || app.preferences_pending);
        v_flex()
            .gap_5()
            .child(setting_row(
                "侧栏宽度",
                h_flex()
                    .gap_2()
                    .children([240u16, 264, 320].map(|value| {
                        let owner = owner.clone();
                        Button::new(("setting-sidebar", usize::from(value)))
                            .outline()
                            .small()
                            .selected(width == value)
                            .disabled(disabled)
                            .label(format!("{value}"))
                            .on_click(move |_, _, cx| {
                                owner.update(cx, |this, cx| this.set_sidebar_width(value, cx))
                            })
                    }))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_secondary(cx))
                            .child("逻辑像素"),
                    )
                    .into_any_element(),
            ))
            .into_any_element()
    }

    fn render_manual(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = self.form_pending;
        v_flex()
            .gap_4()
            .child(
                Button::new("manual-back")
                    .ghost()
                    .small()
                    .icon(gpui_kit::component::IconName::ArrowLeft)
                    .label("Agent")
                    .disabled(pending)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = SettingsPage::Agents;
                        this.form_error = None;
                        cx.notify();
                    })),
            )
            .child(field(
                "名称",
                Input::new(&self.name)
                    .disabled(pending)
                    .aria_label("Agent 名称")
                    .into_any_element(),
            ))
            .child(field(
                "可执行文件",
                h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.executable)
                                .disabled(pending)
                                .aria_label("可执行文件"),
                        ),
                    )
                    .child(
                        Button::new("pick-agent-executable")
                            .outline()
                            .label("选择…")
                            .disabled(pending)
                            .on_click(cx.listener(Self::pick_executable)),
                    )
                    .into_any_element(),
            ))
            .child(field(
                "参数（每行一个）",
                Textarea::new(&self.arguments)
                    .disabled(pending)
                    .aria_label("启动参数，每行一个")
                    .into_any_element(),
            ))
            .when_some(self.form_error.clone(), |col, error| {
                col.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("save-agent-configuration")
                        .primary()
                        .label(if pending {
                            "保存中…"
                        } else {
                            "保存配置"
                        })
                        .disabled(pending)
                        .on_click(cx.listener(Self::save_manual)),
                ),
            )
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.page {
            SettingsPage::Agents => self.render_agents(window, cx),
            SettingsPage::Appearance => self.render_appearance(cx),
            SettingsPage::Workspace => self.render_workspace(cx),
            SettingsPage::Manual(_) => self.render_manual(cx),
            SettingsPage::Install => self.render_install(cx),
        };
        let application = self.application(cx);
        let pending = application
            .as_ref()
            .is_some_and(|app| app.read(cx).preferences_pending);
        let error = application
            .as_ref()
            .and_then(|app| app.read(cx).error.clone());
        v_flex()
            .w_full()
            .min_w_0()
            .gap_5()
            .when(
                !matches!(self.page, SettingsPage::Manual(_) | SettingsPage::Install),
                |col| {
                    col.child(
                        TabBar::new("settings-tabs")
                            .underline()
                            .selected_index(match self.page {
                                SettingsPage::Appearance => 1,
                                SettingsPage::Workspace => 2,
                                _ => 0,
                            })
                            .child(Tab::new().label("Agent"))
                            .child(Tab::new().label("外观"))
                            .child(Tab::new().label("工作区"))
                            .on_click(cx.listener(|this, index, _, cx| {
                                this.page = match index {
                                    1 => SettingsPage::Appearance,
                                    2 => SettingsPage::Workspace,
                                    _ => SettingsPage::Agents,
                                };
                                cx.notify();
                            })),
                    )
                },
            )
            .child(
                div()
                    .id("settings-body")
                    .min_h(rems(220.0 / theme::BASE_FONT))
                    .max_h(window.bounds().size.height * 0.55)
                    .overflow_y_scroll()
                    .child(content),
            )
            .when(
                matches!(
                    self.page,
                    SettingsPage::Appearance | SettingsPage::Workspace
                ),
                |col| {
                    col.when(pending, |col| {
                        col.child(
                            div()
                                .text_sm()
                                .text_color(theme::text_secondary(cx))
                                .child("正在保存…"),
                        )
                    })
                    .when_some(error, |col, error| {
                        col.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
                },
            )
    }
}

fn field(label: &'static str, control: AnyElement) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(div().text_sm().child(label))
        .child(control)
}

fn setting_row(label: &'static str, control: AnyElement) -> impl IntoElement {
    h_flex()
        .w_full()
        .min_w_0()
        .gap_4()
        .justify_between()
        .child(div().text_sm().child(label))
        .child(control)
}
