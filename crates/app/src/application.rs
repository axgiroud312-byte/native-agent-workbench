use crate::{host::draft_command, theme};
use gpui_kit::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};
use workbench_core::*;
use workbench_service::{ApplicationService, ServiceOptions, ServiceSnapshot};

pub(crate) struct ApplicationState {
    pub(crate) handle: Option<ApplicationService>,
    pub(crate) confirmed: Option<Arc<ServiceSnapshot>>,
    pub(crate) error: Option<String>,
    pub(crate) starting: bool,
    pub(crate) preferences_pending: bool,
    options: ServiceOptions,
    subscription: Option<Task<()>>,
    preference_edits: Vec<PreferenceEdit>,
    draft_edits: BTreeMap<SessionId, String>,
    drafts_in_flight: BTreeSet<SessionId>,
    draft_failures: BTreeMap<SessionId, String>,
    quit_requested: bool,
    shutdown_started: bool,
}

/// One application owner, independent of settings dialogs and selected sessions.
pub(crate) struct ApplicationOwner(pub Entity<ApplicationState>);
impl Global for ApplicationOwner {}

#[derive(Clone)]
pub(crate) enum PreferenceEdit {
    Zoom(u16),
    Sidebar(u16),
    Project(ProjectId),
    ProjectExpanded(ProjectId, bool),
    ToolExpanded(ToolCallId, bool),
}
impl PreferenceEdit {
    fn apply(&self, preferences: &mut WorkspacePreferences) {
        match *self {
            Self::Zoom(value) => preferences.zoom_percent = value,
            Self::Sidebar(value) => preferences.sidebar_width = value,
            Self::Project(id) => preferences.selected_project_id = Some(id),
            Self::ProjectExpanded(id, expanded) => {
                if expanded {
                    preferences.expanded_projects.insert(id);
                } else {
                    preferences.expanded_projects.remove(&id);
                }
            }
            Self::ToolExpanded(id, expanded) => {
                if expanded {
                    preferences.expanded_tools.insert(id);
                } else {
                    preferences.expanded_tools.remove(&id);
                }
            }
        }
    }
    fn same_target(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Zoom(_), Self::Zoom(_))
            | (Self::Sidebar(_), Self::Sidebar(_))
            | (Self::Project(_), Self::Project(_)) => true,
            (Self::ProjectExpanded(a, _), Self::ProjectExpanded(b, _)) => a == b,
            (Self::ToolExpanded(a, _), Self::ToolExpanded(b, _)) => a == b,
            _ => false,
        }
    }
}

