mod command;
mod event;
mod receipt;
mod validate;

use crate::state::SessionState;
use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    Live,
    Replay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyDisposition {
    Applied,
    Duplicate,
    Stale,
    Rejected(BusinessError),
}

/// Proposed atomic state plus effects. Commit through StoragePort before publishing/delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    next: WorkbenchState,
    effects: Vec<RuntimeEffect>,
    disposition: ApplyDisposition,
}
impl Transition {
    pub fn state(&self) -> &WorkbenchState {
        &self.next
    }
    pub fn effects(&self) -> &[RuntimeEffect] {
        &self.effects
    }
    pub fn disposition(&self) -> &ApplyDisposition {
        &self.disposition
    }
    pub fn into_parts(self) -> (WorkbenchState, Vec<RuntimeEffect>, ApplyDisposition) {
        (self.next, self.effects, self.disposition)
    }
}

/// Deterministic single update entry: no clock, random ID, I/O or view-owned business state.
/// Rejected commands/events are atomic no-ops with a persisted decision and consumed host order.
pub fn reduce(
    state: &WorkbenchState,
    record: &JournalRecord,
    mode: ApplyMode,
) -> Result<Transition, BusinessError> {
    let seen = match &record.entry {
        JournalEntry::Command(command) => state.commands.contains_key(&command.id),
        JournalEntry::Event(event) => state.events.contains(&event.id),
    };
    if record.sequence <= state.sequence && seen {
        return Ok(Transition {
            next: state.clone(),
            effects: Vec::new(),
            disposition: ApplyDisposition::Duplicate,
        });
    }
    if state.sequence.checked_add(1) != Some(record.sequence) {
        return Err(BusinessError::new(
            ErrorKind::SequenceConflict,
            "host journal sequence is not the next committed value",
            RetryAdvice::Never,
        ));
    }
    let mut next = state.clone();
    let mut effects = Vec::new();
    let mut disposition = ApplyDisposition::Duplicate;
    if !seen {
        let result = match &record.entry {
            JournalEntry::Command(command) => {
                command::apply(&mut next, command, &mut effects).map(|()| ApplyDisposition::Applied)
            }
            JournalEntry::Event(event) => event::apply(&mut next, event, &mut effects),
        };
        disposition = match result {
            Ok(value) => value,
            Err(error) => {
                next = state.clone();
                effects.clear();
                ApplyDisposition::Rejected(error)
            }
        };
    }
    next.sequence = record.sequence;
    match &record.entry {
        JournalEntry::Command(command) => {
            let status = match &disposition {
                ApplyDisposition::Rejected(error) => CommandStatus::Rejected {
                    error: error.clone(),
                },
                _ => CommandStatus::Applied,
            };
            next.commands.entry(command.id).or_insert(CommandResult {
                id: command.id,
                status,
            });
        }
        JournalEntry::Event(event) => {
            next.events.insert(event.id);
        }
    }
    if mode == ApplyMode::Replay {
        effects.clear();
    }
    Ok(Transition {
        next,
        effects,
        disposition,
    })
}

/// Replays host records only. App must then journal Command::Restart before live operation.
pub fn replay(
    initial: WorkbenchState,
    records: &[JournalRecord],
) -> Result<WorkbenchState, BusinessError> {
    records.iter().try_fold(initial, |state, record| {
        reduce(&state, record, ApplyMode::Replay).map(|t| t.next)
    })
}

pub(crate) fn is_fresh_prompt(s: &SessionState) -> bool {
    command::fresh_prompt(s).is_ok()
}

pub(crate) fn has_unsettled_input(s: &SessionState) -> bool {
    validate::pending_submission(s, None).is_err()
}
pub(crate) fn is_steering_eligible(s: &SessionState) -> bool {
    s.record.current_run_id.is_some_and(|id| {
        validate::steering(s, id, &s.draft.content, &s.draft.configuration).is_ok()
    })
}
pub(crate) fn valid_input(
    s: &SessionState,
    content: &[InputPart],
    requested: &RequestedConfiguration,
) -> bool {
    validate::input(s, content, requested).is_ok()
}

pub(crate) fn valid_input_configuration(
    s: &SessionState,
    requested: &RequestedConfiguration,
) -> bool {
    validate::input_configuration(s, requested).is_ok()
}

fn session_mut(
    state: &mut WorkbenchState,
    scope: Scope,
) -> Result<&mut SessionState, BusinessError> {
    let id = scope
        .session_id
        .ok_or_else(|| BusinessError::invalid("session scope is required"))?;
    let session = state.sessions.get_mut(&id).ok_or_else(|| {
        BusinessError::new(
            ErrorKind::NotFound,
            "session is absent or deleted",
            RetryAdvice::Never,
        )
    })?;
    if scope.epoch != Some(session.record.connection.epoch) {
        return Err(BusinessError::stale());
    }
    if scope
        .agent_id
        .is_some_and(|id| id != session.record.agent_id())
    {
        return Err(BusinessError::stale());
    }
    Ok(session)
}

fn effect(
    effects: &mut Vec<RuntimeEffect>,
    cause: EffectCause,
    scope: Scope,
    operation: RuntimeOperation,
) {
    effects.push(RuntimeEffect {
        cause,
        scope,
        operation,
    });
}

fn invalidate(
    session: &mut SessionState,
    reason: RequestInvalidationReason,
    at: Timestamp,
) -> Vec<RequestId> {
    let mut ids = Vec::new();
    for request in session.requests.permissions.values_mut() {
        if request.status.is_waiting() {
            ids.push(request.context.id);
            request.status = RequestStatus::Invalidated {
                invalidated_at: at,
                reason: reason.clone(),
            };
        }
    }
    for request in session.requests.questions.values_mut() {
        if request.status.is_waiting() {
            ids.push(request.context.id);
            request.status = RequestStatus::Invalidated {
                invalidated_at: at,
                reason: reason.clone(),
            };
        }
    }
    ids
}

fn interrupt(session: &mut SessionState, at: Timestamp, reason: &str) {
    for run in session.runs.values_mut() {
        if !run.state.is_terminal() {
            run.state = ExecutionState::Interrupted {
                observed_at: at,
                reason: reason.into(),
            };
        }
    }
    for tool in session.tools.values_mut() {
        if matches!(tool.state, ToolState::Pending | ToolState::Running) {
            tool.state = ToolState::Interrupted {
                observed_at: at,
                reason: reason.into(),
            };
        }
    }
    for message in &mut session.messages {
        if matches!(message.state, MessageState::Streaming) {
            message.state = MessageState::Interrupted {
                reason: reason.into(),
            };
        }
    }
    for submission in session.submissions.values_mut() {
        if matches!(
            submission.state,
            SubmissionState::AwaitingAcknowledgement { .. }
        ) {
            submission.state = SubmissionState::OutcomeUnknown {
                epoch: session.record.connection.epoch,
                observed_at: at,
                reason: reason.into(),
            };
        }
    }
    for attempt in session.attempts.values_mut() {
        if matches!(
            attempt.state,
            AttemptState::InFlight | AttemptState::Accepted
        ) {
            attempt.state = AttemptState::Unknown {
                reason: reason.into(),
            };
        }
    }
    session.record.current_run_id = None;
    session.record.configuration.pending = None;
    session.next_configuration = None;
    session.reconciliations.clear();
}
