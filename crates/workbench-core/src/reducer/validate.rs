use crate::state::SessionState;
use crate::*;

pub(super) fn supported(value: &CapabilitySupport, feature: &str) -> Result<(), BusinessError> {
    match value {
        CapabilitySupport::Supported => Ok(()),
        CapabilitySupport::Unknown => Err(BusinessError::new(
            ErrorKind::CapabilityUnknown,
            format!("{feature} has not been negotiated"),
            RetryAdvice::AfterReconnect,
        )),
        CapabilitySupport::Unsupported { .. } => Err(BusinessError::new(
            ErrorKind::Unsupported,
            format!("{feature} is unsupported"),
            RetryAdvice::AfterCorrection,
        )),
    }
}

pub(super) fn ready(session: &SessionState) -> Result<&AgentCapabilities, BusinessError> {
    if session.closing || !matches!(session.record.connection.phase, ConnectionPhase::Ready) {
        return Err(BusinessError::new(
            ErrorKind::NotReady,
            "session is not connected",
            RetryAdvice::AfterReconnect,
        ));
    }
    let capabilities = session
        .record
        .capabilities
        .as_ref()
        .filter(|c| c.epoch == session.record.connection.epoch)
        .ok_or_else(|| {
            BusinessError::new(
                ErrorKind::CapabilityUnknown,
                "current capabilities are not confirmed",
                RetryAdvice::AfterReconnect,
            )
        })?;
    Ok(capabilities)
}

pub(super) fn configuration(
    c: &AgentCapabilities,
    requested: &RequestedConfiguration,
) -> Result<(), BusinessError> {
    let model = requested
        .model
        .as_ref()
        .map(|id| {
            c.models
                .iter()
                .find(|m| &m.option.id == id)
                .ok_or_else(|| BusinessError::invalid("model option is no longer available"))
        })
        .transpose()?;
    if let Some(effort) = &requested.reasoning_effort {
        if !model.is_some_and(|m| m.reasoning_efforts.iter().any(|e| &e.id == effort)) {
            return Err(BusinessError::invalid(
                "effort requires a confirmed compatible model option",
            ));
        }
    }
    let permission = match &requested.permission {
        PermissionSelection::DefaultFullAccess => c
            .permission_modes
            .iter()
            .any(|p| p.mode == AccessMode::FullAccess),
        PermissionSelection::AgentOption { id } => {
            c.permission_modes.iter().any(|p| &p.option.id == id)
        }
    };
    if !permission {
        return Err(BusinessError::new(
            ErrorKind::Unsupported,
            "requested permission mode is not available",
            RetryAdvice::AfterCorrection,
        ));
    }
    Ok(())
}

/// Shared confirmed configuration gate, independent of whether local input is saved yet.
pub(super) fn input_configuration<'a>(
    session: &'a SessionState,
    requested: &RequestedConfiguration,
) -> Result<&'a AgentCapabilities, BusinessError> {
    let c = ready(session)?;
    configuration(c, requested)?;
    if session.record.configuration.pending.is_some() || session.next_configuration.is_some() {
        return Err(BusinessError::busy(
            "configuration must settle before dispatch",
        ));
    }
    let effective = session
        .record
        .configuration
        .effective
        .as_ref()
        .filter(|e| e.epoch == c.epoch && e.capability_revision == c.revision)
        .ok_or_else(|| {
            BusinessError::busy(
                "Agent configuration has not been confirmed for current capabilities",
            )
        })?;
    let permission_matches = match &requested.permission {
        PermissionSelection::AgentOption { id } => effective.permission.as_ref() == Some(id),
        PermissionSelection::DefaultFullAccess => effective.permission.as_ref().is_some_and(|id| {
            c.permission_modes
                .iter()
                .any(|p| &p.option.id == id && p.mode == AccessMode::FullAccess)
        }),
    };
    if requested
        .model
        .as_ref()
        .is_some_and(|id| effective.model.as_ref() != Some(id))
        || requested
            .reasoning_effort
            .as_ref()
            .is_some_and(|id| effective.reasoning_effort.as_ref() != Some(id))
        || !permission_matches
    {
        return Err(BusinessError::busy(
            "input configuration intent must be confirmed before sending",
        ));
    }
    Ok(c)
}

