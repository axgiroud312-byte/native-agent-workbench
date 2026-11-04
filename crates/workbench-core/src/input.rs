use serde::{Deserialize, Serialize};

use crate::{
    AgentCommandId, AttachmentId, ConnectionEpoch, ExternalSubmissionId, FileReference, MessageId,
    QueuedInputId, RequestedConfiguration, RunId, SessionId, SubmissionId, Timestamp,
};

/// Input has a deliberately narrower vocabulary than Agent output blocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputPart {
    Text {
        text: String,
    },
    Attachment {
        attachment_id: AttachmentId,
    },
    FileReference {
        reference: FileReference,
    },
    AgentCommand {
        command_id: AgentCommandId,
        arguments: String,
    },
}

/// One durable draft per session. Revision changes with content/configuration edits.
/// Caret, IME composition, popup state and focus belong to the view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ComposerDraft {
    pub session_id: SessionId,
    pub revision: u64,
    pub content: Vec<InputPart>,
    pub configuration: RequestedConfiguration,
    pub updated_at: Timestamp,
}

impl ComposerDraft {
    pub fn new(session_id: SessionId, updated_at: Timestamp) -> Self {
        Self {
            session_id,
            revision: 0,
            content: Vec::new(),
            configuration: RequestedConfiguration::default(),
            updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueuedInputState {
    Waiting,
    /// Dispatch is held while the durable queued content is being edited.
    Editing,
    Dispatching {
        submission_id: SubmissionId,
    },
    Submitted {
        submission_id: SubmissionId,
    },
    Cancelled {
        cancelled_at: Timestamp,
    },
}

/// Snapshot belongs to the session, independent of later composer edits and selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QueuedInput {
    pub id: QueuedInputId,
    pub session_id: SessionId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub content: Vec<InputPart>,
    pub configuration: RequestedConfiguration,
    pub state: QueuedInputState,
}

impl QueuedInput {
    pub fn from_draft(id: QueuedInputId, draft: &ComposerDraft, created_at: Timestamp) -> Self {
        Self {
            id,
            session_id: draft.session_id,
            created_at,
            updated_at: created_at,
            content: draft.content.clone(),
            configuration: draft.configuration.clone(),
            state: QueuedInputState::Waiting,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueuePauseReason {
    User,
    RunStopped,
    RunFailed,
    Disconnected,
    ApplicationRestart,
    SubmissionOutcomeUnknown,
    CapabilityChanged { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueDispatchState {
    Ready,
    Paused {
        reason: QueuePauseReason,
        paused_at: Timestamp,
    },
}

/// Vec order is user order; IDs survive removal, editing and serialization.
/// This is a core-owned queue, not a claim that the Agent implements native queuing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InputQueue {
    pub session_id: SessionId,
    pub entries: Vec<QueuedInput>,
    pub dispatch: QueueDispatchState,
}

impl InputQueue {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            entries: Vec::new(),
            dispatch: QueueDispatchState::Ready,
        }
    }
}

/// A local run ID is reserved before new-run dispatch. Steering targets an existing run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmissionIntent {
    StartRun { run_id: RunId },
    SteerRun { run_id: RunId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmissionState {
    Prepared,
    AwaitingAcknowledgement {
        epoch: ConnectionEpoch,
        sent_at: Timestamp,
    },
    Accepted {
        epoch: ConnectionEpoch,
        accepted_at: Timestamp,
        run_id: RunId,
        message_id: Option<MessageId>,
        external_id: Option<ExternalSubmissionId>,
    },
    Rejected {
        rejected_at: Timestamp,
        reason: String,
    },
    OutcomeUnknown {
        epoch: ConnectionEpoch,
        observed_at: Timestamp,
        reason: String,
    },
}

/// Durable logical submission. Content and intent freeze once sent; never blindly retry
/// unknown delivery or convert a steering request into a new run without a definite receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InputSubmission {
    pub id: SubmissionId,
    pub session_id: SessionId,
    pub queued_input_id: Option<QueuedInputId>,
    pub source_draft_revision: Option<u64>,
    pub created_at: Timestamp,
    pub intent: SubmissionIntent,
    pub content: Vec<InputPart>,
    pub configuration: RequestedConfiguration,
    pub state: SubmissionState,
}

impl InputSubmission {
    pub fn from_draft(
        id: SubmissionId,
        draft: &ComposerDraft,
        intent: SubmissionIntent,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            session_id: draft.session_id,
            queued_input_id: None,
            source_draft_revision: Some(draft.revision),
            created_at,
            intent,
            content: draft.content.clone(),
            configuration: draft.configuration.clone(),
            state: SubmissionState::Prepared,
        }
    }

    pub fn from_queued(
        id: SubmissionId,
        input: &QueuedInput,
        intent: SubmissionIntent,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            session_id: input.session_id,
            queued_input_id: Some(input.id),
            source_draft_revision: None,
            created_at,
            intent,
            content: input.content.clone(),
            configuration: input.configuration.clone(),
            state: SubmissionState::Prepared,
        }
    }
}
