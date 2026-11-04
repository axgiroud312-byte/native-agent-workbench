use super::{ApplyDisposition, effect, interrupt, invalidate, session_mut, validate};
use crate::state::{DiagnosticRecord, SessionState};
use crate::*;

pub(super) fn apply(
    state: &mut WorkbenchState,
    envelope: &EventEnvelope,
    effects: &mut Vec<RuntimeEffect>,
) -> Result<ApplyDisposition, BusinessError> {
    let scope = envelope.scope;
    let at = envelope.observed_at;
    let cause = EffectCause::Event { id: envelope.id };
    let Some(session_id) = scope.session_id else {
        if let Event::Error { error } = &envelope.event {
            state.global_errors.push(error.clone());
            return Ok(ApplyDisposition::Applied);
        }
        return Err(BusinessError::invalid(
            "session event must carry local session identity",
        ));
    };
    if state.deleted_sessions.contains(&session_id) || !state.sessions.contains_key(&session_id) {
        return Ok(ApplyDisposition::Stale);
    }
    if scope.epoch != Some(state.sessions[&session_id].record.connection.epoch) {
        return Ok(ApplyDisposition::Stale);
    }
    if let Some(cursor) = &envelope.source.remote_cursor {
        let key = (
            envelope.source.id,
            scope.session_id,
            scope.epoch,
            cursor.clone(),
        );
        if !state.remote_events.insert(key) {
            return Ok(ApplyDisposition::Duplicate);
        }
    }
    let session = session_mut(state, scope)?;
    if session.closing
        && !matches!(
            envelope.event,
            Event::ResourcesReleased
                | Event::ClosingResourcesReconciled { .. }
                | Event::Error { .. }
                | Event::Unsupported { .. }
        )
    {
        return Ok(ApplyDisposition::Stale);
    }
    if matches!(
        session.record.connection.phase,
        ConnectionPhase::Disconnected { .. }
            | ConnectionPhase::Failed { .. }
            | ConnectionPhase::Closed
    ) && !matches!(
        envelope.event,
        Event::ResourcesReleased
            | Event::ClosingResourcesReconciled { .. }
            | Event::Error { .. }
            | Event::Unsupported { .. }
    ) {
        return Ok(ApplyDisposition::Stale);
    }
    if matches!(envelope.source.origin, EventOrigin::AgentReplay { .. }) {
        session.record.history = HistoryAvailability::Partial {
            reason:
                "Agent history requires identity/snapshot reconciliation; local history retained"
                    .into(),
        };
        session.diagnostics.push(DiagnosticRecord {
            event_id: envelope.id,
            type_name: "agent_history_replay".into(),
            payload: serde_json::to_value(&envelope.event)
                .map_err(|_| BusinessError::invalid("cannot preserve replay diagnostic"))?,
        });
        return Ok(ApplyDisposition::Applied);
    }
    let transport_event = matches!(
        envelope.event,
        Event::Attempt { .. } | Event::RunRecovered { .. } | Event::SubmissionReconciled { .. }
    );
    if let Some(run_id) = scope.run_id {
        if !transport_event
            && (session.record.current_run_id != Some(run_id)
                || !session
                    .runs
                    .get(&run_id)
                    .is_some_and(|r| !r.state.is_terminal()))
        {
            return Ok(ApplyDisposition::Stale);
        }
    }
    match &envelope.event {
        Event::ConnectionChanged { phase } => {
            if matches!(session.record.connection.phase, ConnectionPhase::Closed)
                && !matches!(phase, ConnectionPhase::Closed)
            {
                return Ok(ApplyDisposition::Stale);
            }
            if matches!(
                phase,
                ConnectionPhase::Disconnected { .. }
                    | ConnectionPhase::Failed { .. }
                    | ConnectionPhase::Closed
            ) {
                let ids = invalidate(session, RequestInvalidationReason::Disconnected, at);
                interrupt(
                    session,
                    at,
                    "connection lost without confirmed run completion",
                );
                session.pause(QueuePauseReason::Disconnected, at);
                if !ids.is_empty() {
                    effect(
                        effects,
                        cause,
                        scope,
                        RuntimeOperation::CancelRequests { ids },
                    );
                }
            }
            session.record.connection.phase = phase.clone();
        }
        Event::ResourcesReleased => {
            if session.closing_handoff.is_some() {
                return Ok(ApplyDisposition::Stale);
            }
            invalidate(session, RequestInvalidationReason::Disconnected, at);
            interrupt(session, at, "runtime resources released");
            session.resources_released = true;
            session.prompt_in_flight = None;
            session.closing = false;
            session.record.connection.phase = ConnectionPhase::Closed;
            session.pause(QueuePauseReason::Disconnected, at);
        }
        Event::ClosingResourcesReconciled {
            abandoned_epoch,
            outcome,
        } => {
            let Some(handoff) = session.closing_handoff.as_ref() else {
                return Ok(ApplyDisposition::Stale);
            };
            if !session.closing
                || handoff.abandoned_epoch != *abandoned_epoch
                || handoff.request_id != scope.request_id
                || scope.request_id.is_none()
            {
                return Ok(ApplyDisposition::Stale);
            }
            if !matches!(envelope.source.origin, EventOrigin::Runtime { .. }) {
                return Err(BusinessError::invalid(
                    "resource handoff requires a Runtime ownership/cleanup report",
                ));
            }
            match outcome {
                ResourceCleanupOutcome::Released => {
                    session.resources_released = true;
                    session.closing = false;
                    session.closing_handoff = None;
                    session.prompt_in_flight = None;
                    session.record.connection.phase = ConnectionPhase::Closed;
                }
                ResourceCleanupOutcome::Unconfirmed { reason } => {
                    session.closing_handoff.as_mut().unwrap().request_id = None;
                    session.errors.push(BusinessError::new(
                        ErrorKind::ResourceUnavailable,
                        reason,
                        RetryAdvice::ReconcileFirst,
                    ));
                }
            }
        }
        Event::SessionRegistered { binding, resumed } => {
            if *resumed
                && session
                    .record
                    .external
                    .as_ref()
                    .is_none_or(|old| old.id != binding.id)
            {
                return Err(BusinessError::invalid(
                    "resume must preserve the registered external session ID",
                ));
            }
            if !resumed && session.record.external.is_some() {
                return Err(BusinessError::invalid(
                    "cannot replace existing external session binding",
                ));
            }
            session.record.external = Some(binding.clone());
            if *resumed {
                session.record.resume = SessionResumeState::Resumed {
                    epoch: scope.epoch.unwrap(),
                };
            }
        }
        Event::Capabilities { capabilities } => {
            if capabilities.epoch != scope.epoch.unwrap() {
                return Err(BusinessError::stale());
            }
            if session.record.capabilities.as_ref().is_some_and(|old| {
                old.epoch == capabilities.epoch && old.revision >= capabilities.revision
            }) {
                return Ok(ApplyDisposition::Stale);
            }
            session.record.capabilities = Some(capabilities.clone());
        }
        Event::ConfigurationReported {
            effective,
            capabilities,
        } => {
            if effective.epoch != scope.epoch.unwrap()
                || capabilities.epoch != effective.epoch
                || effective.capability_revision != capabilities.revision
            {
                return Err(BusinessError::stale());
            }
            if session.record.capabilities.as_ref().is_some_and(|old| {
                old.epoch == capabilities.epoch && old.revision > capabilities.revision
            }) {
                return Ok(ApplyDisposition::Stale);
            }
            if session
                .record
                .configuration
                .effective
                .as_ref()
                .is_some_and(|old| {
                    old.epoch == effective.epoch
                        && old.capability_revision >= effective.capability_revision
                })
            {
                return Ok(ApplyDisposition::Stale);
            }
            if effective
                .model
                .as_ref()
                .is_some_and(|id| !capabilities.models.iter().any(|m| &m.option.id == id))
                || effective.permission.as_ref().is_some_and(|id| {
                    !capabilities
                        .permission_modes
                        .iter()
                        .any(|p| &p.option.id == id)
                })
            {
                return Err(BusinessError::invalid(
                    "confirmed option is absent from the complete capability snapshot",
                ));
            }
            if let Some(effort) = &effective.reasoning_effort {
                if !capabilities.models.iter().any(|m| {
                    effective.model.as_ref() == Some(&m.option.id)
                        && m.reasoning_efforts.iter().any(|e| &e.id == effort)
                }) {
                    return Err(BusinessError::invalid(
                        "confirmed effort is incompatible with confirmed model",
                    ));
                }
            }
            session.record.configuration.effective = Some(effective.clone());
            session.record.capabilities = Some(capabilities.clone());
        }
        Event::ConfigurationChangeSettled { reported_revision } => {
            if session
                .record
                .configuration
                .effective
                .as_ref()
                .is_none_or(|e| {
                    e.epoch != scope.epoch.unwrap() || e.capability_revision != *reported_revision
                })
            {
                return Err(BusinessError::invalid(
                    "configuration must report a full confirmed snapshot before settling a request",
                ));
            }
            if session
                .record
                .configuration
                .pending
                .as_ref()
                .is_none_or(|p| Some(p.request_id) != scope.request_id)
            {
                return Ok(ApplyDisposition::Stale);
            }
            session.record.configuration.pending = None;
            next_configuration(session, scope, cause, at, effects)?;
        }
        Event::ConfigurationDeferred { timing } => {
            if let Some(pending) = &mut session.record.configuration.pending {
                if Some(pending.request_id) != scope.request_id {
                    return Ok(ApplyDisposition::Stale);
                }
                pending.timing = timing.clone();
            } else {
                return Ok(ApplyDisposition::Stale);
            }
        }
        Event::ConfigurationRejected { reason } => {
            if session
                .record
                .configuration
                .pending
                .as_ref()
                .is_none_or(|p| Some(p.request_id) != scope.request_id)
            {
                return Ok(ApplyDisposition::Stale);
            }
            let pending = session.record.configuration.pending.take().unwrap();
            session.record.configuration.last_rejection =
                Some(ConfigurationRejection::new(pending, at, reason));
            session.pause(
                QueuePauseReason::CapabilityChanged {
                    reason: reason.clone(),
                },
                at,
            );
            next_configuration(session, scope, cause, at, effects)?;
        }
        Event::RunRecovered { run_id } => {
            let caps = validate::ready(session)?;
            validate::supported(&caps.active_run_recovery, "active run recovery")?;
            if scope.run_id != Some(*run_id)
                || session.record.current_run_id.is_some()
                || session.prompt_in_flight.is_some()
            {
                return Err(BusinessError::stale());
            }
            let run = session
                .runs
                .get_mut(run_id)
                .ok_or_else(BusinessError::stale)?;
            if !matches!(
                run.state,
                ExecutionState::Interrupted { .. } | ExecutionState::OutcomeUnknown { .. }
            ) {
                return Err(BusinessError::stale());
            }
            run.recovery = RunRecoveryState::Recovered {
                epoch: scope.epoch.unwrap(),
            };
            run.state = ExecutionState::Running;
            session.record.current_run_id = Some(*run_id);
        }
        Event::RunEnded { end } => {
            let id = run_scope(session, scope)?;
            if session
                .prompt_in_flight
                .is_some_and(|a| session.attempts[&a].context.run_id == id)
            {
                return Err(BusinessError::invalid(
                    "ordinary prompt ends only when its own call returns",
                ));
            }
            finish(session, id, end, at);
            let ids = invalidate(session, RequestInvalidationReason::RunEnded, at);
            if !ids.is_empty() {
                effect(
                    effects,
                    cause,
                    scope,
                    RuntimeOperation::CancelRequests { ids },
                );
            }
        }
        Event::MessageStarted { message } => {
            context(session, scope, message.context)?;
            if message.submission_id.is_some_and(|id| !session.submissions.get(&id).is_some_and(|s| match s.state {
                SubmissionState::Accepted { run_id, .. } => run_id == message.context.run_id,
                _ => matches!(s.intent, SubmissionIntent::StartRun { run_id } | SubmissionIntent::SteerRun { run_id } if run_id == message.context.run_id),
            })) { return Err(BusinessError::invalid("message submission does not belong to this session/run")); }
            if session.messages.iter().any(|m| m.id == message.id) {
                return Ok(ApplyDisposition::Stale);
            }
            blocks(session, &message.blocks, message.context)?;
            if message.blocks.iter().any(|b| {
                session
                    .messages
                    .iter()
                    .any(|m| m.blocks.iter().any(|old| old.id == b.id))
            }) {
                return Err(BusinessError::invalid(
                    "content block identity already belongs to another message",
                ));
            }
            session.messages.push(message.clone());
        }
        Event::ContentBlock { message_id, block } => {
            run_scope(session, scope)?;
            blocks(
                session,
                std::slice::from_ref(block),
                RunContext::new(session.record.id(), scope.run_id.unwrap()),
            )?;
            if session
                .messages
                .iter()
                .any(|m| m.id != *message_id && m.blocks.iter().any(|b| b.id == block.id))
            {
                return Err(BusinessError::stale());
            }
            let message = session
                .messages
                .iter_mut()
                .find(|m| {
                    m.id == *message_id
                        && Some(m.context.run_id) == scope.run_id
                        && matches!(m.state, MessageState::Streaming)
                })
                .ok_or_else(BusinessError::stale)?;
            if message.blocks.iter().any(|b| b.id == block.id) {
                return Ok(ApplyDisposition::Stale);
            }
            message.blocks.push(block.clone());
        }
        Event::TextDelta {
            message_id,
            block_id,
            delta,
        } => {
            run_scope(session, scope)?;
            let message = session
                .messages
                .iter_mut()
                .find(|m| {
                    m.id == *message_id
                        && Some(m.context.run_id) == scope.run_id
                        && matches!(m.state, MessageState::Streaming)
                })
                .ok_or_else(BusinessError::stale)?;
            let block = message
                .blocks
                .iter_mut()
                .find(|b| b.id == *block_id)
                .ok_or_else(|| {
                    BusinessError::invalid("text block must be declared before deltas")
                })?;
            let ContentKind::Text { text, .. } = &mut block.content else {
                return Err(BusinessError::invalid("delta target is not text"));
            };
            text.push_str(delta);
        }
        Event::MessageCompleted { message_id } => {
            run_scope(session, scope)?;
            session
                .messages
                .iter_mut()
                .find(|m| m.id == *message_id && Some(m.context.run_id) == scope.run_id)
                .ok_or_else(BusinessError::stale)?
                .state = MessageState::Complete;
        }
        Event::Tool { tool } => {
            context(session, scope, tool.context)?;
            if tool.epoch != scope.epoch.unwrap() {
                return Err(BusinessError::stale());
            }
            blocks(session, &tool.output, tool.context)?;
            if session.tools.get(&tool.id).is_some_and(|old| {
                old.context != tool.context
                    || matches!(
                        old.state,
                        ToolState::Completed { .. }
                            | ToolState::Failed { .. }
                            | ToolState::Cancelled { .. }
                            | ToolState::Interrupted { .. }
                    )
            }) {
                return Ok(ApplyDisposition::Stale);
            }
            session.tools.insert(tool.id, tool.clone());
        }
        Event::Plan { plan } => {
            context(session, scope, plan.context)?;
            if session
                .plans
                .get(&plan.id)
                .is_some_and(|old| old.context != plan.context)
            {
                return Err(BusinessError::stale());
            }
            if plan.epoch != scope.epoch.unwrap() {
                return Err(BusinessError::stale());
            }
            session.plans.insert(plan.id, plan.clone());
        }
        Event::FileChange { change } => {
            context(session, scope, change.context)?;
            if session
                .changes
                .get(&change.id)
                .is_some_and(|old| old.context != change.context)
            {
                return Err(BusinessError::stale());
            }
            let FileChangeSource::AgentReport { tool_call_id } = change.source;
            if tool_call_id.is_some_and(|id| {
                !session
                    .tools
                    .get(&id)
                    .is_some_and(|tool| tool.context == change.context)
            }) {
                return Err(BusinessError::invalid(
                    "file change tool provenance belongs to another run",
                ));
            }
            if change.epoch != scope.epoch.unwrap() {
                return Err(BusinessError::stale());
            }
            session.changes.insert(change.id, change.clone());
        }
        Event::Attachment { attachment } => {
            if attachment.session_id != session.record.id() {
                return Err(BusinessError::stale());
            }
            session
                .attachments
                .insert(attachment.id, attachment.clone());
        }
        Event::Permission { request } => {
            request_context(session, scope, &request.context)?;
            if session
                .requests
                .permissions
                .contains_key(&request.context.id)
                || session.requests.questions.contains_key(&request.context.id)
            {
                return Ok(ApplyDisposition::Stale);
            }
            if !matches!(request.status, RequestStatus::Pending) {
                return Err(BusinessError::invalid("new request must be pending"));
            }
            session
                .requests
                .permissions
                .insert(request.context.id, request.clone());
        }
        Event::Question { request } => {
            request_context(session, scope, &request.context)?;
            if session
                .requests
                .permissions
                .contains_key(&request.context.id)
                || session.requests.questions.contains_key(&request.context.id)
            {
                return Ok(ApplyDisposition::Stale);
            }
            if !matches!(request.status, RequestStatus::Pending) {
                return Err(BusinessError::invalid("new question must be pending"));
            }
            let mut ids = std::collections::BTreeSet::new();
            if request.fields.iter().any(|f| !ids.insert(&f.id)) {
                return Err(BusinessError::invalid("question field IDs are duplicated"));
            }
            session
                .requests
                .questions
                .insert(request.context.id, request.clone());
        }
        Event::RequestInvalidated { reason } => {
            let id = scope.request_id.ok_or_else(BusinessError::stale)?;
            if let Some(request) = session.requests.permissions.get_mut(&id) {
                if request.context.run.run_id != scope.run_id.ok_or_else(BusinessError::stale)?
                    || !request.status.is_waiting()
                {
                    return Ok(ApplyDisposition::Stale);
                }
                request.status = RequestStatus::Invalidated {
                    invalidated_at: at,
                    reason: reason.clone(),
                };
            } else if let Some(request) = session.requests.questions.get_mut(&id) {
                if request.context.run.run_id != scope.run_id.ok_or_else(BusinessError::stale)?
                    || !request.status.is_waiting()
                {
                    return Ok(ApplyDisposition::Stale);
                }
                request.status = RequestStatus::Invalidated {
                    invalidated_at: at,
                    reason: reason.clone(),
                };
            } else {
                return Ok(ApplyDisposition::Stale);
            }
        }
        Event::PermissionDelivery {
            attempt_id,
            delivery,
        } => {
            let id = scope.request_id.ok_or_else(BusinessError::stale)?;
            let request = session
                .requests
                .permissions
                .get_mut(&id)
                .ok_or_else(BusinessError::stale)?;
            if scope.run_id != Some(request.context.run.run_id) {
                return Err(BusinessError::stale());
            }
            if !deliver(&mut request.status, *attempt_id, delivery, at) {
                return Ok(ApplyDisposition::Stale);
            }
        }
        Event::QuestionDelivery {
            attempt_id,
            delivery,
        } => {
            let id = scope.request_id.ok_or_else(BusinessError::stale)?;
            let request = session
                .requests
                .questions
                .get_mut(&id)
                .ok_or_else(BusinessError::stale)?;
            if scope.run_id != Some(request.context.run.run_id) {
                return Err(BusinessError::stale());
            }
            if !deliver(&mut request.status, *attempt_id, delivery, at) {
                return Ok(ApplyDisposition::Stale);
            }
        }
        Event::Attempt {
            attempt_id,
            outcome,
        } => return super::receipt::apply(session, envelope, *attempt_id, outcome, effects),
        Event::SubmissionReconciled {
            submission_id,
            attempt_id,
            attempt_epoch,
            evidence,
        } => {
            let id = scope.request_id.ok_or_else(BusinessError::stale)?;
            let Some(target) = session.reconciliations.get(&id) else {
                return Ok(ApplyDisposition::Stale);
            };
            if target.submission_id != *submission_id
                || target.attempt_id != *attempt_id
                || target.attempt_epoch != *attempt_epoch
                || Some(target.connection_epoch) != scope.epoch
            {
                return Ok(ApplyDisposition::Stale);
            }
            if !matches!(envelope.source.origin, EventOrigin::Runtime { .. }) {
                return Err(BusinessError::invalid(
                    "reconciliation requires an authoritative Runtime report",
                ));
            }
            let attempt = session
                .attempts
                .get(attempt_id)
                .ok_or_else(BusinessError::stale)?
                .clone();
            if attempt.submission_id != *submission_id
                || attempt.epoch != *attempt_epoch
                || attempt.context.session_id != session.record.id()
                || scope
                    .run_id
                    .is_some_and(|run| run != attempt.context.run_id)
            {
                return Err(BusinessError::stale());
            }
            let awaiting_end = attempt.state == AttemptState::ConsumedAwaitingEnd;
            if !validate::reconciliation_eligible(session, &attempt) {
                return Ok(ApplyDisposition::Stale);
            }
            match evidence {
                ConsumptionEvidence::Unknown { .. } => {}
                ConsumptionEvidence::NotConsumed => {
                    if awaiting_end {
                        return Err(BusinessError::invalid(
                            "known consumed input cannot become not consumed",
                        ));
                    }
                    let submission = session.submissions.get_mut(submission_id).unwrap();
                    submission.state = SubmissionState::Rejected {
                        rejected_at: at,
                        reason: "confirmed not consumed during reconciliation".into(),
                    };
                    session.attempts.get_mut(attempt_id).unwrap().state = AttemptState::NotSent {
                        reason: "confirmed not consumed".into(),
                    };
                    if let Some(queued_id) = submission.queued_input_id {
                        if let Some(q) =
                            session.queue.entries.iter_mut().find(|q| q.id == queued_id)
                        {
                            q.state = QueuedInputState::Waiting;
                        }
                    }
                    if let SubmissionIntent::StartRun { run_id } = submission.intent {
                        if session.record.current_run_id == Some(run_id) {
                            session.record.current_run_id = None;
                        }
                        if let Some(run) = session.runs.get_mut(&run_id) {
                            if !run.state.is_terminal() {
                                run.state = ExecutionState::Failed {
                                    ended_at: at,
                                    reason: "confirmed no execution: input was not consumed".into(),
                                };
                            }
                        }
                    }
                    if session.prompt_in_flight == Some(*attempt_id) {
                        session.prompt_in_flight = None;
                    }
                }
                ConsumptionEvidence::Consumed { run_id, end } => {
                    if *run_id != attempt.context.run_id
                        || !session
                            .runs
                            .get(run_id)
                            .is_some_and(|run| run.context == attempt.context)
                    {
                        return Err(BusinessError::invalid(
                            "reconciliation must map to the original attempt's local run",
                        ));
                    }
                    session.submissions.get_mut(submission_id).unwrap().state =
                        SubmissionState::Accepted {
                            epoch: *attempt_epoch,
                            accepted_at: at,
                            run_id: *run_id,
                            message_id: None,
                            external_id: None,
                        };
                    if let Some(queued_id) = session.submissions[submission_id].queued_input_id {
                        let queued = session
                            .queue
                            .entries
                            .iter_mut()
                            .find(|q| q.id == queued_id)
                            .ok_or_else(|| {
                                BusinessError::invalid("reconciled queued input is missing")
                            })?;
                        if !matches!(queued.state, QueuedInputState::Dispatching { submission_id: owner } | QueuedInputState::Submitted { submission_id: owner } if owner == *submission_id)
                        {
                            return Err(BusinessError::invalid(
                                "queue entry does not belong to the reconciled submission",
                            ));
                        }
                        queued.state = QueuedInputState::Submitted {
                            submission_id: *submission_id,
                        };
                    }
                    if let Some(end) = end {
                        session.attempts.get_mut(attempt_id).unwrap().state =
                            AttemptState::Returned;
                        if session.prompt_in_flight == Some(*attempt_id) {
                            session.prompt_in_flight = None;
                        }
                        finish(session, *run_id, end, at);
                        if session.record.current_run_id == Some(*run_id) {
                            session.record.current_run_id = None;
                        }
                    } else {
                        session.attempts.get_mut(attempt_id).unwrap().state =
                            AttemptState::ConsumedAwaitingEnd;
                        if session.record.current_run_id.is_none() {
                            session.record.current_run_id = Some(*run_id);
                            session.runs.get_mut(run_id).unwrap().state =
                                ExecutionState::OutcomeUnknown {
                                    observed_at: at,
                                    reason:
                                        "input consumed; execution completion still unconfirmed"
                                            .into(),
                                };
                        }
                    }
                }
            }
            session.reconciliations.remove(&id);
        }
        Event::Error { error } => {
            session.errors.push(error.clone());
            if error.retry == RetryAdvice::ReconcileFirst {
                session.pause(QueuePauseReason::SubmissionOutcomeUnknown, at);
            }
        }
        Event::Unsupported {
            type_name,
            diagnostic,
        } => {
            session.diagnostics.push(DiagnosticRecord {
                event_id: envelope.id,
                type_name: type_name.clone(),
                payload: diagnostic.clone(),
            });
        }
    }
    Ok(ApplyDisposition::Applied)
}