pub(super) fn input(
    session: &SessionState,
    content: &[InputPart],
    requested: &RequestedConfiguration,
) -> Result<(), BusinessError> {
    let c = input_configuration(session, requested)?;
    if content.is_empty() {
        return Err(BusinessError::invalid("input is empty"));
    }
    let mut attachments = 0u32;
    for part in content {
        match part {
            InputPart::Attachment { attachment_id } => {
                let attachment = session.attachments.get(attachment_id).ok_or_else(|| {
                    BusinessError::invalid("attachment is missing or belongs to another session")
                })?;
                if !matches!(attachment.storage, AttachmentStorage::Managed { .. }) {
                    return Err(BusinessError::invalid("attachment bytes are not ready"));
                }
                match attachment.kind {
                    AttachmentKind::Image { .. } => supported(&c.attachments.images, "images")?,
                    AttachmentKind::File => supported(&c.attachments.files, "files")?,
                }
                if !c.attachments.media_types.is_empty()
                    && !c.attachments.media_types.contains(&attachment.media_type)
                {
                    return Err(BusinessError::invalid(
                        "attachment media type is unsupported",
                    ));
                }
                if let Some(max) = c.attachments.max_bytes_per_attachment {
                    if attachment.byte_length.is_none_or(|bytes| bytes > max) {
                        return Err(BusinessError::invalid(
                            "attachment size is unknown or exceeds the limit",
                        ));
                    }
                }
                attachments = attachments
                    .checked_add(1)
                    .ok_or_else(|| BusinessError::invalid("too many attachments"))?;
            }
            InputPart::FileReference { reference }
                if reference.project_id != session.record.project_id() =>
            {
                return Err(BusinessError::invalid(
                    "file reference belongs to another project",
                ));
            }
            InputPart::AgentCommand { command_id, .. }
                if !c.commands.iter().any(|c| &c.id == command_id) =>
            {
                return Err(BusinessError::invalid("Agent command is unavailable"));
            }
            _ => {}
        }
    }
    if c.attachments
        .max_attachments_per_input
        .is_some_and(|max| attachments > max)
    {
        return Err(BusinessError::invalid("too many attachments"));
    }
    Ok(())
}

/// Shared action eligibility; UI uses its current draft, Submit uses its frozen input.
pub(super) fn steering(
    session: &SessionState,
    run_id: RunId,
    content: &[InputPart],
    requested: &RequestedConfiguration,
) -> Result<(), BusinessError> {
    pending_submission(session, Some(run_id))?;
    input(session, content, requested)?;
    if session.record.current_run_id != Some(run_id)
        || !session
            .current_run()
            .is_some_and(|r| matches!(r.state, ExecutionState::Running | ExecutionState::Starting))
    {
        return Err(BusinessError::stale());
    }
    let caps = ready(session)?;
    supported(&caps.steering, "steering extension")?;
    supported(
        &caps.steering_prompt_required,
        "steering promptRequired idle behavior",
    )?;
    if !matches!(
        caps.steering_source,
        Some(ProtocolFeatureSource::Extension { .. })
    ) {
        return Err(BusinessError::invalid(
            "steering extension provenance is missing",
        ));
    }
    Ok(())
}

pub(super) fn pending_submission(
    session: &SessionState,
    steering_target: Option<RunId>,
) -> Result<(), BusinessError> {
    if session.attempts.values().any(|a| a.state == AttemptState::ConsumedAwaitingEnd) || session.submissions.values().any(|s| {
        crate::transport::unsettled(&s.state) && !(matches!(s.state, SubmissionState::AwaitingAcknowledgement { .. }) &&
            matches!(s.intent, SubmissionIntent::StartRun { run_id } if steering_target == Some(run_id)))
    }) { return Err(BusinessError::busy("another logical submission or reconciled execution is undecided")); }
    Ok(())
}

