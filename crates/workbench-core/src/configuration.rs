use serde::{Deserialize, Serialize};

use crate::{AgentOptionId, ConnectionEpoch, RequestId, RunId, Timestamp};

/// User intent owned by core; a default full-access request is not an Agent option ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionSelection {
    DefaultFullAccess,
    AgentOption { id: AgentOptionId },
}

/// Configuration intent captured in drafts, queued input and submission snapshots.
/// `None` means no explicit model/effort request; never infer a confirmed value from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestedConfiguration {
    pub model: Option<AgentOptionId>,
    pub reasoning_effort: Option<AgentOptionId>,
    pub permission: PermissionSelection,
}

impl Default for RequestedConfiguration {
    fn default() -> Self {
        Self {
            model: None,
            reasoning_effort: None,
            permission: PermissionSelection::DefaultFullAccess,
        }
    }
}

/// Confirmed values supplied by the Agent. Missing fields remain unknown/unsupported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EffectiveConfiguration {
    pub epoch: ConnectionEpoch,
    pub capability_revision: u64,
    pub confirmed_at: Timestamp,
    pub model: Option<AgentOptionId>,
    pub reasoning_effort: Option<AgentOptionId>,
    pub permission: Option<AgentOptionId>,
}

impl EffectiveConfiguration {
    pub fn new(epoch: ConnectionEpoch, capability_revision: u64, confirmed_at: Timestamp) -> Self {
        Self {
            epoch,
            capability_revision,
            confirmed_at,
            model: None,
            reasoning_effort: None,
            permission: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigurationApplyTiming {
    AwaitingAgent,
    AfterRun { run_id: RunId },
    NextRun,
    AgentReported { description: String },
}

/// Snapshot already sent for confirmation; newer user intent may differ from this value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PendingConfiguration {
    pub request_id: RequestId,
    pub epoch: ConnectionEpoch,
    pub requested_at: Timestamp,
    pub requested: RequestedConfiguration,
    pub timing: ConfigurationApplyTiming,
}

impl PendingConfiguration {
    pub fn new(
        request_id: RequestId,
        epoch: ConnectionEpoch,
        requested_at: Timestamp,
        requested: RequestedConfiguration,
    ) -> Self {
        Self {
            request_id,
            epoch,
            requested_at,
            requested,
            timing: ConfigurationApplyTiming::AwaitingAgent,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConfigurationRejection {
    pub request_id: RequestId,
    pub epoch: ConnectionEpoch,
    pub requested: RequestedConfiguration,
    pub reason: String,
    pub rejected_at: Timestamp,
}

impl ConfigurationRejection {
    pub fn new(
        pending: PendingConfiguration,
        rejected_at: Timestamp,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            request_id: pending.request_id,
            epoch: pending.epoch,
            requested: pending.requested,
            reason: reason.into(),
            rejected_at,
        }
    }
}

/// Session-owned intent, in-flight snapshot, and confirmed value are separate facts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionConfiguration {
    pub requested: RequestedConfiguration,
    pub pending: Option<PendingConfiguration>,
    pub effective: Option<EffectiveConfiguration>,
    pub last_rejection: Option<ConfigurationRejection>,
}
