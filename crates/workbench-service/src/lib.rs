//! Application-owned I/O and delivery. UI holds only a handle and confirmed projections.
mod actor;
mod api;
mod bridge;
pub use agent_catalog::{
    CatalogError, CatalogErrorKind, CatalogSnapshot, InstallCommand, InstallOutcome, InstallPlan,
    InstallPlanId, InstallPrerequisite, InstallState, Installability, LocalCandidate,
    LocalDiscovery, ManualLaunch, ManualValidation, NativeTool, RefreshPolicy, RegistryAgent,
    RegistryEvidence, RegistryState, RuntimeCompatibility, ToolchainDiscovery,
};
pub use api::*;
pub use workbench_core::{
    AgentDefinition, AgentId, AgentLaunchConfiguration, AuthenticationAction,
    AuthenticationFailure, AuthenticationInteraction, AuthenticationInteractionAction,
    AuthenticationInteractionId, AuthenticationInteractionKind, AuthenticationMethod,
    AuthenticationMethodAvailability, AuthenticationMethodId, AuthenticationMethodKind,
    AuthenticationNotice, AuthenticationScope, AuthenticationStatus, BusinessError,
    CommandEnvelope, Project, ProjectId, RequestId, SessionId, SessionSnapshot, Timestamp,
    WorkbenchSnapshot, WorkspacePreferences,
};
