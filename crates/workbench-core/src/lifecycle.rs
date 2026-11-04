use serde::{Deserialize, Serialize};

use crate::{ConnectionEpoch, RequestId, Timestamp};

/// Connection facts never stand in for run completion or pending interactions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionPhase {
    NotStarted,
    Starting,
    Authenticating,
    Ready,
    Disconnected { reason: String },
    Failed { reason: String },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConnectionState {
    pub epoch: ConnectionEpoch,
    pub phase: ConnectionPhase,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            epoch: ConnectionEpoch::INITIAL,
            phase: ConnectionPhase::NotStarted,
        }
    }
}

/// A confirmed end reason reported by the Agent, not inferred from transport closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunResult {
    EndOfTurn,
    LimitReached { reason: String },
    AgentReported { reason: String },
}

/// The run owner updates these facts. Requesting cancellation is not confirmation.
/// Interrupted and unknown outcomes prohibit automatic queue dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionState {
    Starting,
    Running,
    Cancelling {
        request_id: RequestId,
        requested_at: Timestamp,
    },
    Completed {
        ended_at: Timestamp,
        result: RunResult,
    },
    Cancelled {
        ended_at: Timestamp,
    },
    Failed {
        ended_at: Timestamp,
        reason: String,
    },
    Interrupted {
        observed_at: Timestamp,
        reason: String,
    },
    OutcomeUnknown {
        observed_at: Timestamp,
        reason: String,
    },
}

impl ExecutionState {
    /// Only an authoritative end makes a run terminal. It does not authorize queue dispatch.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Cancelled { .. } | Self::Failed { .. }
        )
    }
}

/// Local history can be readable even when no Agent session/run can be resumed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryAvailability {
    Readable,
    Partial { reason: String },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionResumeState {
    #[default]
    NotChecked,
    Unsupported {
        reason: String,
    },
    Available,
    Resuming {
        epoch: ConnectionEpoch,
    },
    Resumed {
        epoch: ConnectionEpoch,
    },
    Failed {
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunRecoveryState {
    #[default]
    NotAttempted,
    Checking {
        epoch: ConnectionEpoch,
    },
    Recovered {
        epoch: ConnectionEpoch,
    },
    Unavailable {
        reason: String,
    },
}