impl ApplicationState {
    pub(crate) fn new(data_root: Option<PathBuf>, enabled: bool, cx: &mut Context<Self>) -> Self {
        let options = data_root.map_or_else(ServiceOptions::new, |root| {
            ServiceOptions::new().with_data_root(root)
        });
        let mut this = Self {
            handle: None,
            confirmed: None,
            error: None,
            starting: false,
            preferences_pending: false,
            options,
            subscription: None,
            preference_edits: Vec::new(),
            draft_edits: BTreeMap::new(),
            drafts_in_flight: BTreeSet::new(),
            draft_failures: BTreeMap::new(),
            quit_requested: false,
            shutdown_started: false,
        };
        if enabled {
            this.start(cx);
        }
        this
    }

    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        if self.starting || self.handle.is_some() {
            return;
        }
        self.starting = true;
        self.error = None;
        let options = self.options.clone();
        self.subscription =
            Some(cx.spawn(
                async move |this, cx| match ApplicationService::start(options).await {
                    Ok(handle) => {
                        let mut updates = handle.subscribe();
                        let current = updates.current();
                        if this
                            .update(cx, |state, cx| {
                                state.handle = Some(handle);
                                state.starting = false;
                                state.confirmed = None;
                                state.publish(current, cx);
                                state.maybe_shutdown(cx);
                            })
                            .is_err()
                        {
                            return;
                        }
                        loop {
                            match updates.changed().await {
                                Ok(snapshot) => {
                                    if this
                                        .update(cx, |state, cx| state.publish(snapshot, cx))
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                Err(error) => {
                                    let _ = this.update(cx, |state, cx| {
                                        if !state.quit_requested {
                                            state.handle = None;
                                            state.error = Some(error.message);
                                            cx.notify();
                                        }
                                    });
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = this.update(cx, |state, cx| {
                            state.starting = false;
                            state.error = Some(error.message);
                            cx.notify();
                            state.maybe_shutdown(cx);
                        });
                    }
                },
            ));
        cx.notify();
    }

    fn publish(&mut self, snapshot: Arc<ServiceSnapshot>, cx: &mut Context<Self>) {
        if self
            .confirmed
            .as_ref()
            .is_some_and(|old| old.revision > snapshot.revision)
        {
            return;
        }
        let previous_zoom = self.confirmed.as_ref().map(|s| s.preferences.zoom_percent);
        if previous_zoom != Some(snapshot.preferences.zoom_percent) {
            theme::apply_zoom_percent(snapshot.preferences.zoom_percent, cx);
        }
        self.confirmed = Some(snapshot);
        cx.notify();
    }

    pub(crate) fn read_latest(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.handle {
            self.publish(handle.snapshot(), cx);
        }
    }

    pub(crate) fn save_preference(&mut self, edit: PreferenceEdit, cx: &mut Context<Self>) {
        if self.quit_requested {
            return;
        }
        self.preference_edits.retain(|old| !old.same_target(&edit));
        self.preference_edits.push(edit);
        self.flush_preferences(cx);
    }

    fn flush_preferences(&mut self, cx: &mut Context<Self>) {
        if self.preferences_pending || self.preference_edits.is_empty() {
            return;
        }
        let (Some(handle), Some(snapshot)) = (self.handle.clone(), self.confirmed.clone()) else {
            return;
        };
        let mut preferences = snapshot.preferences.clone();
        for edit in self.preference_edits.drain(..) {
            edit.apply(&mut preferences);
        }
        if preferences == snapshot.preferences {
            return;
        }
        self.preferences_pending = true;
        let future = handle.save_preferences(snapshot.preferences_revision, preferences);
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let _ = this.update(cx, |state, cx| {
                state.preferences_pending = false;
                state.publish(handle.snapshot(), cx);
                if let Err(error) = result {
                    state.error = Some(format!("设置未保存：{}", error.message));
                } else {
                    state.error = None;
                }
                state.flush_preferences(cx);
                state.maybe_shutdown(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn select_session(&mut self, id: Option<SessionId>, cx: &mut Context<Self>) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let future = handle.select_session(id);
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let _ = this.update(cx, |state, cx| {
                state.publish(handle.snapshot(), cx);
                state.error = result.err().map(|e| e.message);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn draft_text(&self, id: SessionId) -> Option<&str> {
        self.draft_edits.get(&id).map(String::as_str)
    }

    pub(crate) fn draft_error(&self, id: SessionId) -> Option<&str> {
        self.draft_failures.get(&id).map(String::as_str)
    }

    pub(crate) fn save_draft(&mut self, id: SessionId, text: String, cx: &mut Context<Self>) {
        if self.quit_requested {
            return;
        }
        self.draft_edits.insert(id, text);
        self.draft_failures.remove(&id);
        self.flush_draft(id, cx);
    }

    fn flush_draft(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if self.drafts_in_flight.contains(&id) || self.draft_failures.contains_key(&id) {
            return;
        }
        let (Some(handle), Some(text)) = (self.handle.clone(), self.draft_edits.get(&id).cloned())
        else {
            return;
        };
        self.drafts_in_flight.insert(id);
        cx.spawn(async move |this, cx| {
            let result = async {
                let reply = handle.query_session(id).await?;
                let session = reply.snapshot.ok_or_else(|| {
                    BusinessError::new(
                        ErrorKind::NotFound,
                        "会话已不存在，草稿仍保留在当前窗口",
                        RetryAdvice::AfterCorrection,
                    )
                })?;
                if let Some(command) = draft_command(&session, text.clone())? {
                    let receipt = handle.execute(command).await?;
                    if let CommandStatus::Rejected { error } = receipt.result {
                        return Err(error);
                    }
                }
                Ok::<_, BusinessError>(())
            }
            .await;
            let _ = this.update(cx, |state, cx| {
                state.drafts_in_flight.remove(&id);
                state.publish(handle.snapshot(), cx);
                match result {
                    Ok(()) => {
                        if state.draft_edits.get(&id) == Some(&text) {
                            state.draft_edits.remove(&id);
                        }
                        state.flush_draft(id, cx);
                    }
                    Err(error) => {
                        state
                            .draft_failures
                            .insert(id, format!("草稿未保存：{}", error.message));
                    }
                }
                state.maybe_shutdown(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn request_quit(&mut self, cx: &mut Context<Self>) {
        self.quit_requested = true;
        self.maybe_shutdown(cx);
    }

    fn maybe_shutdown(&mut self, cx: &mut Context<Self>) {
        if !self.quit_requested
            || self.shutdown_started
            || self.starting
            || self.preferences_pending
            || !self.drafts_in_flight.is_empty()
        {
            return;
        }
        self.shutdown_started = true;
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            if let Some(handle) = handle {
                if let Err(error) = handle.shutdown().await {
                    let _ = this.update(cx, |state, cx| {
                        state.error = Some(error.message);
                        cx.notify();
                    });
                }
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}
