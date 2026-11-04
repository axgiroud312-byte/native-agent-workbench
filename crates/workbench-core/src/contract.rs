use crate::*;
use serde::{Deserialize, Serialize};

/// Typed retry evidence. Unknown delivery is never safe to resend automatically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAdvice {
    Never,
    AfterCorrection,
    AfterReconnect,
    ReconcileFirst,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotFound,
    Conflict,
    InvalidInput,
    StaleIdentity,
    CapabilityUnknown,
    Unsupported,
    NotReady,
    Busy,
    RequestExpired,
    DeliveryUnknown,
    Protocol,
    Authentication,
    Transport,
    Storage,
    SequenceConflict,
    Exhausted,
    RemoteExecution,
    Rejected,
    ResourceUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BusinessError {
    pub kind: ErrorKind,
    pub message: String,
    pub retry: RetryAdvice,
}

impl BusinessError {
    pub fn new(kind: ErrorKind, message: impl Into<String>, retry: RetryAdvice) -> Self {
        Self {
            kind,
            message: message.into(),
            retry,
        }
    }
    pub(crate) fn invalid(message: &str) -> Self {
        Self::new(
            ErrorKind::InvalidInput,
            message,
            RetryAdvice::AfterCorrection,
        )
    }
    pub(crate) fn stale() -> Self {
        Self::new(
            ErrorKind::StaleIdentity,
            "owner, run or connection generation no longer matches",
            RetryAdvice::Never,
        )
    }
    pub(crate) fn busy(message: &str) -> Self {
        Self::new(ErrorKind::Busy, message, RetryAdvice::AfterCorrection)
    }
}

impl std::fmt::Display for BusinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for BusinessError {}