pub(super) fn reconciliation_eligible(session: &SessionState, attempt: &TransportAttempt) -> bool {
    session
        .submissions
        .get(&attempt.submission_id)
        .is_some_and(|s| match s.state {
            SubmissionState::OutcomeUnknown { .. } => {
                matches!(attempt.state, AttemptState::Unknown { .. })
            }
            SubmissionState::Accepted { run_id, epoch, .. } => {
                attempt.state == AttemptState::ConsumedAwaitingEnd
                    && run_id == attempt.context.run_id
                    && epoch == attempt.epoch
            }
            _ => false,
        })
}

pub(super) fn request_scope(
    session: &SessionState,
    scope: Scope,
    context: &RequestContext,
) -> Result<(), BusinessError> {
    if scope.request_id != Some(context.id)
        || scope.run_id != Some(context.run.run_id)
        || context.run.session_id != session.record.id()
        || scope.epoch != Some(context.epoch)
        || session.record.current_run_id != Some(context.run.run_id)
        || !session
            .current_run()
            .is_some_and(|r| matches!(r.state, ExecutionState::Running | ExecutionState::Starting))
    {
        return Err(BusinessError::new(
            ErrorKind::RequestExpired,
            "request no longer has a live original responder/run",
            RetryAdvice::Never,
        ));
    }
    Ok(())
}

pub(super) fn answers(
    fields: &[QuestionField],
    response: &QuestionResponse,
) -> Result<(), BusinessError> {
    let QuestionResponse::Answered { answers } = response else {
        return Ok(());
    };
    if fields.iter().any(|f| f.is_secret) {
        return Err(BusinessError::new(
            ErrorKind::Unsupported,
            "secret form values require a non-persistent authentication channel",
            RetryAdvice::Never,
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for answer in answers {
        if !seen.insert(&answer.field_id) {
            return Err(BusinessError::invalid("duplicate form field"));
        }
        let field = fields
            .iter()
            .find(|f| f.id == answer.field_id)
            .ok_or_else(|| BusinessError::invalid("unknown form field"))?;
        match (&field.kind, &answer.value) {
            (QuestionFieldKind::Text { .. }, QuestionAnswerValue::Text { text })
                if !field.required || !text.trim().is_empty() => {}
            (
                QuestionFieldKind::SingleChoice {
                    options,
                    allows_custom,
                },
                QuestionAnswerValue::Choices { option_ids, custom },
            )
            | (
                QuestionFieldKind::MultipleChoice {
                    options,
                    allows_custom,
                },
                QuestionAnswerValue::Choices { option_ids, custom },
            ) => {
                let unique: std::collections::BTreeSet<_> = option_ids.iter().collect();
                if unique.len() != option_ids.len()
                    || option_ids
                        .iter()
                        .any(|id| !options.iter().any(|o| &o.id == id))
                    || custom.is_some() && !allows_custom
                {
                    return Err(BusinessError::invalid("invalid form choices"));
                }
                let count = option_ids.len()
                    + usize::from(custom.as_ref().is_some_and(|s| !s.trim().is_empty()));
                if field.required && count == 0
                    || matches!(field.kind, QuestionFieldKind::SingleChoice { .. }) && count > 1
                {
                    return Err(BusinessError::invalid("wrong number of selected choices"));
                }
            }
            (QuestionFieldKind::Unsupported { .. }, _) => {
                return Err(BusinessError::new(
                    ErrorKind::Unsupported,
                    "form field schema is unsupported",
                    RetryAdvice::AfterCorrection,
                ));
            }
            _ => {
                return Err(BusinessError::invalid(
                    "form answer does not match its field type",
                ));
            }
        }
    }
    for field in fields {
        if field.required
            && (!seen.contains(&field.id)
                || matches!(field.kind, QuestionFieldKind::Unsupported { .. }))
        {
            return Err(BusinessError::invalid(
                "required form field cannot be omitted or unsupported",
            ));
        }
    }
    Ok(())
}
