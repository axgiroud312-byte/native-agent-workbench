use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Durable view preferences, deliberately outside session business state and the reducer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WorkspacePreferences {
    pub sidebar_width: u16,
    pub zoom_percent: u16,
    pub selected_project_id: Option<ProjectId>,
    pub selected_session_id: Option<SessionId>,
    pub expanded_projects: BTreeSet<ProjectId>,
    pub expanded_tools: BTreeSet<ToolCallId>,
    pub expanded_changes: BTreeSet<FileChangeId>,
}
impl Default for WorkspacePreferences {
    fn default() -> Self {
        Self {
            sidebar_width: 264,
            zoom_percent: 100,
            selected_project_id: None,
            selected_session_id: None,
            expanded_projects: BTreeSet::new(),
            expanded_tools: BTreeSet::new(),
            expanded_changes: BTreeSet::new(),
        }
    }
}