/// Optional run/request supports connection, capabilities and configuration session events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Scope {
    pub agent_id: Option<AgentId>,
    pub session_id: Option<SessionId>,
    pub run_id: Option<RunId>,
    pub request_id: Option<RequestId>,
    pub epoch: Option<ConnectionEpoch>,
}
impl Scope {
    pub fn session(id: SessionId, epoch: ConnectionEpoch) -> Self {
        Self {
            session_id: Some(id),
            epoch: Some(epoch),
            ..Self::default()
        }
    }
    pub fn with_run(mut self, id: RunId) -> Self {
        self.run_id = Some(id);
        self
    }
    pub fn with_request(mut self, id: RequestId) -> Self {
        self.request_id = Some(id);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    PutProject {
        project: Project,
    },
    PutAgent {
        agent: AgentDefinition,
    },
    CreateSession {
        session: SessionRecord,
    },
    RenameSession {
        title: String,
    },
    Connect {
        resume: bool,
    },
    CloseSession,
    ReconcileClosingResources {
        abandoned_epoch: ConnectionEpoch,
    },
    DeleteSession,
    DeleteProject {
        project_id: ProjectId,
    },
    SaveDraft {
        draft: ComposerDraft,
        expected_revision: u64,
    },
    PutAttachment {
        attachment: Attachment,
    },
    Enqueue {
        input: QueuedInput,
    },
    EditQueued {
        id: QueuedInputId,
        content: Vec<InputPart>,
        configuration: RequestedConfiguration,
    },
    SetQueueEditing {
        id: QueuedInputId,
        editing: bool,
    },
    CancelQueued {
        id: QueuedInputId,
    },
    PauseQueue,
    ResumeQueue,
    Submit {
        submission: InputSubmission,
        attempt_id: AttemptId,
    },
    /// Same logical submission, new attempt and local run; only after explicit non-consumption.
    FallbackToPrompt {
        submission_id: SubmissionId,
        attempt_id: AttemptId,
        run_id: RunId,
    },
    Stop,
    ReconcileSubmission {
        submission_id: SubmissionId,
        attempt_id: AttemptId,
        attempt_epoch: ConnectionEpoch,
    },
    Configure {
        requested: RequestedConfiguration,
        capability_revision: u64,
    },
    RespondPermission {
        response: PermissionResponse,
        attempt_id: ResponseAttemptId,
    },
    RespondQuestion {
        response: QuestionResponse,
        attempt_id: ResponseAttemptId,
    },
    /// Must be journaled after replay, before exposing restored state to live operations.
    Restart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandEnvelope {
    pub version: ModelVersion,
    pub id: CommandId,
    pub issued_at: Timestamp,
    pub scope: Scope,
    pub command: Command,
}
impl CommandEnvelope {
    pub fn new(id: CommandId, issued_at: Timestamp, scope: Scope, command: Command) -> Self {
        Self {
            version: ModelVersion::CURRENT,
            id,
            issued_at,
            scope,
            command,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandStatus {
    Applied,
    Rejected { error: BusinessError },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommandResult {
    pub id: CommandId,
    pub status: CommandStatus,
}

/// An opaque cursor exists only when the remote source actually supplies a reliable token.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RemoteCursor {
    pub stream: String,
    pub value: String,
}
impl RemoteCursor {
    pub fn new(stream: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            stream: stream.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventOrigin {
    Host,
    Runtime { adapter: String, version: String },
    AgentReplay { adapter: String, version: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EventSource {
    pub id: EventSourceId,
    pub origin: EventOrigin,
    pub remote_cursor: Option<RemoteCursor>,
}
impl EventSource {
    pub fn new(id: EventSourceId, origin: EventOrigin) -> Self {
        Self {
            id,
            origin,
            remote_cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEnd {
    Completed { result: RunResult },
    Cancelled,
    Failed { reason: String },
}

/// Codex extension receipts; no remote run/message identity is invented.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SteeringReceipt {
    Injected,
    StartedNewTurn { local_run_id: RunId },
    PromptRequired,
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// Use only for actual acceptance evidence, never merely a socket write.
    PromptAccepted,
    PromptReturned {
        end: RunEnd,
    },
    Steering {
        receipt: SteeringReceipt,
    },
    /// Runtime can prove the request was not sent/consumed.
    NotSent {
        reason: String,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseDelivery {
    Submitted,
    /// Validation rejected while the original live responder is still usable.
    RejectedLocally {
        reason: String,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConsumptionEvidence {
    Consumed {
        run_id: RunId,
        end: Option<RunEnd>,
    },
    /// Proof of non-consumption AND that the original transport call is closed.
    NotConsumed,
    Unknown {
        reason: String,
    },
}

/// New-instance cleanup is not proof that the old Agent task completed successfully.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceCleanupOutcome {
    Released,
    Unconfirmed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ClosingResourceHandoff {
    pub abandoned_epoch: ConnectionEpoch,
    pub request_id: Option<RequestId>,
}
impl ClosingResourceHandoff {
    pub fn new(abandoned_epoch: ConnectionEpoch) -> Self {
        Self {
            abandoned_epoch,
            request_id: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    ConnectionChanged {
        phase: ConnectionPhase,
    },
    SessionRegistered {
        binding: ExternalSessionBinding,
        resumed: bool,
    },
    ResourcesReleased,
    ClosingResourcesReconciled {
        abandoned_epoch: ConnectionEpoch,
        outcome: ResourceCleanupOutcome,
    },
    Capabilities {
        capabilities: AgentCapabilities,
    },
    ConfigurationReported {
        effective: EffectiveConfiguration,
        capabilities: AgentCapabilities,
    },
    ConfigurationChangeSettled {
        reported_revision: u64,
    },
    ConfigurationRejected {
        reason: String,
    },
    ConfigurationDeferred {
        timing: ConfigurationApplyTiming,
    },
    RunRecovered {
        run_id: RunId,
    },
    RunEnded {
        end: RunEnd,
    },
    MessageStarted {
        message: Message,
    },
    ContentBlock {
        message_id: MessageId,
        block: ContentBlock,
    },
    TextDelta {
        message_id: MessageId,
        block_id: ContentBlockId,
        delta: String,
    },
    MessageCompleted {
        message_id: MessageId,
    },
    Tool {
        tool: ToolCall,
    },
    Plan {
        plan: Plan,
    },
    FileChange {
        change: FileChange,
    },
    Attachment {
        attachment: Attachment,
    },
    Permission {
        request: PermissionRequest,
    },
    Question {
        request: QuestionRequest,
    },
    RequestInvalidated {
        reason: RequestInvalidationReason,
    },
    PermissionDelivery {
        attempt_id: ResponseAttemptId,
        delivery: ResponseDelivery,
    },
    QuestionDelivery {
        attempt_id: ResponseAttemptId,
        delivery: ResponseDelivery,
    },
    Attempt {
        attempt_id: AttemptId,
        outcome: AttemptOutcome,
    },
    SubmissionReconciled {
        submission_id: SubmissionId,
        attempt_id: AttemptId,
        attempt_epoch: ConnectionEpoch,
        evidence: ConsumptionEvidence,
    },
    Error {
        error: BusinessError,
    },
    /// Diagnostics are stored internally. UI receives only type and safe summary text.
    Unsupported {
        type_name: String,
        diagnostic: serde_json::Value,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EventEnvelope {
    #[serde(default)]
    pub command_id: Option<CommandId>,
    pub version: ModelVersion,
    pub id: EventId,
    pub observed_at: Timestamp,
    pub scope: Scope,
    pub source: EventSource,
    pub event: Event,
}
impl EventEnvelope {
    pub fn new(
        id: EventId,
        observed_at: Timestamp,
        scope: Scope,
        source: EventSource,
        event: Event,
    ) -> Self {
        Self {
            command_id: None,
            version: ModelVersion::CURRENT,
            id,
            observed_at,
            scope,
            source,
            event,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    Command(CommandEnvelope),
    Event(EventEnvelope),
}

/// Strict host order; not an Agent cursor. Allocate once before transactional commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JournalRecord {
    pub version: ModelVersion,
    pub sequence: u64,
    pub entry: JournalEntry,
}
impl JournalRecord {
    pub fn new(sequence: u64, entry: JournalEntry) -> Self {
        Self {
            version: ModelVersion::CURRENT,
            sequence,
            entry,
        }
    }
}
