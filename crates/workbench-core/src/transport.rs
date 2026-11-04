use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransportMethod {
    Prompt,
    Steering { prompt_required: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttemptState {
    InFlight,
    Accepted,
    /// Reconciliation proved consumption, but execution end still requires evidence.
    ConsumedAwaitingEnd,
    Injected,
    StartedNewTurn {
        run_id: RunId,
    },
    NotConsumed,
    Returned,
    NotSent {
        reason: String,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TransportAttempt {
    pub id: AttemptId,
    pub submission_id: SubmissionId,
    pub request_id: RequestId,
    pub context: RunContext,
    pub epoch: ConnectionEpoch,
    pub method: TransportMethod,
    pub state: AttemptState,
    pub command_id: CommandId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeOperation {
    Connect {
        project: Project,
        agent: AgentDefinition,
        external: Option<ExternalSessionBinding>,
        resume: bool,
    },
    ReleaseSession,
    ReconcileClosingResources {
        abandoned_epoch: ConnectionEpoch,
    },
    Submit {
        attempt: TransportAttempt,
        submission: InputSubmission,
    },
    Cancel,
    ReconcileSubmission {
        submission_id: SubmissionId,
        attempt: TransportAttempt,
    },
    Configure {
        pending: PendingConfiguration,
    },
    RespondPermission {
        context: RequestContext,
        response: PermissionResponse,
        attempt_id: ResponseAttemptId,
    },
    RespondQuestion {
        context: RequestContext,
        response: QuestionResponse,
        attempt_id: ResponseAttemptId,
    },
    /// Invalidates the UI first, then consumes/cancels the original live SDK responders.
    CancelRequests {
        ids: Vec<RequestId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectCause {
    Command { id: CommandId },
    Event { id: EventId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RuntimeEffect {
    pub cause: EffectCause,
    pub scope: Scope,
    pub operation: RuntimeOperation,
}

/// A logical submission is allowed at most one undecided transport attempt.
pub(crate) fn unsettled(state: &SubmissionState) -> bool {
    matches!(
        state,
        SubmissionState::Prepared
            | SubmissionState::AwaitingAcknowledgement { .. }
            | SubmissionState::OutcomeUnknown { .. }
    )
}
