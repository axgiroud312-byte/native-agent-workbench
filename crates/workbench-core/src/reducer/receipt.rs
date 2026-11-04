use super::{ApplyDisposition, effect};
use crate::state::SessionState;
use crate::*;

pub(super) fn apply(
    session: &mut SessionState,
    envelope: &EventEnvelope,
    id: AttemptId,
    outcome: &AttemptOutcome,
    effects: &mut Vec<RuntimeEffect>,
) -> Result<ApplyDisposition, BusinessError> {
    let attempt = session
        .attempts
        .get(&id)
        .ok_or_else(BusinessError::stale)?
        .clone();
    let scope = envelope.scope;
    let at = envelope.observed_at;
    if scope.run_id != Some(attempt.context.run_id)
        || scope.request_id != Some(attempt.request_id)
        || scope.epoch != Some(attempt.epoch)
    {
        return Err(BusinessError::stale());
    }
    if matches!(
        attempt.state,
        AttemptState::Returned
            | AttemptState::Injected
            | AttemptState::StartedNewTurn { .. }
            | AttemptState::NotConsumed
            | AttemptState::NotSent { .. }
    ) {
        return Ok(ApplyDisposition::Stale);
    }
    let is_prompt = matches!(attempt.method, TransportMethod::Prompt);
    if attempt.state == AttemptState::ConsumedAwaitingEnd
        && !matches!(outcome, AttemptOutcome::PromptReturned { .. })
    {
        return Ok(ApplyDisposition::Stale);
    }
    let attempt_state;
    match outcome {
        AttemptOutcome::PromptAccepted if is_prompt => {
            accepted(session, &attempt, attempt.context.run_id, at);
            attempt_state = AttemptState::Accepted;
            if let Some(run) = session.runs.get_mut(&attempt.context.run_id) {
                if matches!(run.state, ExecutionState::Starting) {
                    run.state = ExecutionState::Running;
                    run.started_at = Some(at);
                }
            }
        }
        AttemptOutcome::PromptReturned { end } if is_prompt => {
            accepted(session, &attempt, attempt.context.run_id, at);
            attempt_state = AttemptState::Returned;
            if session.prompt_in_flight == Some(id) {
                session.prompt_in_flight = None;
            }
            super::event::finish(session, attempt.context.run_id, end, at);
            let mut ids = Vec::new();
            for request in session
                .requests
                .permissions
                .values_mut()
                .filter(|r| r.context.run.run_id == attempt.context.run_id)
            {
                if request.status.is_waiting() {
                    ids.push(request.context.id);
                    request.status = RequestStatus::Invalidated {
                        invalidated_at: at,
                        reason: RequestInvalidationReason::RunEnded,
                    };
                }
            }
            for request in session
                .requests
                .questions
                .values_mut()
                .filter(|r| r.context.run.run_id == attempt.context.run_id)
            {
                if request.status.is_waiting() {
                    ids.push(request.context.id);
                    request.status = RequestStatus::Invalidated {
                        invalidated_at: at,
                        reason: RequestInvalidationReason::RunEnded,
                    };
                }
            }
            if !ids.is_empty() {
                effect(
                    effects,
                    EffectCause::Event { id: envelope.id },
                    scope,
                    RuntimeOperation::CancelRequests { ids },
                );
            }
        }
        AttemptOutcome::Steering { receipt } if !is_prompt => match receipt {
            SteeringReceipt::Injected => {
                accepted(session, &attempt, attempt.context.run_id, at);
                attempt_state = AttemptState::Injected;
            }
            SteeringReceipt::StartedNewTurn { local_run_id } => {
                if session.runs.contains_key(local_run_id) {
                    return Err(BusinessError::invalid(
                        "adapter-owned new turn requires a fresh local run ID",
                    ));
                }
                accepted(session, &attempt, *local_run_id, at);
                let mut run = Run::new(
                    RunContext::new(session.record.id(), *local_run_id),
                    attempt.epoch,
                    at,
                );
                run.state = ExecutionState::Running;
                run.started_at = Some(at);
                let stop = session
                    .cancel_requests
                    .get(&attempt.context.run_id)
                    .copied();
                if let Some(request_id) = stop {
                    run.state = ExecutionState::Cancelling {
                        request_id,
                        requested_at: at,
                    };
                    session.cancel_requests.insert(*local_run_id, request_id);
                    effect(
                        effects,
                        EffectCause::Event { id: envelope.id },
                        Scope::session(session.record.id(), attempt.epoch)
                            .with_run(*local_run_id)
                            .with_request(request_id),
                        RuntimeOperation::Cancel,
                    );
                }
                // The extension did not carry configuration or remote identity/completion proof.
                session.runs.insert(*local_run_id, run);
                session.record.current_run_id = Some(*local_run_id);
                if stop.is_some() {
                    session.pause(QueuePauseReason::RunStopped, at);
                } else {
                    session.pause(
                        QueuePauseReason::CapabilityChanged {
                            reason:
                                "adapter-owned new turn needs authoritative completion tracking"
                                    .into(),
                        },
                        at,
                    );
                }
                attempt_state = AttemptState::StartedNewTurn {
                    run_id: *local_run_id,
                };
            }
            SteeringReceipt::PromptRequired => {
                if !matches!(
                    attempt.method,
                    TransportMethod::Steering {
                        prompt_required: true
                    }
                ) {
                    return Err(BusinessError::invalid(
                        "unnegotiated promptRequired receipt",
                    ));
                }
                session
                    .submissions
                    .get_mut(&attempt.submission_id)
                    .unwrap()
                    .state = SubmissionState::NotConsumed {
                    epoch: attempt.epoch,
                    attempt_id: id,
                    observed_at: at,
                };
                attempt_state = AttemptState::NotConsumed;
            }
            SteeringReceipt::Failed { reason } => {
                unknown(session, &attempt, at, reason);
                attempt_state = AttemptState::Unknown {
                    reason: reason.clone(),
                };
            }
        },
        AttemptOutcome::NotSent { reason } => {
            if matches!(attempt.state, AttemptState::Accepted) {
                return Err(BusinessError::invalid(
                    "consumed input cannot become not-sent",
                ));
            }
            attempt_state = AttemptState::NotSent {
                reason: reason.clone(),
            };
            session
                .submissions
                .get_mut(&attempt.submission_id)
                .unwrap()
                .state = SubmissionState::Rejected {
                rejected_at: at,
                reason: reason.clone(),
            };
            if is_prompt {
                if session.prompt_in_flight == Some(id) {
                    session.prompt_in_flight = None;
                }
                super::event::finish(
                    session,
                    attempt.context.run_id,
                    &RunEnd::Failed {
                        reason: reason.clone(),
                    },
                    at,
                );
            }
            if let Some(queued) = session.submissions[&attempt.submission_id].queued_input_id {
                if let Some(q) = session.queue.entries.iter_mut().find(|q| q.id == queued) {
                    q.state = QueuedInputState::Waiting;
                }
            }
            session.pause(QueuePauseReason::RunFailed, at);
        }
        AttemptOutcome::Unknown { reason } => {
            unknown(session, &attempt, at, reason);
            attempt_state = AttemptState::Unknown {
                reason: reason.clone(),
            };
        }
        _ => {
            return Err(BusinessError::invalid(
                "transport result does not match its method",
            ));
        }
    }
    session.attempts.get_mut(&id).unwrap().state = attempt_state;
    Ok(ApplyDisposition::Applied)
}

