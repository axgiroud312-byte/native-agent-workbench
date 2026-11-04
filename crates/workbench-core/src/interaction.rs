use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    AgentFieldId, AgentOptionId, CapabilityOption, ConnectionEpoch, ExternalRequestId, RequestId,
    ResponseAttemptId, RunContext, SessionId, Timestamp, ToolCallId,
};

/// Local routing identity on one connection generation. Runtime may route responses by
/// an SDK callback keyed by this local ID without exposing a protocol request ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RequestContext {
    pub id: RequestId,
    pub run: RunContext,
    pub epoch: ConnectionEpoch,
    pub external_id: Option<ExternalRequestId>,
    pub created_at: Timestamp,
    pub tool_call_id: Option<ToolCallId>,
}

impl RequestContext {
    pub fn new(
        id: RequestId,
        run: RunContext,
        epoch: ConnectionEpoch,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            run,
            epoch,
            external_id: None,
            created_at,
            tool_call_id: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestInvalidationReason {
    RunEnded,
    RunCancelled,
    Disconnected,
    Superseded,
    SessionDeleted,
    AgentWithdrawn,
    Expired,
}

/// Submitting records intent; ResponseSubmitted means successful handoff to the SDK,
/// not an additional Agent acknowledgement or confirmation of the authorized operation.
/// Unknown delivery must be reconciled, and invalidated requests never become actionable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestStatus<Response> {
    Pending,
    Submitting {
        attempt_id: ResponseAttemptId,
        submitted_at: Timestamp,
        response: Response,
    },
    ResponseSubmitted {
        submitted_at: Timestamp,
        response: Response,
    },
    ResponseRejected {
        rejected_at: Timestamp,
        response: Response,
        reason: String,
    },
    OutcomeUnknown {
        attempt_id: ResponseAttemptId,
        observed_at: Timestamp,
        response: Response,
        reason: String,
    },
    Invalidated {
        invalidated_at: Timestamp,
        reason: RequestInvalidationReason,
    },
}

impl<Response> RequestStatus<Response> {
    /// Final actionability additionally requires matching epoch/run and a non-cancelling run.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::Pending | Self::ResponseRejected { .. })
    }

    pub fn is_waiting(&self) -> bool {
        !matches!(
            self,
            Self::ResponseSubmitted { .. } | Self::Invalidated { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionResource {
    File {
        path: PathBuf,
    },
    Command {
        command: String,
        working_directory: Option<PathBuf>,
    },
    Uri {
        uri: String,
    },
    AgentReported {
        description: String,
    },
}

/// Agent-reported operation and affected resources, not a host authorization policy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionScope {
    pub description: Option<String>,
    pub resources: Vec<PermissionResource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    AgentDefined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionOption {
    pub option: CapabilityOption,
    pub decision: PermissionDecisionKind,
    pub scope_description: Option<String>,
}

impl PermissionOption {
    pub fn new(option: CapabilityOption, decision: PermissionDecisionKind) -> Self {
        Self {
            option,
            decision,
            scope_description: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionResponse {
    Selected { option_id: AgentOptionId },
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionRequest {
    pub context: RequestContext,
    pub operation: String,
    pub description: Option<String>,
    pub scope: PermissionScope,
    pub options: Vec<PermissionOption>,
    pub status: RequestStatus<PermissionResponse>,
}

impl PermissionRequest {
    pub fn new(
        context: RequestContext,
        operation: impl Into<String>,
        options: Vec<PermissionOption>,
    ) -> Self {
        Self {
            context,
            operation: operation.into(),
            description: None,
            scope: PermissionScope::default(),
            options,
            status: RequestStatus::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionFieldKind {
    Text {
        multiline: bool,
    },
    SingleChoice {
        options: Vec<CapabilityOption>,
        allows_custom: bool,
    },
    MultipleChoice {
        options: Vec<CapabilityOption>,
        allows_custom: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionField {
    pub id: AgentFieldId,
    pub prompt: String,
    pub required: bool,
    pub kind: QuestionFieldKind,
}

impl QuestionField {
    pub fn new(id: AgentFieldId, prompt: impl Into<String>, kind: QuestionFieldKind) -> Self {
        Self {
            id,
            prompt: prompt.into(),
            required: true,
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionAnswerValue {
    Text {
        text: String,
    },
    Choices {
        option_ids: Vec<AgentOptionId>,
        custom: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionAnswer {
    pub field_id: AgentFieldId,
    pub value: QuestionAnswerValue,
}

impl QuestionAnswer {
    pub fn new(field_id: AgentFieldId, value: QuestionAnswerValue) -> Self {
        Self { field_id, value }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionResponse {
    Answered { answers: Vec<QuestionAnswer> },
    Cancelled,
}

/// Multiple fields express a form. All prompts and choices come from the Agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionRequest {
    pub context: RequestContext,
    pub title: Option<String>,
    pub fields: Vec<QuestionField>,
    pub status: RequestStatus<QuestionResponse>,
}

impl QuestionRequest {
    pub fn new(context: RequestContext, fields: Vec<QuestionField>) -> Self {
        Self {
            context,
            title: None,
            fields,
            status: RequestStatus::Pending,
        }
    }
}

/// Core-owned interaction records for one session. Waiting is a projection of statuses,
/// not a mutually exclusive execution state. Keys must equal the contained local request ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionRequests {
    pub session_id: SessionId,
    pub permissions: BTreeMap<RequestId, PermissionRequest>,
    pub questions: BTreeMap<RequestId, QuestionRequest>,
}

impl SessionRequests {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            permissions: BTreeMap::new(),
            questions: BTreeMap::new(),
        }
    }

    pub fn waiting_ids(&self) -> impl Iterator<Item = RequestId> + '_ {
        self.permissions
            .values()
            .filter(|request| request.status.is_waiting())
            .map(|request| request.context.id)
            .chain(
                self.questions
                    .values()
                    .filter(|request| request.status.is_waiting())
                    .map(|request| request.context.id),
            )
    }
}
