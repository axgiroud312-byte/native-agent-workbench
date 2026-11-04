use crate::fixture::{FixturePreset, build_fixture};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use workbench_core::{
    ApplyDisposition, ApplyMode, BusinessError, Command, CommandEnvelope, CommandId, ComposerDraft,
    ErrorKind, InputPart, JournalEntry, JournalRecord, ProjectId, RetryAdvice, Scope, SessionId,
    SessionSnapshot, Timestamp, ToolCallId, WorkbenchSnapshot, WorkbenchState,
    WorkspacePreferences, reduce,
};

/// Core commands and view preferences remain separate at the future service seam.
pub enum UiIntent {
    Command(CommandEnvelope),
    Preferences(WorkspacePreferences),
}
struct FixtureSource {
    state: WorkbenchState,
    preset: FixturePreset,
}

/// Normal launch has only an empty committed snapshot. A picked directory is view context,
/// never a manufactured Project record. Only the explicit fixture source runs a local reducer.
pub struct WorkbenchHost {
    snapshot: WorkbenchSnapshot,
    active_session: Option<SessionSnapshot>,
    preferences: WorkspacePreferences,
    fixture: Option<FixtureSource>,
    temporary_directory: Option<PathBuf>,
    error: Option<BusinessError>,
}
impl WorkbenchHost {
    pub fn new(preset: Option<FixturePreset>) -> Self {
        let mut host = Self {
            snapshot: WorkbenchState::new().snapshot(),
            active_session: None,
            preferences: WorkspacePreferences::default(),
            fixture: None,
            temporary_directory: None,
            error: None,
        };
        if let Some(preset) = preset {
            let (state, preferences) = build_fixture(preset);
            host.preferences = preferences;
            host.fixture = Some(FixtureSource { state, preset });
            host.refresh();
        }
        host
    }
    pub fn snapshot(&self) -> &WorkbenchSnapshot {
        &self.snapshot
    }
    pub fn active_session(&self) -> Option<&SessionSnapshot> {
        self.active_session.as_ref()
    }
    pub fn preferences(&self) -> &WorkspacePreferences {
        &self.preferences
    }
    pub fn active_preset(&self) -> Option<FixturePreset> {
        self.fixture.as_ref().map(|f| f.preset)
    }
    pub fn last_error(&self) -> Option<&BusinessError> {
        self.error.as_ref()
    }
    pub fn temporary_directory(&self) -> Option<&std::path::Path> {
        self.temporary_directory.as_deref()
    }
    pub fn set_temporary_directory(&mut self, path: PathBuf) {
        self.temporary_directory = Some(path);
    }
    pub fn selected_project(&self) -> Option<&workbench_core::Project> {
        let id = self.preferences.selected_project_id?;
        self.snapshot.projects.iter().find(|p| p.id == id)
    }
    pub fn agent_name(&self, id: workbench_core::AgentId) -> String {
        self.snapshot
            .agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "未知 Agent".into())
    }
    pub fn select_project(&mut self, id: ProjectId) {
        if !self.snapshot.projects.iter().any(|p| p.id == id) {
            return;
        }
        self.preferences.selected_project_id = Some(id);
        let current_belongs = self
            .snapshot
            .sessions
            .iter()
            .any(|s| Some(s.id) == self.preferences.selected_session_id && s.project_id == id);
        if !current_belongs {
            self.preferences.selected_session_id = self
                .snapshot
                .sessions
                .iter()
                .find(|s| s.project_id == id)
                .map(|s| s.id);
            self.refresh();
        }
    }
    pub fn select_session(&mut self, id: SessionId) {
        if let Some(row) = self.snapshot.sessions.iter().find(|s| s.id == id) {
            self.preferences.selected_session_id = Some(id);
            self.preferences.selected_project_id = Some(row.project_id);
            self.refresh();
        }
    }
    pub fn set_project_expanded(&mut self, id: ProjectId, expanded: bool) {
        if expanded {
            self.preferences.expanded_projects.insert(id);
        } else {
            self.preferences.expanded_projects.remove(&id);
        }
    }
    pub fn toggle_tool_expanded(&mut self, id: ToolCallId) {
        if !self.preferences.expanded_tools.remove(&id) {
            self.preferences.expanded_tools.insert(id);
        }
    }
    pub fn update_preferences(&mut self, preferences: WorkspacePreferences) {
        let _ = self.dispatch(UiIntent::Preferences(preferences));
    }
    pub fn save_draft_text(&mut self, text: String) {
        let Some(session) = &self.active_session else {
            return;
        };
        if draft_plain_text(&session.draft) == text {
            return;
        }
        let mut draft = session.draft.clone();
        let expected_revision = draft.revision;
        let Some(revision) = expected_revision.checked_add(1) else {
            return;
        };
        draft.revision = revision;
        draft.updated_at = now();
        draft
            .content
            .retain(|part| !matches!(part, InputPart::Text { .. }));
        if !text.is_empty() {
            draft.content.insert(0, InputPart::Text { text });
        }
        let scope = Scope::session(session.record.id(), session.record.connection.epoch);
        let command = CommandEnvelope::new(
            CommandId::new(),
            now(),
            scope,
            Command::SaveDraft {
                draft,
                expected_revision,
            },
        );
        if let Err(error) = self.dispatch(UiIntent::Command(command)) {
            self.error = Some(error);
        }
    }
    fn dispatch(&mut self, intent: UiIntent) -> Result<(), BusinessError> {
        match intent {
            UiIntent::Preferences(preferences) => {
                self.preferences = preferences;
                Ok(())
            }
            UiIntent::Command(command) => {
                let Some(source) = &mut self.fixture else {
                    return Err(BusinessError::new(
                        ErrorKind::NotReady,
                        "会话服务尚未连接",
                        RetryAdvice::Never,
                    ));
                };
                let sequence = source.state.sequence().checked_add(1).ok_or_else(|| {
                    BusinessError::new(ErrorKind::Exhausted, "样例顺序已耗尽", RetryAdvice::Never)
                })?;
                let record = JournalRecord::new(sequence, JournalEntry::Command(command));
                let transition = reduce(&source.state, &record, ApplyMode::Live)?;
                if !transition.effects().is_empty() {
                    return Err(BusinessError::new(
                        ErrorKind::NotReady,
                        "开发样例不执行 Agent 操作",
                        RetryAdvice::Never,
                    ));
                }
                if let ApplyDisposition::Rejected(error) = transition.disposition() {
                    return Err(error.clone());
                }
                source.state = transition.into_parts().0;
                self.error = None;
                self.refresh();
                Ok(())
            }
        }
    }
    fn refresh(&mut self) {
        if let Some(source) = &self.fixture {
            self.snapshot = source.state.snapshot();
            self.active_session = self
                .preferences
                .selected_session_id
                .and_then(|id| source.state.session_snapshot(id));
        }
    }
}
fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Timestamp::new(millis.min(i64::MAX as u128) as i64)
}
pub fn draft_plain_text(draft: &ComposerDraft) -> String {
    draft
        .content
        .iter()
        .filter_map(|part| match part {
            InputPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
