use super::{effect, interrupt, invalidate, session_mut, validate};
use crate::state::SessionState;
use crate::*;

pub(super) fn apply(
    state: &mut WorkbenchState,
    envelope: &CommandEnvelope,
    effects: &mut Vec<RuntimeEffect>,
) -> Result<(), BusinessError> {
    let scope = envelope.scope;
    let at = envelope.issued_at;
    let cause = EffectCause::Command { id: envelope.id };
    match &envelope.command {
        Command::PutProject { project } => {
            if state.deleted_projects.contains(&project.id) {
                return Err(BusinessError::stale());
            }
            if state
                .projects
                .get(&project.id)
                .is_some_and(|old| old.directory != project.directory)
                && state
                    .sessions
                    .values()
                    .any(|s| s.record.project_id() == project.id && !s.resources_released)
            {
                return Err(BusinessError::busy(
                    "release all project connections before explicitly relocating its directory",
                ));
            }
            state.projects.insert(project.id, project.clone());
            return Ok(());
        }
        Command::PutAgent { agent } => {
            state.agents.insert(agent.id, agent.clone());
            return Ok(());
        }
        Command::CreateSession { session } => {
            if state.sessions.contains_key(&session.id())
                || state.deleted_sessions.contains(&session.id())
            {
                return Err(BusinessError::stale());
            }
            if !state.projects.contains_key(&session.project_id())
                || !state.agents.contains_key(&session.agent_id())
            {
                return Err(BusinessError::invalid("session project/Agent must exist"));
            }
            let expected = SessionRecord::new(
                session.id(),
                session.project_id(),
                session.agent_id(),
                &session.title,
                session.created_at,
            );
            if *session != expected {
                return Err(BusinessError::invalid(
                    "create-session cannot inject external bindings or runtime state",
                ));
            }
            state
                .sessions
                .insert(session.id(), SessionState::new(session.clone()));
            return Ok(());
        }
        Command::DeleteProject { project_id } => {
            if !state.projects.contains_key(project_id) {
                return Err(BusinessError::stale());
            }
            let ids: Vec<_> = state
                .sessions
                .iter()
                .filter(|(_, s)| s.record.project_id() == *project_id)
                .map(|(id, _)| *id)
                .collect();
            if ids.iter().any(|id| !state.sessions[id].resources_released) {
                return Err(BusinessError::busy(
                    "close and release affected sessions before deleting project index",
                ));
            }
            for id in ids {
                state.sessions.remove(&id);
                state.deleted_sessions.insert(id);
            }
            state.projects.remove(project_id);
            state.deleted_projects.insert(*project_id);
            return Ok(());
        }
        Command::DeleteSession => {
            let session = session_mut(state, scope)?;
            if !session.resources_released {
                return Err(BusinessError::busy(
                    "runtime resources must be released before deleting a session",
                ));
            }
            let id = session.record.id();
            state.sessions.remove(&id);
            state.deleted_sessions.insert(id);
            return Ok(());
        }
        Command::Restart => {
            for session in state.sessions.values_mut() {
                if session.closing && !session.resources_released {
                    let abandoned = session
                        .closing_handoff
                        .as_ref()
                        .map_or(session.record.connection.epoch, |h| h.abandoned_epoch);
                    session.closing_handoff = Some(ClosingResourceHandoff::new(abandoned));
                }
                invalidate(session, RequestInvalidationReason::Disconnected, at);
                interrupt(
                    session,
                    at,
                    "application restarted before confirmed completion",
                );
                session.record.connection.epoch =
                    session.record.connection.epoch.next().ok_or_else(|| {
                        BusinessError::new(
                            ErrorKind::Exhausted,
                            "connection epoch exhausted",
                            RetryAdvice::Never,
                        )
                    })?;
                session.record.connection.phase = ConnectionPhase::Disconnected {
                    reason: "application restarted".into(),
                };
                session.record.resume = SessionResumeState::NotChecked;
                session.prompt_in_flight = None;
                session.pause(QueuePauseReason::ApplicationRestart, at);
            }
            return Ok(());
        }
        Command::Connect { resume } => {
            let session = session_mut(state, scope)?;
            if session.closing {
                return Err(BusinessError::busy(
                    "wait for old resources to be released before reconnecting",
                ));
            }
            if !matches!(
                session.record.connection.phase,
                ConnectionPhase::NotStarted
                    | ConnectionPhase::Disconnected { .. }
                    | ConnectionPhase::Failed { .. }
                    | ConnectionPhase::Closed
            ) {
                return Err(BusinessError::busy(
                    "connection is already starting or ready",
                ));
            }
            if *resume && session.record.external.is_none() {
                return Err(BusinessError::invalid(
                    "no registered workbench session to resume",
                ));
            }
            if !resume && session.record.external.is_some() {
                return Err(BusinessError::invalid(
                    "existing external session must be explicitly resumed, never silently replaced",
                ));
            }
            invalidate(session, RequestInvalidationReason::Disconnected, at);
            interrupt(session, at, "connection replaced");
            let epoch = session.record.connection.epoch.next().ok_or_else(|| {
                BusinessError::new(
                    ErrorKind::Exhausted,
                    "connection epoch exhausted",
                    RetryAdvice::Never,
                )
            })?;
            session.record.connection.epoch = epoch;
            session.record.connection.phase = ConnectionPhase::Starting;
            session.resources_released = false;
            session.prompt_in_flight = None;
            if *resume {
                session.record.resume = SessionResumeState::Resuming { epoch };
            }
            let project_id = session.record.project_id();
            let agent_id = session.record.agent_id();
            let external = session.record.external.clone();
            let id = session.record.id();
            let project = state.projects[&project_id].clone();
            let agent = state.agents[&agent_id].clone();
            effect(
                effects,
                cause,
                Scope::session(id, epoch),
                RuntimeOperation::Connect {
                    project,
                    agent,
                    external,
                    resume: *resume,
                },
            );
            return Ok(());
        }
        _ => {}
    }
    let session = session_mut(state, scope)?;
    match &envelope.command {
        Command::RenameSession { title } => {
            session.record.title = title.clone();
        }
        Command::SaveDraft {
            draft,
            expected_revision,
        } => {
            if draft.session_id != session.record.id()
                || *expected_revision != session.draft.revision
                || draft.revision
                    != expected_revision
                        .checked_add(1)
                        .ok_or_else(|| BusinessError::invalid("draft revision exhausted"))?
            {
                return Err(BusinessError::stale());
            }
            owners(session, &draft.content)?;
            session.draft = draft.clone();
        }
        Command::PutAttachment { attachment } => {
            if attachment.session_id != session.record.id() {
                return Err(BusinessError::stale());
            }
            session
                .attachments
                .insert(attachment.id, attachment.clone());
        }
        Command::Enqueue { input } => {
            if input.session_id != session.record.id()
                || session.queue.entries.iter().any(|q| q.id == input.id)
                || !matches!(input.state, QueuedInputState::Waiting)
            {
                return Err(BusinessError::invalid(
                    "invalid queue identity or initial state",
                ));
            }
            owners(session, &input.content)?;
            session.queue.entries.push(input.clone());
        }
        Command::EditQueued {
            id,
            content,
            configuration,
        } => {
            owners(session, content)?;
            let q = editable(session, *id)?;
            q.content = content.clone();
            q.configuration = configuration.clone();
            q.updated_at = at;
        }
        Command::SetQueueEditing { id, editing } => {
            editable(session, *id)?.state = if *editing {
                QueuedInputState::Editing
            } else {
                QueuedInputState::Waiting
            };
        }
        Command::CancelQueued { id } => {
            editable(session, *id)?.state = QueuedInputState::Cancelled { cancelled_at: at };
        }
        Command::PauseQueue => session.pause(QueuePauseReason::User, at),
        Command::ResumeQueue => {
            validate::ready(session)?;
            validate::pending_submission(session, None)?;
            if session.current_run().is_some_and(|r| {
                matches!(
                    r.state,
                    ExecutionState::Cancelling { .. }
                        | ExecutionState::OutcomeUnknown { .. }
                        | ExecutionState::Interrupted { .. }
                )
            }) {
                return Err(BusinessError::busy(
                    "reconcile in-flight/unknown work before resuming queue",
                ));
            }
            session.queue.dispatch = QueueDispatchState::Ready;
        }
        Command::Submit {
            submission,
            attempt_id,
        } => {
            if submission.session_id != session.record.id()
                || submission.state != SubmissionState::Prepared
                || session.submissions.contains_key(&submission.id)
            {
                return Err(BusinessError::invalid(
                    "submission identity/state is invalid or already used",
                ));
            }
            let method = match submission.intent {
                SubmissionIntent::StartRun { run_id } => {
                    validate::pending_submission(session, None)?;
                    validate::input(session, &submission.content, &submission.configuration)?;
                    fresh_prompt(session)?;
                    if submission.queued_input_id.is_some()
                        && !matches!(session.queue.dispatch, QueueDispatchState::Ready)
                    {
                        return Err(BusinessError::busy("automatic queue dispatch is paused"));
                    }
                    if scope.run_id != Some(run_id) || session.runs.contains_key(&run_id) {
                        return Err(BusinessError::stale());
                    }
                    let head = session.queue.entries.iter().find(|q| {
                        matches!(
                            q.state,
                            QueuedInputState::Waiting
                                | QueuedInputState::Editing
                                | QueuedInputState::Dispatching { .. }
                        )
                    });
                    match (submission.queued_input_id, head) {
                        (Some(id), Some(q))
                            if q.id == id
                                && q.state == QueuedInputState::Waiting
                                && q.content == submission.content
                                && q.configuration == submission.configuration => {}
                        (None, None) => {}
                        _ => {
                            return Err(BusinessError::busy(
                                "queue head must be dispatched in user order with its current frozen content",
                            ));
                        }
                    }
                    TransportMethod::Prompt
                }
                SubmissionIntent::SteerRun { run_id } => {
                    if scope.run_id != Some(run_id) || submission.queued_input_id.is_some() {
                        return Err(BusinessError::stale());
                    }
                    validate::steering(
                        session,
                        run_id,
                        &submission.content,
                        &submission.configuration,
                    )?;
                    TransportMethod::Steering {
                        prompt_required: true,
                    }
                }
            };
            session
                .submissions
                .insert(submission.id, submission.clone());
            dispatch(
                session,
                submission.id,
                *attempt_id,
                envelope,
                method,
                effects,
            )?;
        }
        Command::FallbackToPrompt {
            submission_id,
            attempt_id,
            run_id,
        } => {
            fresh_prompt(session)?;
            if !matches!(session.queue.dispatch, QueueDispatchState::Ready) {
                return Err(BusinessError::busy(
                    "steering fallback is paused with the queue",
                ));
            }
            let submission = session
                .submissions
                .get(submission_id)
                .ok_or_else(|| BusinessError::invalid("submission not found"))?;
            let last = session
                .attempts
                .values()
                .filter(|a| a.submission_id == *submission_id)
                .collect::<Vec<_>>();
            if last.len() != 1
                || last[0].state != AttemptState::NotConsumed
                || !matches!(
                    last[0].method,
                    TransportMethod::Steering {
                        prompt_required: true
                    }
                )
                || last[0].epoch != session.record.connection.epoch
                || scope.run_id != Some(*run_id)
                || session.runs.contains_key(run_id)
            {
                return Err(BusinessError::invalid(
                    "fallback requires one explicit unconsumed steering receipt and a new local run",
                ));
            }
            validate::input(session, &submission.content, &submission.configuration)?;
            session.submissions.get_mut(submission_id).unwrap().intent =
                SubmissionIntent::StartRun { run_id: *run_id };
            dispatch(
                session,
                *submission_id,
                *attempt_id,
                envelope,
                TransportMethod::Prompt,
                effects,
            )?;
        }
        Command::ReconcileSubmission {
            submission_id,
            attempt_id,
            attempt_epoch,
        } => {
            validate::ready(session)?;
            let request_id = scope.request_id.ok_or_else(|| {
                BusinessError::invalid("reconciliation request identity is required")
            })?;
            let attempt = session
                .attempts
                .get(attempt_id)
                .ok_or_else(BusinessError::stale)?
                .clone();
            if attempt.submission_id != *submission_id
                || attempt.epoch != *attempt_epoch
                || scope.run_id.is_some_and(|id| id != attempt.context.run_id)
            {
                return Err(BusinessError::stale());
            }
            if session.reconciliations.contains_key(&request_id)
                || !validate::reconciliation_eligible(session, &attempt)
            {
                return Err(BusinessError::stale());
            }
            session.reconciliations.insert(
                request_id,
                crate::state::ReconciliationTarget {
                    submission_id: *submission_id,
                    attempt_id: *attempt_id,
                    attempt_epoch: *attempt_epoch,
                    connection_epoch: session.record.connection.epoch,
                },
            );
            effect(
                effects,
                cause,
                scope,
                RuntimeOperation::ReconcileSubmission {
                    submission_id: *submission_id,
                    attempt,
                },
            );
        }
        Command::Stop => {
            let run_id = session
                .record
                .current_run_id
                .ok_or_else(|| BusinessError::busy("no active run"))?;
            if scope.run_id != Some(run_id) {
                return Err(BusinessError::stale());
            }
            let request_id = scope
                .request_id
                .ok_or_else(|| BusinessError::invalid("cancel request identity is required"))?;
            session.pause(QueuePauseReason::RunStopped, at);
            session.cancel_requests.insert(run_id, request_id);
            let ids = invalidate(session, RequestInvalidationReason::RunCancelled, at);
            if !ids.is_empty() {
                effect(
                    effects,
                    cause.clone(),
                    scope,
                    RuntimeOperation::CancelRequests { ids },
                );
            }
            let run = session.runs.get_mut(&run_id).unwrap();
            if !run.state.is_terminal() && !matches!(run.state, ExecutionState::Cancelling { .. }) {
                run.state = ExecutionState::Cancelling {
                    request_id,
                    requested_at: at,
                };
                effect(effects, cause, scope, RuntimeOperation::Cancel);
            }
        }
        Command::ReconcileClosingResources { abandoned_epoch } => {
            let request_id = scope.request_id.ok_or_else(|| {
                BusinessError::invalid("resource handoff request identity is required")
            })?;
            let handoff = session
                .closing_handoff
                .as_mut()
                .ok_or_else(BusinessError::stale)?;
            if !session.closing
                || session.resources_released
                || handoff.abandoned_epoch != *abandoned_epoch
                || handoff.request_id.is_some()
            {
                return Err(BusinessError::stale());
            }
            handoff.request_id = Some(request_id);
            effect(
                effects,
                cause,
                scope,
                RuntimeOperation::ReconcileClosingResources {
                    abandoned_epoch: *abandoned_epoch,
                },
            );
        }
        Command::CloseSession => {
            if session.closing_handoff.is_some() {
                return Err(BusinessError::busy(
                    "reconcile abandoned resources in the new instance",
                ));
            }
            session.closing = true;
            session.pause(QueuePauseReason::Disconnected, at);
            let ids = invalidate(session, RequestInvalidationReason::Disconnected, at);
            interrupt(session, at, "runtime is closing");
            session.record.connection.phase = ConnectionPhase::Disconnected {
                reason: "closing runtime resources".into(),
            };
            if !ids.is_empty() {
                effect(
                    effects,
                    cause.clone(),
                    scope,
                    RuntimeOperation::CancelRequests { ids },
                );
            }
            effect(effects, cause, scope, RuntimeOperation::ReleaseSession);
        }
        Command::Configure {
            requested,
            capability_revision,
        } => {
            let caps = validate::ready(session)?;
            if caps.revision != *capability_revision {
                return Err(BusinessError::stale());
            }
            validate::configuration(caps, requested)?;
            if session.current_run().is_some() {
                validate::supported(&caps.configuration_during_run, "configuration during a run")?;
            }
            let id = scope.request_id.ok_or_else(|| {
                BusinessError::invalid("configuration request identity is required")
            })?;
            if session
                .record
                .configuration
                .pending
                .as_ref()
                .is_some_and(|p| p.request_id == id)
            {
                return Err(BusinessError::stale());
            }
            let pending = PendingConfiguration::new(
                id,
                session.record.connection.epoch,
                at,
                requested.clone(),
            );
            session.record.configuration.requested = requested.clone();
            if session.record.configuration.pending.is_some() {
                session.next_configuration = Some(pending);
            } else {
                session.record.configuration.pending = Some(pending.clone());
                effect(
                    effects,
                    cause,
                    scope,
                    RuntimeOperation::Configure { pending },
                );
            }
        }
        Command::RespondPermission {
            response,
            attempt_id,
        } => {
            let id = scope
                .request_id
                .ok_or_else(|| BusinessError::invalid("request ID required"))?;
            let request = session
                .requests
                .permissions
                .get(&id)
                .ok_or_else(BusinessError::stale)?;
            validate::request_scope(session, scope, &request.context)?;
            if !request.status.is_actionable() {
                return Err(BusinessError::stale());
            }
            if let PermissionResponse::Selected { option_id } = response {
                if !request.options.iter().any(|o| &o.option.id == option_id) {
                    return Err(BusinessError::invalid(
                        "permission option was not offered by Agent",
                    ));
                }
            }
            let context = request.context.clone();
            session.requests.permissions.get_mut(&id).unwrap().status = RequestStatus::Submitting {
                attempt_id: *attempt_id,
                submitted_at: at,
                response: response.clone(),
            };
            effect(
                effects,
                cause,
                scope,
                RuntimeOperation::RespondPermission {
                    context,
                    response: response.clone(),
                    attempt_id: *attempt_id,
                },
            );
        }
        Command::RespondQuestion {
            response,
            attempt_id,
        } => {
            let id = scope
                .request_id
                .ok_or_else(|| BusinessError::invalid("request ID required"))?;
            let request = session
                .requests
                .questions
                .get(&id)
                .ok_or_else(BusinessError::stale)?;
            validate::request_scope(session, scope, &request.context)?;
            if !request.status.is_actionable() {
                return Err(BusinessError::stale());
            }
            validate::answers(&request.fields, response)?;
            let context = request.context.clone();
            session.requests.questions.get_mut(&id).unwrap().status = RequestStatus::Submitting {
                attempt_id: *attempt_id,
                submitted_at: at,
                response: response.clone(),
            };
            effect(
                effects,
                cause,
                scope,
                RuntimeOperation::RespondQuestion {
                    context,
                    response: response.clone(),
                    attempt_id: *attempt_id,
                },
            );
        }
        _ => return Err(BusinessError::invalid("command scope is invalid")),
    }
    Ok(())
}

