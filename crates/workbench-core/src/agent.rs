use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{AgentCommandId, AgentId, AgentOptionId, ConnectionEpoch};

/// Agent catalog record owned by discovery/registry. It contains no credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentDefinition {
    pub id: AgentId,
    pub name: String,
    pub source: AgentSource,
    pub distributions: Vec<AgentDistribution>,
    pub discovery: AgentDiscovery,
}

impl AgentDefinition {
    pub fn new(id: AgentId, name: impl Into<String>, source: AgentSource) -> Self {
        Self {
            id,
            name: name.into(),
            source,
            distributions: Vec::new(),
            discovery: AgentDiscovery::NotChecked,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentSource {
    Registry { registry_id: String },
    LocalConfiguration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentDistribution {
    pub id: String,
    pub version: String,
    /// Platform identifiers supplied by discovery, such as `windows-x86_64`.
    pub platforms: Vec<String>,
    /// Descriptive source only; not an install instruction executed by core.
    pub source: String,
}

impl AgentDistribution {
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            platforms: Vec::new(),
            source: source.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentDiscovery {
    NotChecked,
    NotInstalled,
    Installed {
        executable: PathBuf,
        version: Option<String>,
        distribution_id: Option<String>,
    },
    Incompatible {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

/// An unnegotiated capability is different from a confirmed unsupported capability.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapabilitySupport {
    #[default]
    Unknown,
    Unsupported {
        reason: Option<String>,
    },
    Supported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CapabilityOption {
    pub id: AgentOptionId,
    pub label: String,
}

impl CapabilityOption {
    pub fn new(id: AgentOptionId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelCapability {
    pub option: CapabilityOption,
    /// Effort options belong to this model, never to a hard-coded global list.
    pub reasoning_efforts: Vec<CapabilityOption>,
}

impl ModelCapability {
    pub fn new(option: CapabilityOption) -> Self {
        Self {
            option,
            reasoning_efforts: Vec::new(),
        }
    }
}

/// Meaning reported by the adapter; the option ID remains Agent-provided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    FullAccess,
    Restricted,
    AgentDefined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionCapability {
    pub option: CapabilityOption,
    pub mode: AccessMode,
}

impl PermissionCapability {
    pub fn new(option: CapabilityOption, mode: AccessMode) -> Self {
        Self { option, mode }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentCommand {
    pub id: AgentCommandId,
    pub name: String,
    pub description: Option<String>,
}

impl AgentCommand {
    pub fn new(id: AgentCommandId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            description: None,
        }
    }
}

/// Limits are reported by the Agent; `None` means unknown, not unlimited.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AttachmentCapabilities {
    pub images: CapabilitySupport,
    pub files: CapabilitySupport,
    pub media_types: Vec<String>,
    pub max_bytes_per_attachment: Option<u64>,
    pub max_attachments_per_input: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionCapabilities {
    pub text: CapabilitySupport,
    pub single_choice: CapabilitySupport,
    pub multiple_choice: CapabilitySupport,
    pub forms: CapabilitySupport,
}

/// Runtime-owned confirmed capabilities for one session connection and model selection.
/// Refresh after model changes; consumers must check epoch/revision before sending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentCapabilities {
    pub epoch: ConnectionEpoch,
    pub revision: u64,
    pub models: Vec<ModelCapability>,
    pub permission_modes: Vec<PermissionCapability>,
    pub commands: Vec<AgentCommand>,
    pub attachments: AttachmentCapabilities,
    pub questions: QuestionCapabilities,
    pub steering: CapabilitySupport,
    pub session_resume: CapabilitySupport,
    /// Reattaching an active run is separate from opening its session again.
    pub active_run_recovery: CapabilitySupport,
    pub configuration_during_run: CapabilitySupport,
}

impl AgentCapabilities {
    pub fn new(epoch: ConnectionEpoch, revision: u64) -> Self {
        Self {
            epoch,
            revision,
            models: Vec::new(),
            permission_modes: Vec::new(),
            commands: Vec::new(),
            attachments: AttachmentCapabilities::default(),
            questions: QuestionCapabilities::default(),
            steering: CapabilitySupport::Unknown,
            session_resume: CapabilitySupport::Unknown,
            active_run_recovery: CapabilitySupport::Unknown,
            configuration_during_run: CapabilitySupport::Unknown,
        }
    }
}
