use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The sole mutable business aggregate. Callers receive projections, never mutable records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchState {
    pub(crate) sequence: u64,
    pub(crate) projects: BTreeMap<ProjectId, Project>,
    pub(crate) agents: BTreeMap<AgentId, AgentDefinition>,
    pub(crate) sessions: BTreeMap<SessionId, SessionState>,
    pub(crate) deleted_sessions: BTreeSet<SessionId>,
    pub(crate) deleted_projects: BTreeSet<ProjectId>,
    pub(crate) commands: BTreeMap<CommandId, CommandResult>,
    pub(crate) global_errors: Vec<BusinessError>,
    pub(crate) events: BTreeSet<EventId>,
    pub(crate) remote_events: BTreeSet<(
        EventSourceId,
        Option<SessionId>,
        Option<ConnectionEpoch>,
        RemoteCursor,
    )>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionState {
    pub record: SessionRecord,
    pub runs: BTreeMap<RunId, Run>,
    pub messages: Vec<Message>,
    pub tools: BTreeMap<ToolCallId, ToolCall>,
    pub plans: BTreeMap<PlanId, Plan>,
    pub changes: BTreeMap<FileChangeId, FileChange>,
    pub attachments: BTreeMap<AttachmentId, Attachment>,
    pub requests: SessionRequests,
    pub draft: ComposerDraft,
    pub queue: InputQueue,
    pub submissions: BTreeMap<SubmissionId, InputSubmission>,
    pub attempts: BTreeMap<AttemptId, TransportAttempt>,
    pub reconciliations: BTreeMap<RequestId, ReconciliationTarget>,
    pub cancel_requests: BTreeMap<RunId, RequestId>,
    /// Ordinary prompt call must actually return before another can be dispatched.
    pub prompt_in_flight: Option<AttemptId>,
    pub next_configuration: Option<PendingConfiguration>,
    pub resources_released: bool,
    pub closing: bool,
    #[serde(default)]
    pub closing_handoff: Option<ClosingResourceHandoff>,
    pub diagnostics: Vec<DiagnosticRecord>,
    pub errors: Vec<BusinessError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DiagnosticRecord {
    pub event_id: EventId,
    pub type_name: String,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReconciliationTarget {
    pub submission_id: SubmissionId,
    pub attempt_id: AttemptId,
    pub attempt_epoch: ConnectionEpoch,
    pub connection_epoch: ConnectionEpoch,
}

impl SessionState {
    pub fn new(record: SessionRecord) -> Self {
        let id = record.id();
        let at = record.created_at;
        Self {
            record,
            runs: BTreeMap::new(),
            messages: Vec::new(),
            tools: BTreeMap::new(),
            plans: BTreeMap::new(),
            changes: BTreeMap::new(),
            attachments: BTreeMap::new(),
            requests: SessionRequests::new(id),
            draft: ComposerDraft::new(id, at),
            queue: InputQueue::new(id),
            submissions: BTreeMap::new(),
            attempts: BTreeMap::new(),
            reconciliations: BTreeMap::new(),
            cancel_requests: BTreeMap::new(),
            prompt_in_flight: None,
            next_configuration: None,
            resources_released: true,
            closing: false,
            closing_handoff: None,
            diagnostics: Vec::new(),
            errors: Vec::new(),
        }
    }
    pub fn pause(&mut self, reason: QueuePauseReason, at: Timestamp) {
        self.queue.dispatch = QueueDispatchState::Paused {
            reason,
            paused_at: at,
        };
    }
    pub fn waiting(&self) -> bool {
        self.requests.waiting_ids().next().is_some()
    }
    pub fn current_run(&self) -> Option<&Run> {
        self.record.current_run_id.and_then(|id| self.runs.get(&id))
    }
}

impl WorkbenchState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn command_result(&self, id: CommandId) -> Option<&CommandResult> {
        self.commands.get(&id)
    }
    pub fn is_deleted(&self, id: SessionId) -> bool {
        self.deleted_sessions.contains(&id)
    }
}