fn owners(session: &SessionState, content: &[InputPart]) -> Result<(), BusinessError> {
    for part in content {
        match part {
            InputPart::Attachment { attachment_id }
                if !session.attachments.contains_key(attachment_id) =>
            {
                return Err(BusinessError::invalid(
                    "attachment belongs to another session or does not exist",
                ));
            }
            InputPart::FileReference { reference }
                if reference.project_id != session.record.project_id() =>
            {
                return Err(BusinessError::invalid(
                    "file reference belongs to another project",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn editable(
    session: &mut SessionState,
    id: QueuedInputId,
) -> Result<&mut QueuedInput, BusinessError> {
    session
        .queue
        .entries
        .iter_mut()
        .find(|q| {
            q.id == id
                && matches!(
                    q.state,
                    QueuedInputState::Waiting | QueuedInputState::Editing
                )
        })
        .ok_or_else(|| BusinessError::busy("queue entry has been dispatched, cancelled or removed"))
}

pub(super) fn fresh_prompt(session: &SessionState) -> Result<(), BusinessError> {
    validate::ready(session)?;
    validate::pending_submission(session, None)?;
    if session.prompt_in_flight.is_some()
        || session.record.current_run_id.is_some()
        || session.waiting()
    {
        return Err(BusinessError::busy(
            "old prompt/run/requests must end and queue must be resumed before another prompt",
        ));
    }
    Ok(())
}

fn dispatch(
    session: &mut SessionState,
    submission_id: SubmissionId,
    attempt_id: AttemptId,
    envelope: &CommandEnvelope,
    method: TransportMethod,
    effects: &mut Vec<RuntimeEffect>,
) -> Result<(), BusinessError> {
    if session.attempts.contains_key(&attempt_id) {
        return Err(BusinessError::stale());
    }
    let request_id = envelope
        .scope
        .request_id
        .ok_or_else(|| BusinessError::invalid("transport request identity required"))?;
    if session
        .attempts
        .values()
        .any(|a| a.request_id == request_id)
    {
        return Err(BusinessError::stale());
    }
    let submission = session.submissions.get_mut(&submission_id).unwrap();
    let run_id = match submission.intent {
        SubmissionIntent::StartRun { run_id } | SubmissionIntent::SteerRun { run_id } => run_id,
    };
    let context = RunContext::new(session.record.id(), run_id);
    let epoch = session.record.connection.epoch;
    submission.state = SubmissionState::AwaitingAcknowledgement {
        epoch,
        sent_at: envelope.issued_at,
    };
    if let Some(id) = submission.queued_input_id {
        session
            .queue
            .entries
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or_else(|| BusinessError::invalid("queue entry not found"))?
            .state = QueuedInputState::Dispatching { submission_id };
    }
    if matches!(method, TransportMethod::Prompt) {
        let mut run = Run::new(context, epoch, envelope.issued_at);
        run.dispatched_configuration = session.record.configuration.effective.clone();
        session.runs.insert(run_id, run);
        session.record.current_run_id = Some(run_id);
        session.prompt_in_flight = Some(attempt_id);
    }
    let attempt = TransportAttempt {
        id: attempt_id,
        submission_id,
        request_id,
        context,
        epoch,
        method,
        state: AttemptState::InFlight,
        command_id: envelope.id,
    };
    session.attempts.insert(attempt_id, attempt.clone());
    effect(
        effects,
        EffectCause::Command { id: envelope.id },
        envelope.scope,
        RuntimeOperation::Submit {
            attempt,
            submission: submission.clone(),
        },
    );
    Ok(())
}
