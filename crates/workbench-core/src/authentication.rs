//! Transient authentication discovery/actions. These types deliberately do not implement
//! serde traits and must never enter Command, Event, JournalRecord or StoragePort.
//!
//! ```compile_fail
//! use workbench_core::AuthenticationRequest;
//! fn persist(request: &AuthenticationRequest) {
//!     serde_json::to_string(request).unwrap();
//! }
//! ```
//! ```compile_fail
//! use workbench_core::AuthenticationNotice;
//! fn persist(notice: &AuthenticationNotice) {
//!     serde_json::to_string(notice).unwrap();
//! }
//! ```

use crate::{AgentId, ConnectionEpoch, PortFuture, RequestId, SessionId};

/// Actual live authentication channel, with no invented external session or RunId.
/// Agent-only epochs are allocated for that Agent channel; session epochs use its connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthenticationScope {
    agent_id: AgentId,
    session_id: Option<SessionId>,
    epoch: ConnectionEpoch,
}

impl AuthenticationScope {
    pub const fn agent(agent_id: AgentId, epoch: ConnectionEpoch) -> Self {
        Self {
            agent_id,
            session_id: None,
            epoch,
        }
    }
    pub const fn session(agent_id: AgentId, session_id: SessionId, epoch: ConnectionEpoch) -> Self {
        Self {
            agent_id,
            session_id: Some(session_id),
            epoch,
        }
    }
    pub const fn agent_id(self) -> AgentId {
        self.agent_id
    }
    pub const fn session_id(self) -> Option<SessionId> {
        self.session_id
    }
    pub const fn epoch(self) -> ConnectionEpoch {
        self.epoch
    }
}

/// Identifier offered by the Agent, not a hard-coded provider name or credential.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthenticationMethodId(String);
impl AuthenticationMethodId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationMethodKind {
    /// SDK authenticate(methodId); absent protocol type defaults to this kind.
    AgentManaged,
    /// Official terminal flow; must not be passed to SDK authenticate.
    Terminal,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationUnavailableReason {
    CapabilityUnknown,
    HostUnsupported,
    UnsupportedMethodKind,
    SecretEntryUnsupported,
    NoLiveConnection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationMethodAvailability {
    Available,
    Unavailable {
        reason: AuthenticationUnavailableReason,
    },
}

/// Public method metadata only. Runtime strips private _meta, terminal args/env and URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthenticationMethod {
    pub id: AuthenticationMethodId,
    pub name: String,
    pub description: Option<String>,
    pub kind: AuthenticationMethodKind,
    pub availability: AuthenticationMethodAvailability,
}
impl AuthenticationMethod {
    pub fn new(
        id: AuthenticationMethodId,
        name: impl Into<String>,
        kind: AuthenticationMethodKind,
        availability: AuthenticationMethodAvailability,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            description: None,
            kind,
            availability,
        }
    }
}

/// Runtime-owned interaction address. Not a URL, secret, SDK ID or reusable credential.
/// Runtime expires the handle on completion, withdrawal, cancellation or connection replacement.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthenticationInteractionId(RequestId);
impl AuthenticationInteractionId {
    pub fn new() -> Self {
        Self(RequestId::new())
    }
}
impl Default for AuthenticationInteractionId {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Debug for AuthenticationInteractionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthenticationInteractionId(..)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationInteractionKind {
    OfficialUrl,
    UnsupportedSecret,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthenticationInteraction {
    pub id: AuthenticationInteractionId,
    pub kind: AuthenticationInteractionKind,
    pub method_id: Option<AuthenticationMethodId>,
}
impl AuthenticationInteraction {
    pub fn new(id: AuthenticationInteractionId, kind: AuthenticationInteractionKind) -> Self {
        Self {
            id,
            kind,
            method_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationInteractionAction {
    /// Runtime opens its retained official URL, then responds accept on the original channel.
    OpenOfficialFlow,
    Decline,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthenticationAction {
    Discover,
    Authenticate {
        method_id: AuthenticationMethodId,
    },
    /// Cancels only the named authentication operation, never a chat Run.
    Cancel {
        request_id: RequestId,
    },
    Interact {
        interaction_id: AuthenticationInteractionId,
        action: AuthenticationInteractionAction,
    },
}

/// Local transient operation identity. Caller retains id/scope across delivery races.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthenticationRequest {
    pub id: RequestId,
    pub scope: AuthenticationScope,
    pub action: AuthenticationAction,
}
impl AuthenticationRequest {
    pub fn new(id: RequestId, scope: AuthenticationScope, action: AuthenticationAction) -> Self {
        Self { id, scope, action }
    }
}

/// Sanitized failure classes, never remote JSON, stderr, temporary URL or token text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationFailure {
    NoLiveConnection,
    StaleScope,
    MethodNotOffered,
    MethodUnavailable,
    InteractionExpired,
    ProviderRejected,
    Transport,
    Timeout,
    OfficialFlowFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthenticationStatus {
    Unknown,
    Discovering,
    Required,
    InProgress {
        method_id: AuthenticationMethodId,
    },
    /// Requires actual successful official authenticate/terminal result or verified login reuse.
    Authenticated,
    Cancelled,
    Failed {
        failure: AuthenticationFailure,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationInteractionEnd {
    Completed,
    Declined,
    Cancelled,
    Expired,
    ConnectionClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthenticationUpdate {
    /// Discovery alone says neither Required nor Authenticated.
    Methods {
        methods: Vec<AuthenticationMethod>,
    },
    Status {
        status: AuthenticationStatus,
    },
    InteractionRequired {
        interaction: AuthenticationInteraction,
    },
    InteractionClosed {
        interaction_id: AuthenticationInteractionId,
        reason: AuthenticationInteractionEnd,
    },
}

/// Typed in-memory UI notification. request_id links an auth operation if reliably known.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthenticationNotice {
    pub scope: AuthenticationScope,
    pub request_id: Option<RequestId>,
    pub update: AuthenticationUpdate,
}
impl AuthenticationNotice {
    pub fn new(scope: AuthenticationScope, update: AuthenticationUpdate) -> Self {
        Self {
            scope,
            request_id: None,
            update,
        }
    }
    pub fn with_request(mut self, id: RequestId) -> Self {
        self.request_id = Some(id);
        self
    }
}

/// App receives notices directly in transient UI state; never forwards them to the journal sink.
pub trait AuthenticationNoticeSink {
    fn emit_authentication(&self, notice: AuthenticationNotice) -> PortFuture<'_, ()>;
}

/// Optional, independent Runtime extension; existing RuntimePort/StoragePort remain unchanged.
/// Runtime owns method mapping, credential/URL/secret buffers and original responders.
/// Check full scope, active operation and method/interaction identity before acting. Never
/// wait for user input in an SDK dispatch callback; keep control/withdrawal messages flowing.
/// Errors returned through PortFuture must also be sanitized, with no remote secret payload.
pub trait AuthenticationPort {
    fn execute_authentication<'a>(
        &'a self,
        request: AuthenticationRequest,
        sink: &'a dyn AuthenticationNoticeSink,
    ) -> PortFuture<'a, ()>;
}