fn run_scope(session: &SessionState, scope: Scope) -> Result<RunId, BusinessError> {
    let id = scope
        .run_id
        .ok_or_else(|| BusinessError::invalid("run scope is required"))?;
    if session.record.current_run_id != Some(id) {
        return Err(BusinessError::stale());
    }
    Ok(id)
}
fn context(session: &SessionState, scope: Scope, context: RunContext) -> Result<(), BusinessError> {
    if context.session_id != session.record.id() || Some(context.run_id) != scope.run_id {
        return Err(BusinessError::stale());
    }
    run_scope(session, scope).map(|_| ())
}
fn request_context(
    session: &SessionState,
    scope: Scope,
    c: &RequestContext,
) -> Result<(), BusinessError> {
    context(session, scope, c.run)?;
    if Some(c.id) != scope.request_id
        || Some(c.epoch) != scope.epoch
        || session
            .current_run()
            .is_some_and(|r| matches!(r.state, ExecutionState::Cancelling { .. }))
    {
        return Err(BusinessError::stale());
    }
    if let Some(id) = c.tool_call_id {
        if !session.tools.get(&id).is_some_and(|t| t.context == c.run) {
            return Err(BusinessError::invalid("request tool owner does not match"));
        }
    }
    Ok(())
}
fn blocks(
    session: &SessionState,
    blocks: &[ContentBlock],
    owner: RunContext,
) -> Result<(), BusinessError> {
    let mut ids = std::collections::BTreeSet::new();
    for block in blocks {
        if !ids.insert(block.id) {
            return Err(BusinessError::invalid("duplicate content block ID"));
        }
        match &block.content {
            ContentKind::ToolCall { tool_call_id }
                if !session
                    .tools
                    .get(tool_call_id)
                    .is_some_and(|t| t.context == owner) =>
            {
                return Err(BusinessError::invalid(
                    "tool reference does not belong to this run",
                ));
            }
            ContentKind::Plan { plan_id }
                if !session
                    .plans
                    .get(plan_id)
                    .is_some_and(|p| p.context == owner) =>
            {
                return Err(BusinessError::invalid(
                    "plan reference does not belong to this run",
                ));
            }
            ContentKind::FileChange { file_change_id }
                if !session
                    .changes
                    .get(file_change_id)
                    .is_some_and(|c| c.context == owner) =>
            {
                return Err(BusinessError::invalid(
                    "change reference does not belong to this run",
                ));
            }
            ContentKind::Image { attachment_id, .. }
            | ContentKind::Attachment { attachment_id }
                if !session.attachments.contains_key(attachment_id) =>
            {
                return Err(BusinessError::invalid(
                    "message attachment has no matching session owner",
                ));
            }
            ContentKind::FileReference { reference }
                if reference.project_id != session.record.project_id() =>
            {
                return Err(BusinessError::stale());
            }
            _ => {}
        }
    }
    Ok(())
}
fn deliver<R: Clone>(
    status: &mut RequestStatus<R>,
    id: ResponseAttemptId,
    delivery: &ResponseDelivery,
    at: Timestamp,
) -> bool {
    let RequestStatus::Submitting {
        attempt_id,
        response,
        ..
    } = status
    else {
        return false;
    };
    if *attempt_id != id {
        return false;
    }
    let response = response.clone();
    *status = match delivery {
        ResponseDelivery::Submitted => RequestStatus::ResponseSubmitted {
            submitted_at: at,
            response,
        },
        ResponseDelivery::RejectedLocally { reason } => RequestStatus::ResponseRejected {
            rejected_at: at,
            response,
            reason: reason.clone(),
        },
        ResponseDelivery::Unknown { reason } => RequestStatus::OutcomeUnknown {
            attempt_id: id,
            observed_at: at,
            response,
            reason: reason.clone(),
        },
    };
    true
}
fn next_configuration(
    session: &mut SessionState,
    scope: Scope,
    cause: EffectCause,
    at: Timestamp,
    effects: &mut Vec<RuntimeEffect>,
) -> Result<(), BusinessError> {
    if let Some(pending) = session.next_configuration.take() {
        match validate::configuration(validate::ready(session)?, &pending.requested) {
            Ok(()) => {
                session.record.configuration.pending = Some(pending.clone());
                effect(
                    effects,
                    cause,
                    Scope::session(session.record.id(), scope.epoch.unwrap())
                        .with_request(pending.request_id),
                    RuntimeOperation::Configure { pending },
                );
            }
            Err(error) => {
                session.record.configuration.last_rejection =
                    Some(ConfigurationRejection::new(pending, at, &error.message));
                session.pause(
                    QueuePauseReason::CapabilityChanged {
                        reason: error.message,
                    },
                    at,
                );
            }
        }
    }
    Ok(())
}