fn accepted(session: &mut SessionState, attempt: &TransportAttempt, run_id: RunId, at: Timestamp) {
    let submission = session.submissions.get_mut(&attempt.submission_id).unwrap();
    if !matches!(submission.state, SubmissionState::Accepted { .. }) {
        submission.state = SubmissionState::Accepted {
            epoch: attempt.epoch,
            accepted_at: at,
            run_id,
            message_id: None,
            external_id: None,
        };
    }
    if let Some(id) = submission.queued_input_id {
        if let Some(q) = session.queue.entries.iter_mut().find(|q| q.id == id) {
            q.state = QueuedInputState::Submitted {
                submission_id: submission.id,
            };
        }
    }
    // Clear only the exact accepted draft; later user edits and unknown/failed delivery survive.
    if submission.source_draft_revision == Some(session.draft.revision)
        && submission.content == session.draft.content
        && submission.configuration == session.draft.configuration
    {
        if let Some(revision) = session.draft.revision.checked_add(1) {
            session.draft.content.clear();
            session.draft.revision = revision;
            session.draft.updated_at = at;
        }
    }
}
fn unknown(session: &mut SessionState, attempt: &TransportAttempt, at: Timestamp, reason: &str) {
    let submission = session.submissions.get_mut(&attempt.submission_id).unwrap();
    if !matches!(submission.state, SubmissionState::Accepted { .. }) {
        submission.state = SubmissionState::OutcomeUnknown {
            epoch: attempt.epoch,
            observed_at: at,
            reason: reason.into(),
        };
    }
    session.pause(QueuePauseReason::SubmissionOutcomeUnknown, at);
    if matches!(attempt.method, TransportMethod::Prompt) {
        if let Some(run) = session.runs.get_mut(&attempt.context.run_id) {
            if !run.state.is_terminal() {
                run.state = ExecutionState::OutcomeUnknown {
                    observed_at: at,
                    reason: reason.into(),
                };
            }
        }
    }
}
