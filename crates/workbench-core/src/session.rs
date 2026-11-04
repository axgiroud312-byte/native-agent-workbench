use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    AgentCapabilities, AgentId, ConnectionEpoch, ConnectionState, ExecutionState, ExternalRunId,
    ExternalSessionId, HistoryAvailability, ProjectId, RunContext, RunId, RunRecoveryState,
    SessionConfiguration, SessionId, SessionResumeState, Timestamp,
};

/// Project index owned by core/storage. Removing it never deletes its directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub directory: PathBuf,
    pub created_at: Timestamp,
}

impl Project {
    pub fn new(
        id: ProjectId,
        name: impl Into<String>,
        directory: PathBuf,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            directory,
            created_at,
        }
    }
}

/// There is deliberately no imported/external-history origin in this version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOrigin {
    WorkbenchCreated,
}

/// Registered only after creating an external session for this local session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExternalSessionBinding {
    pub id: ExternalSessionId,
    pub registered_at: Timestamp,
}

impl ExternalSessionBinding {
    pub fn new(id: ExternalSessionId, registered_at: Timestamp) -> Self {
        Self { id, registered_at }
    }
}

/// Core-owned session identity. Project and Agent bindings cannot be changed in place.
/// Selecting a different Agent creates a new SessionRecord. No focus/scroll state lives here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionRecord {
    id: SessionId,
    project_id: ProjectId,
    agent_id: AgentId,
    origin: SessionOrigin,
    pub title: String,
    pub created_at: Timestamp,
    pub external: Option<ExternalSessionBinding>,
    pub connection: ConnectionState,
    /// None is idle. Execution details have one owner: the referenced Run.
    pub current_run_id: Option<RunId>,
    pub configuration: SessionConfiguration,
    pub capabilities: Option<AgentCapabilities>,
    pub history: HistoryAvailability,
    pub resume: SessionResumeState,
}

impl SessionRecord {
    pub fn new(
        id: SessionId,
        project_id: ProjectId,
        agent_id: AgentId,
        title: impl Into<String>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            project_id,
            agent_id,
            origin: SessionOrigin::WorkbenchCreated,
            title: title.into(),
            created_at,
            external: None,
            connection: ConnectionState::default(),
            current_run_id: None,
            configuration: SessionConfiguration::default(),
            capabilities: None,
            history: HistoryAvailability::Readable,
            resume: SessionResumeState::NotChecked,
        }
    }

    pub const fn id(&self) -> SessionId {
        self.id
    }
    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }
    pub const fn origin(&self) -> SessionOrigin {
        self.origin
    }
}

/// One execution, with its original dispatch epoch retained even after reconnection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Run {
    pub context: RunContext,
    pub epoch: ConnectionEpoch,
    pub external_id: Option<ExternalRunId>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub state: ExecutionState,
    pub recovery: RunRecoveryState,
    /// Frozen at dispatch; later session configuration does not rewrite this run.
    #[serde(default)]
    pub dispatched_configuration: Option<crate::EffectiveConfiguration>,
}

impl Run {
    pub fn new(context: RunContext, epoch: ConnectionEpoch, created_at: Timestamp) -> Self {
        Self {
            context,
            epoch,
            external_id: None,
            created_at,
            started_at: None,
            state: ExecutionState::Starting,
            recovery: RunRecoveryState::NotAttempted,
            dispatched_configuration: None,
        }
    }
}
