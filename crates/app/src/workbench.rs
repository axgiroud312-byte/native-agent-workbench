use crate::{
    application::{ApplicationOwner, ApplicationState, PreferenceEdit},
    conversation::ConversationCache,
    fixture::FixturePreset,
    host::{WorkbenchHost, draft_plain_text},
    settings::SettingsView,
    theme,
};
use gpui_kit::component::{
    ResizableState, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, TextareaState},
    message_scroller::MessageScrollerState,
    resizable::{h_resizable, resizable_panel},
    tree::{TreeEvent, TreeItem, TreeState},
    v_flex,
};
use gpui_kit::*;
use std::collections::BTreeMap;
use workbench_core::{ProjectId, SessionId, ToolCallId};

actions!(
    workbench,
    [
        OpenProject,
        OpenSettings,
        NewSession,
        SubmitDraft,
        StopRun,
        FocusComposer,
        FocusNavigation,
        Quit
    ]
);

pub struct Workbench {
    focus: FocusHandle,
    pub(crate) host: WorkbenchHost,
    pub(crate) application: Entity<ApplicationState>,
    pub(crate) picker_open: bool,
    pub(crate) picker_error: Option<SharedString>,
    picker_task: Option<Task<()>>,
    split: Entity<ResizableState>,
    pub(crate) session_tree: Entity<TreeState>,
    pub(crate) composer_input: Entity<TextareaState>,
    pub(crate) message_scroller: Entity<MessageScrollerState>,
    scrollers: BTreeMap<SessionId, Entity<MessageScrollerState>>,
    pub(crate) conversations: BTreeMap<SessionId, ConversationCache>,
    editor_session: Option<SessionId>,
    editor_model_text: String,
    pub(crate) enter_suppressed: bool,
    settings: Option<Entity<SettingsView>>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let application = cx.global::<ApplicationOwner>().0.clone();
        let host = WorkbenchHost::new(FixturePreset::from_env_or_args());
        let focus = cx.focus_handle();
        let split = cx.new(|_| ResizableState::default());
        let session_tree = cx.new(|cx| TreeState::new(cx).items(Self::tree_items(&host)));
        let composer_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入草稿…")
                .submit_on_enter(true)
                .auto_grow(2, 4)
        });
        let message_scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let mut this = Self {
            focus,
            host,
            application,
            picker_open: false,
            picker_error: None,
            picker_task: None,
            split,
            session_tree,
            composer_input,
            message_scroller,
            scrollers: BTreeMap::new(),
            conversations: BTreeMap::new(),
            editor_session: None,
            editor_model_text: String::new(),
            enter_suppressed: false,
            settings: None,
            _subscriptions: Vec::new(),
        };
        this.sync_session(window, cx);
        this.select_tree_item(cx);
        this._subscriptions.push(cx.observe_in(
            &this.application,
            window,
            |this, _, window, cx| {
                this.sync_application(window, cx);
            },
        ));
        this._subscriptions.push(cx.observe_in(
            &this.session_tree,
            window,
            |this, tree, window, cx| {
                let id = tree.read(cx).selected_item().map(|item| item.id.clone());
                if let Some(id) = id {
                    if let Some(raw) = id.strip_prefix("session:") {
                        if let Ok(id) = raw.parse::<SessionId>() {
                            if this.host.preferences().selected_session_id != Some(id) {
                                this.choose_session(id, window, cx);
                            }
                        }
                    } else if let Some(raw) = id.strip_prefix("project:") {
                        if let Ok(id) = raw.parse::<ProjectId>() {
                            if this.host.preferences().selected_project_id != Some(id) {
                                this.choose_project(id, window, cx);
                            }
                        }
                    }
                }
            },
        ));
        this._subscriptions.push(cx.subscribe(
            &this.session_tree,
            |this, _, event: &TreeEvent, cx| {
                let (id, expanded) = match event {
                    TreeEvent::Expanded(id) => (id, true),
                    TreeEvent::Collapsed(id) => (id, false),
                };
                if let Some(raw) = id.strip_prefix("project:") {
                    if let Ok(id) = raw.parse() {
                        if this.host.is_fixture() {
                            this.host.set_project_expanded(id, expanded);
                        } else {
                            this.application.update(cx, |app, cx| {
                                app.save_preference(
                                    PreferenceEdit::ProjectExpanded(id, expanded),
                                    cx,
                                )
                            });
                        }
                    }
                }
                cx.notify();
            },
        ));
        this._subscriptions.push(cx.subscribe_in(
            &this.composer_input,
            window,
            |this, input, event, window, cx| {
                if let InputEvent::PressEnter { shift, .. } = event {
                    let suppressed = std::mem::take(&mut this.enter_suppressed);
                    if !shift && !suppressed {
                        this.submit_draft(&SubmitDraft, window, cx);
                    }
                    return;
                }
                if let InputEvent::Change = event {
                    if input.update(cx, |input, cx| {
                        input.marked_text_range(window, cx).is_some()
                    }) {
                        return;
                    }
                    let Some(id) = this.editor_session else {
                        return;
                    };
                    let text = input.read(cx).value().to_string();
                    let expected = this
                        .application
                        .read(cx)
                        .draft_text(id)
                        .map(str::to_owned)
                        .or_else(|| {
                            this.host
                                .active_session()
                                .filter(|s| s.record.id() == id)
                                .map(|s| draft_plain_text(&s.draft))
                        });
                    if expected.as_deref() == Some(text.as_str()) {
                        return;
                    }
                    if this.host.is_fixture() {
                        this.host.save_draft_text(text);
                    } else {
                        this.application
                            .update(cx, |app, cx| app.save_draft(id, text, cx));
                    }
                    cx.notify();
                }
            },
        ));
        this.sync_application(window, cx);
        window.focus(&this.focus, cx);
        this
    }

    fn tree_items(host: &WorkbenchHost) -> Vec<TreeItem> {
        host.snapshot()
            .projects
            .iter()
            .map(|project| {
                TreeItem::new(format!("project:{}", project.id), project.name.clone())
                    .expanded(host.preferences().expanded_projects.contains(&project.id))
                    .children(
                        host.snapshot()
                            .sessions
                            .iter()
                            .filter(|s| s.project_id == project.id)
                            .map(|s| TreeItem::new(format!("session:{}", s.id), s.title.clone())),
                    )
            })
            .collect()
    }

    fn select_tree_item(&self, cx: &mut Context<Self>) {
        let key = self
            .host
            .preferences()
            .selected_session_id
            .filter(|id| {
                self.host.snapshot().sessions.iter().any(|session| {
                    session.id == *id
                        && self
                            .host
                            .preferences()
                            .expanded_projects
                            .contains(&session.project_id)
                })
            })
            .map(|id| format!("session:{id}"))
            .or_else(|| {
                self.host
                    .preferences()
                    .selected_project_id
                    .map(|id| format!("project:{id}"))
            });
        self.session_tree.update(cx, |tree, cx| {
            let item = key.map(|key| TreeItem::new(key, ""));
            tree.set_selected_item(item.as_ref(), cx);
        });
    }

    fn sync_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_fixture() {
            cx.notify();
            return;
        }
        let Some(snapshot) = self.application.read(cx).confirmed.clone() else {
            cx.notify();
            return;
        };
        let old = self.host.snapshot();
        let structure_changed = old.projects.iter().map(|p| (&p.id, &p.name)).ne(snapshot
            .workbench
            .projects
            .iter()
            .map(|p| (&p.id, &p.name)))
            || old
                .sessions
                .iter()
                .map(|s| (&s.id, &s.project_id, &s.title))
                .ne(snapshot
                    .workbench
                    .sessions
                    .iter()
                    .map(|s| (&s.id, &s.project_id, &s.title)));
        let expansion_changed =
            self.host.preferences().expanded_projects != snapshot.preferences.expanded_projects;
        let expansion_already_visible = snapshot.workbench.projects.iter().all(|project| {
            let tree = self.session_tree.read(cx);
            let key: SharedString = format!("project:{}", project.id).into();
            tree.index_of(&key)
                .and_then(|ix| tree.entry(ix))
                .is_some_and(|entry| {
                    entry.is_expanded()
                        == snapshot.preferences.expanded_projects.contains(&project.id)
                })
        });
        let selection_changed = self.host.preferences().selected_session_id
            != snapshot.preferences.selected_session_id
            || self.host.preferences().selected_project_id
                != snapshot.preferences.selected_project_id;
        let zoom_changed =
            self.host.preferences().zoom_percent != snapshot.preferences.zoom_percent;
        let tools_expanded =
            self.host.preferences().expanded_tools != snapshot.preferences.expanded_tools;
        self.host.accept_confirmed(snapshot.clone());
        self.scrollers.retain(|id, _| {
            snapshot
                .workbench
                .sessions
                .iter()
                .any(|session| session.id == *id)
        });
        self.conversations.retain(|id, _| {
            snapshot
                .workbench
                .sessions
                .iter()
                .any(|session| session.id == *id)
        });
        let rebuild_tree = structure_changed || expansion_changed && !expansion_already_visible;
        if rebuild_tree {
            let items = Self::tree_items(&self.host);
            self.session_tree
                .update(cx, |tree, cx| tree.set_items(items, cx));
        }
        if rebuild_tree || selection_changed {
            self.select_tree_item(cx);
        }
        let new_id = self.host.active_session().map(|s| s.record.id());
        if self.editor_session != new_id {
            self.sync_session(window, cx);
        } else if let Some(session) = self.host.active_session() {
            let text = self
                .application
                .read(cx)
                .draft_text(session.record.id())
                .map(str::to_owned)
                .unwrap_or_else(|| draft_plain_text(&session.draft));
            let value = self.composer_input.read(cx).value().to_string();
            let composing = self.composer_input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            });
            if !composing && (value == text || value == self.editor_model_text) {
                self.editor_model_text = text.clone();
                if value != text {
                    self.composer_input
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                }
            }
            self.sync_conversation(cx);
        }
        if tools_expanded {
            self.message_scroller.update(cx, |s, cx| s.remeasure(cx));
        }
        let running = self.active_run().is_some();
        self.composer_input
            .update(cx, |input, cx| input.set_submit_on_enter(!running, cx));
        if zoom_changed {
            for scroller in self.scrollers.values() {
                scroller.update(cx, |s, cx| s.remeasure(cx));
            }
        }
        if let Some(settings) = &self.settings {
            settings.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    pub(crate) fn sync_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_session = self.host.active_session().map(|s| s.record.id());
        let text = self
            .editor_session
            .and_then(|id| self.application.read(cx).draft_text(id).map(str::to_owned))
            .or_else(|| {
                self.host
                    .active_session()
                    .map(|s| draft_plain_text(&s.draft))
            })
            .unwrap_or_default();
        self.editor_model_text = text.clone();
        self.composer_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        if let Some(session) = self.host.active_session() {
            let id = session.record.id();
            self.message_scroller = self
                .scrollers
                .entry(id)
                .or_insert_with(|| cx.new(|cx| MessageScrollerState::new(0, cx)))
                .clone();
            self.sync_conversation(cx);
        }
        cx.notify();
    }

    fn sync_conversation(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.host.active_session() {
            self.conversations
                .entry(session.record.id())
                .or_default()
                .sync(session, &self.message_scroller, cx);
        }
    }

    pub(crate) fn choose_project(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.is_fixture() {
            self.host.select_project(id);
            self.sync_session(window, cx);
            self.select_tree_item(cx);
        } else {
            self.application
                .update(cx, |app, cx| app.select_project(Some(id), cx));
        }
        cx.notify();
    }
    fn choose_session(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_fixture() {
            self.host.select_session(id);
            self.sync_session(window, cx);
        } else {
            self.application
                .update(cx, |app, cx| app.select_session(Some(id), cx));
        }
        cx.notify();
    }
    pub(crate) fn set_zoom(&mut self, percent: u16, cx: &mut Context<Self>) {
        if self.host.is_fixture() {
            let mut prefs = self.host.preferences().clone();
            prefs.zoom_percent = theme::apply_zoom_percent(percent, cx);
            self.host.update_preferences(prefs);
            for scroller in self.scrollers.values() {
                scroller.update(cx, |s, cx| s.remeasure(cx));
            }
        } else {
            self.application.update(cx, |app, cx| {
                app.save_preference(PreferenceEdit::Zoom(percent), cx)
            });
        }
        cx.notify();
    }
    pub(crate) fn set_sidebar_width(&mut self, value: u16, cx: &mut Context<Self>) {
        let value = theme::clamp_sidebar_width_logical(value, None);
        if self.host.preferences().sidebar_width == value {
            return;
        }
        if self.host.is_fixture() {
            let mut prefs = self.host.preferences().clone();
            prefs.sidebar_width = value;
            self.host.update_preferences(prefs);
        } else {
            self.application.update(cx, |app, cx| {
                app.save_preference(PreferenceEdit::Sidebar(value), cx)
            });
        }
        cx.notify();
    }
    pub(crate) fn toggle_tool(&mut self, id: ToolCallId, cx: &mut Context<Self>) {
        if self.host.is_fixture() {
            self.host.toggle_tool_expanded(id);
        } else {
            let expanded = !self.host.preferences().expanded_tools.contains(&id);
            self.application.update(cx, |app, cx| {
                app.save_preference(PreferenceEdit::ToolExpanded(id, expanded), cx)
            });
        }
        self.message_scroller.update(cx, |s, cx| s.remeasure(cx));
        cx.notify();
    }
    pub(crate) fn service_ready(&self, cx: &App) -> bool {
        self.application.read(cx).handle.is_some()
    }
    pub(crate) fn service_error(&self, cx: &App) -> Option<String> {
        let app = self.application.read(cx);
        app.error
            .clone()
            .or_else(|| {
                app.confirmed
                    .as_ref()
                    .and_then(|s| s.last_error.as_ref().map(|e| e.message.clone()))
            })
            .filter(|message| !app.navigation_errors.values().any(|error| error == message))
    }

    pub(crate) fn open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open || self.host.is_fixture() || !self.service_ready(cx) {
            return;
        }
        self.picker_open = true;
        self.picker_error = None;
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择项目文件夹".into()),
        });
        let handle = self
            .application
            .read(cx)
            .handle
            .clone()
            .expect("checked service");
        let application = self.application.clone();
        self.picker_task = Some(cx.spawn(async move |this, cx| {
            let result = match response.await {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.into_iter().next() {
                        let name = path
                            .file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy()
                            .into_owned();
                        handle
                            .open_project(path, name)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.message)
                    } else {
                        Ok(())
                    }
                }
                Ok(Ok(None)) => Ok(()),
                Ok(Err(error)) => Err(format!("无法打开文件夹：{error}")),
                Err(_) => Err("文件夹选择器已断开，请重试。".into()),
            };
            application.update(cx, |app, cx| app.read_latest(cx));
            let _ = this.update(cx, |this, cx| {
                this.picker_open = false;
                this.picker_error = result.err().map(Into::into);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self
            .settings
            .get_or_insert_with(|| {
                let owner = cx.weak_entity();
                cx.new(|cx| SettingsView::new(owner, window, cx))
            })
            .clone();
        let opening = settings.clone();
        cx.spawn(async move |_, cx| {
            opening.update(cx, |settings, cx| settings.on_open(cx));
        })
        .detach();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title("设置")
                .w(theme::settings_width(window))
                .child(settings.clone())
                .footer(
                    Button::new("settings-done")
                        .primary()
                        .label("完成")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
        });
    }
    fn focus_composer(&mut self, _: &FocusComposer, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.active_session().is_some() {
            self.composer_input
                .update(cx, |input, cx| input.focus(window, cx));
        }
    }
    fn focus_navigation(
        &mut self,
        _: &FocusNavigation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_tree
            .update(cx, |tree, cx| tree.focus(window, cx));
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.render_sidebar(cx).into_any_element();
        let header = self.render_workspace_header(window, cx).into_any_element();
        let conversation = self
            .render_conversation_region(window, cx)
            .into_any_element();
        let composer = self.render_composer_region(cx).into_any_element();
        let navigation_notice = self.render_navigation_notice(cx).into_any_element();
        let view = cx.entity();
        let width = theme::sidebar_width_pixels(
            self.host.preferences().sidebar_width,
            Some(window.bounds().size.width),
        );
        v_flex()
            .key_context("Workbench")
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::new_session))
            .on_action(cx.listener(Self::submit_draft))
            .on_action(cx.listener(Self::stop_run))
            .on_action(cx.listener(Self::focus_composer))
            .on_action(cx.listener(Self::focus_navigation))
            .bg(theme::workspace_bg(cx))
            .text_color(theme::text_primary(cx))
            .child(
                TitleBar::new()
                    .bg(theme::window_bg(cx))
                    .border_color(theme::window_bg(cx))
                    .child(
                        div().text_sm().child(
                            self.host
                                .active_preset()
                                .map(|f| f.label())
                                .unwrap_or("Agent Workbench"),
                        ),
                    ),
            )
            .child(
                h_resizable("workbench-split")
                    .with_state(&self.split)
                    .on_resize(move |state, window, cx| {
                        if let Some(width) = state.read(cx).sizes().first().copied() {
                            view.update(cx, |this, cx| {
                                let width = theme::clamp_sidebar_width_logical(
                                    f32::from(width).round() as u16,
                                    Some(window.bounds().size.width),
                                );
                                this.set_sidebar_width(width, cx);
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        resizable_panel()
                            .size(width)
                            .size_range(theme::sidebar_size_range())
                            .fixed()
                            .child(sidebar),
                    )
                    .child(
                        resizable_panel().child(
                            v_flex()
                                .size_full()
                                .min_w_0()
                                .min_h_0()
                                .bg(theme::workspace_bg(cx))
                                .child(header)
                                .child(navigation_notice)
                                .child(conversation)
                                .child(composer),
                        ),
                    ),
            )
    }
}
