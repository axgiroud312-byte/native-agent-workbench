use crate::{
    fixture::FixturePreset,
    host::{WorkbenchHost, draft_plain_text},
    theme,
};
use gpui_kit::component::{
    ResizableState, Selectable as _, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, TextareaState},
    message_scroller::MessageScrollerState,
    resizable::{h_resizable, resizable_panel},
    tree::{TreeEvent, TreeItem, TreeState},
    v_flex,
};
use gpui_kit::*;
use std::collections::BTreeMap;
use workbench_core::{ProjectId, SessionId};

actions!(
    workbench,
    [
        OpenProject,
        OpenSettings,
        FocusComposer,
        FocusNavigation,
        Quit
    ]
);

pub struct Workbench {
    focus: FocusHandle,
    pub(crate) host: WorkbenchHost,
    pub(crate) picker_open: bool,
    pub(crate) picker_error: Option<SharedString>,
    picker_task: Option<Task<()>>,
    split: Entity<ResizableState>,
    pub(crate) session_tree: Entity<TreeState>,
    pub(crate) composer_input: Entity<TextareaState>,
    pub(crate) message_scroller: Entity<MessageScrollerState>,
    scrollers: BTreeMap<SessionId, Entity<MessageScrollerState>>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = WorkbenchHost::new(FixturePreset::from_env_or_args());
        let focus = cx.focus_handle();
        let split = cx.new(|_| ResizableState::default());
        let items = Self::tree_items(&host);
        let session_tree = cx.new(|cx| TreeState::new(cx).items(items));
        let composer_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入草稿…")
                .auto_grow(2, 4)
        });
        let message_scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let mut this = Self {
            focus,
            host,
            picker_open: false,
            picker_error: None,
            picker_task: None,
            split,
            session_tree,
            composer_input,
            message_scroller,
            scrollers: BTreeMap::new(),
            _subscriptions: Vec::new(),
        };
        this.sync_session(window, cx);
        if let Some(id) = this.host.preferences().selected_session_id {
            this.session_tree.update(cx, |tree, cx| {
                tree.set_selected_item(Some(&TreeItem::new(format!("session:{id}"), "")), cx)
            });
        }
        this._subscriptions.push(cx.observe_in(
            &this.session_tree,
            window,
            |this, tree, window, cx| {
                let id = tree.read(cx).selected_item().map(|item| item.id.clone());
                if let Some(id) = id {
                    if let Some(raw) = id.strip_prefix("session:") {
                        if let Ok(id) = raw.parse::<SessionId>() {
                            if this.host.preferences().selected_session_id != Some(id) {
                                this.host.select_session(id);
                                this.sync_session(window, cx);
                            }
                        }
                    } else if let Some(raw) = id.strip_prefix("project:") {
                        if let Ok(id) = raw.parse::<ProjectId>() {
                            this.host.select_project(id);
                            this.sync_session(window, cx);
                        }
                    }
                    cx.notify();
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
                        this.host.set_project_expanded(id, expanded);
                    }
                }
                cx.notify();
            },
        ));
        this._subscriptions.push(
            cx.subscribe(&this.composer_input, |this, input, event, cx| {
                if let InputEvent::Change = event {
                    this.host
                        .save_draft_text(input.read(cx).value().to_string());
                    cx.notify();
                }
            }),
        );
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

    pub(crate) fn sync_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .host
            .active_session()
            .map(|s| draft_plain_text(&s.draft))
            .unwrap_or_default();
        self.composer_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        if let Some(session) = self.host.active_session() {
            let id = session.record.id();
            let count = session.messages.len() + session.tools.len();
            self.message_scroller = self
                .scrollers
                .entry(id)
                .or_insert_with(|| cx.new(|cx| MessageScrollerState::new(count, cx)))
                .clone();
        }
        cx.notify();
    }

    pub(crate) fn open_project(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
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
        self.picker_task = Some(cx.spawn(async move |this, cx| {
            let result = response.await;
            let _ = this.update(cx, |this, cx| {
                this.picker_open = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.host.set_temporary_directory(path);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.picker_error = Some(format!("无法打开文件夹：{error}").into())
                    }
                    Err(_) => this.picker_error = Some("文件夹选择器已断开，请重试。".into()),
                }
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
        let entity = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let zoom = entity.read(cx).host.preferences().zoom_percent;
            dialog
                .title("设置")
                .child(
                    v_flex()
                        .gap_4()
                        .child(h_flex().justify_between().child("外观").child("炭灰"))
                        .child(v_flex().gap_2().child("界面缩放").child(
                            h_flex().gap_2().children([100u16, 125, 150].map(|percent| {
                                let entity = entity.clone();
                                Button::new(("zoom", usize::from(percent)))
                                    .outline()
                                    .small()
                                    .selected(zoom == percent)
                                    .label(format!("{percent}%"))
                                    .on_click(move |_, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            let mut prefs = this.host.preferences().clone();
                                            prefs.zoom_percent =
                                                theme::apply_zoom_percent(percent, cx);
                                            this.host.update_preferences(prefs);
                                            for scroller in this.scrollers.values() {
                                                scroller.update(cx, |s, cx| s.remeasure(cx));
                                            }
                                            cx.notify();
                                        })
                                    })
                            })),
                        )),
                )
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
                                let mut prefs = this.host.preferences().clone();
                                prefs.sidebar_width = theme::clamp_sidebar_width_logical(
                                    f32::from(width).round() as u16,
                                    Some(window.bounds().size.width),
                                );
                                this.host.update_preferences(prefs);
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
                                .child(conversation)
                                .child(composer),
                        ),
                    ),
            )
    }
}
