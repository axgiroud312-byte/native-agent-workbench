use std::path::PathBuf;
use workbench_core::{
    AgentDefinition, AgentDiscovery, AgentId, AgentLaunchConfiguration, RequestId, Timestamp,
};

pub const OFFICIAL_REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
pub const CODEX_REGISTRY_ID: &str = "codex-acp";
pub const CODEX_ADAPTER_VERSION: &str = "2.5.0";
pub const CODEX_NATIVE_VERSION: &str = "0.166.0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogErrorKind {
    Network,
    InvalidRegistry,
    Cache,
    NotFound,
    Unsupported,
    InvalidLaunch,
    MissingTool,
    InvalidPlan,
    InstallInProgress,
    InstallFailed,
    /// Termination was attempted but owned-tree release could not be proved. Target remains locked.
    CleanupUnconfirmed,
    Timeout,
    Io,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CatalogError {
    pub kind: CatalogErrorKind,
    pub message: String,
    pub retryable: bool,
}
impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CatalogError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshPolicy {
    PreferCache,
    Revalidate,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryState {
    Live,
    Cached { stale: bool, offline: bool },
    Unavailable { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegistryEvidence {
    pub source_url: String,
    pub format_version: Option<String>,
    pub fetched_at: Option<Timestamp>,
    pub checked_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub state: RegistryState,
    pub problem: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installability {
    Supported,
    Unavailable { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegistryAgent {
    pub registry_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub windows_distributions: Vec<String>,
    pub installation: Installability,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionEvidence {
    ExecutedVersion,
    PackageManifest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeTool {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub version: String,
    pub evidence: VersionEvidence,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolchainDiscovery {
    pub node: Option<NativeTool>,
    pub npm: Option<NativeTool>,
    pub codex: Option<NativeTool>,
    pub issues: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeCompatibility {
    SupportedCodex,
    Incompatible { reason: String },
}
/// Discovery evidence has no business AgentId. WorkbenchService assigns/persists IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LocalCandidate {
    pub key: String,
    pub name: String,
    pub registry_id: Option<String>,
    pub launch: AgentLaunchConfiguration,
    pub adapter_version: Option<String>,
    pub native_agent: Option<NativeTool>,
    pub compatibility: RuntimeCompatibility,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LocalDiscovery {
    pub tools: ToolchainDiscovery,
    pub configured: Vec<AgentDefinition>,
    pub candidates: Vec<LocalCandidate>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CatalogSnapshot {
    pub registry: RegistryEvidence,
    pub available: Vec<RegistryAgent>,
    pub local: LocalDiscovery,
}
impl CatalogSnapshot {
    /// Publishes local discovery without fetching or claiming any Registry refresh.
    pub fn local_only(local: LocalDiscovery, at: Timestamp) -> Self {
        Self {
            registry: RegistryEvidence {
                source_url: OFFICIAL_REGISTRY_URL.into(),
                format_version: None,
                fetched_at: None,
                checked_at: at,
                expires_at: None,
                state: RegistryState::Unavailable {
                    reason: "Registry not refreshed".into(),
                },
                problem: None,
            },
            available: Vec::new(),
            local,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstallPlanId(pub(crate) RequestId);
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstallPrerequisite {
    pub name: String,
    pub satisfied: bool,
    pub detail: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstallCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstallPlan {
    pub id: InstallPlanId,
    pub agent_id: AgentId,
    pub registry_id: String,
    pub source_url: String,
    pub adapter_version: String,
    pub native_agent_version: String,
    pub target_directory: PathBuf,
    pub command: InstallCommand,
    pub prerequisites: Vec<InstallPrerequisite>,
    pub created_at: Timestamp,
    pub native_agent_source_url: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    Prepared,
    Running,
    Succeeded,
    Failed { error: CatalogError },
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct InstallOutcome {
    pub plan_id: InstallPlanId,
    pub definition: AgentDefinition,
    pub reused_completed_result: bool,
}

/// No environment or credential fields. Native Node + adapter JS is the supported shape.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ManualLaunch {
    pub executable: PathBuf,
    pub args: Vec<String>,
}
impl ManualLaunch {
    pub fn new(executable: PathBuf, args: Vec<String>) -> Self {
        Self { executable, args }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ManualValidation {
    pub definition: AgentDefinition,
    pub compatibility: RuntimeCompatibility,
    pub native_agent: Option<NativeTool>,
    pub discovery: AgentDiscovery,
}