pub(super) fn finish(session: &mut SessionState, run_id: RunId, end: &RunEnd, at: Timestamp) {
    let Some(run) = session.runs.get_mut(&run_id) else {
        return;
    };
    if run.state.is_terminal() {
        return;
    }
    run.state = match end {
        RunEnd::Completed { result } => ExecutionState::Completed {
            ended_at: at,
            result: result.clone(),
        },
        RunEnd::Cancelled => ExecutionState::Cancelled { ended_at: at },
        RunEnd::Failed { reason } => ExecutionState::Failed {
            ended_at: at,
            reason: reason.clone(),
        },
    };
    if session.record.current_run_id == Some(run_id) {
        session.record.current_run_id = None;
    }
    for tool in session
        .tools
        .values_mut()
        .filter(|t| t.context.run_id == run_id)
    {
        if matches!(tool.state, ToolState::Pending | ToolState::Running) {
            tool.state = ToolState::Interrupted {
                observed_at: at,
                reason: "run ended without a confirmed tool result".into(),
            };
        }
    }
    for message in session
        .messages
        .iter_mut()
        .filter(|m| m.context.run_id == run_id)
    {
        if matches!(message.state, MessageState::Streaming) {
            message.state = if matches!(
                end,
                RunEnd::Completed {
                    result: RunResult::EndOfTurn
                }
            ) {
                MessageState::Complete
            } else {
                MessageState::Interrupted {
                    reason: "run ended abnormally".into(),
                }
            };
        }
    }
    if !matches!(
        end,
        RunEnd::Completed {
            result: RunResult::EndOfTurn
        }
    ) {
        session.pause(
            if matches!(end, RunEnd::Cancelled) {
                QueuePauseReason::RunStopped
            } else {
                QueuePauseReason::RunFailed
            },
            at,
        );
    }
}
