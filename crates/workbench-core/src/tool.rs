use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ConnectionEpoch, ContentBlock, ExternalToolCallId, FileChangeId, PlanId, PlanStepId,
    RunContext, Timestamp, ToolCallId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolState {
    Pending,
    Running,
    Completed {
        ended_at: Timestamp,
    },
    Failed {
        ended_at: Timestamp,
        reason: String,
    },
    Cancelled {
        ended_at: Timestamp,
    },
    Interrupted {
        observed_at: Timestamp,
        reason: String,
    },
}

/// Run-owned tool facts supplied by runtime; arguments/results retain structured values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCall {
    pub id: ToolCallId,
    pub context: RunContext,
    pub epoch: ConnectionEpoch,
    pub external_id: Option<ExternalToolCallId>,
    pub name: String,
    pub summary: Option<String>,
    pub created_at: Timestamp,
    pub state: ToolState,
    pub arguments: Option<Value>,
    pub result: Option<Value>,
    pub output: Vec<ContentBlock>,
}

impl ToolCall {
    pub fn new(
        id: ToolCallId,
        context: RunContext,
        epoch: ConnectionEpoch,
        name: impl Into<String>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            context,
            epoch,
            external_id: None,
            name: name.into(),
            summary: None,
            created_at,
            state: ToolState::Pending,
            arguments: None,
            result: None,
            output: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepState {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlanStep {
    pub id: PlanStepId,
    pub text: String,
    pub state: PlanStepState,
}

impl PlanStep {
    pub fn new(id: PlanStepId, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            state: PlanStepState::Pending,
        }
    }
}

/// An actual Agent plan. Plain text mentioning a plan remains a text block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Plan {
    pub id: PlanId,
    pub context: RunContext,
    pub epoch: ConnectionEpoch,
    pub updated_at: Timestamp,
    pub steps: Vec<PlanStep>,
}

impl Plan {
    pub fn new(
        id: PlanId,
        context: RunContext,
        epoch: ConnectionEpoch,
        updated_at: Timestamp,
    ) -> Self {
        Self {
            id,
            context,
            epoch,
            updated_at,
            steps: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed { previous_path: PathBuf },
}

/// Explicit provenance prevents assigning a shared-directory Git diff to one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileChangeSource {
    AgentReport { tool_call_id: Option<ToolCallId> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileDiff {
    Unified { text: String },
    Binary,
    Unavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FileChange {
    pub id: FileChangeId,
    pub context: RunContext,
    pub epoch: ConnectionEpoch,
    pub reported_at: Timestamp,
    pub source: FileChangeSource,
    pub path: PathBuf,
    pub kind: FileChangeKind,
    /// Unknown counts stay None, rather than claiming zero changes.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub diff: FileDiff,
}

impl FileChange {
    pub fn new(
        id: FileChangeId,
        context: RunContext,
        epoch: ConnectionEpoch,
        reported_at: Timestamp,
        path: PathBuf,
        kind: FileChangeKind,
        source: FileChangeSource,
    ) -> Self {
        Self {
            id,
            context,
            epoch,
            reported_at,
            source,
            path,
            kind,
            additions: None,
            deletions: None,
            diff: FileDiff::Unavailable {
                reason: "Agent has not supplied a diff".into(),
            },
        }
    }
}
