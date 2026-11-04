use std::{fmt::Debug, path::PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use workbench_core::*;

fn round_trip<T>(record: T) -> T
where
    T: Clone + Debug + PartialEq + Serialize + DeserializeOwned,
{
    let versioned = Versioned::new(record);
    let encoded = serde_json::to_string_pretty(&versioned).unwrap();
    let decoded: Versioned<T> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, versioned);
    decoded.record
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ConversationExample {
    project: Project,
    agent: AgentDefinition,
    sessions: Vec<SessionRecord>,
    run: Run,
    attachment: Attachment,
    messages: Vec<Message>,
    tool: ToolCall,
    plan: Plan,
    change: FileChange,
    requests: SessionRequests,
    draft: ComposerDraft,
    queue: InputQueue,
    submission: InputSubmission,
}

/// Chinese paths, PNG metadata, concurrent interactions and immutable input snapshots.
/// All paths/Agent IDs are fictional; no actual files, Agent or database are needed.
#[test]
fn conversation_preserves_ownership_and_structured_input() {
    let at = Timestamp::new(1_700_000_000_000);
    let epoch = ConnectionEpoch::new(3);
    let project = Project::new(
        ProjectId::new(),
        "你好项目",
        PathBuf::from(r"C:\示例项目\你好"),
        at,
    );
    let mut agent = AgentDefinition::new(
        AgentId::new(),
        "示例 Agent",
        AgentSource::LocalConfiguration,
    );
    let mut distribution = AgentDistribution::new("example-win", "1.0", "example distribution");
    distribution.platforms.push("windows-x86_64".into());
    agent.distributions.push(distribution);
    agent.discovery = AgentDiscovery::Installed {
        executable: PathBuf::from(r"C:\示例工具\agent.exe"),
        version: Some("1.0".into()),
        distribution_id: Some("example-win".into()),
    };
    let mut session = SessionRecord::new(SessionId::new(), project.id, agent.id, "相同标题", at);
    session.external = Some(ExternalSessionBinding::new(
        ExternalSessionId::new("agent/session-甲"),
        at,
    ));
    session.connection.epoch = epoch;
    session.connection.phase = ConnectionPhase::Ready;
    let other_session = SessionRecord::new(SessionId::new(), project.id, agent.id, "相同标题", at);
    let context = RunContext::new(session.id(), RunId::new());
    let mut run = Run::new(context, epoch, at);
    run.external_id = Some(ExternalRunId::new("agent/run-甲"));
    run.started_at = Some(at);
    run.state = ExecutionState::Running;
    session.current_run_id = Some(context.run_id);

    let mut attachment = Attachment::new(
        AttachmentId::new(),
        session.id(),
        at,
        "截图.png",
        "image/png",
        AttachmentKind::Image {
            width: Some(64),
            height: Some(48),
        },
        AttachmentSource::Clipboard,
    );
    attachment.byte_length = Some(1024);
    attachment.storage = AttachmentStorage::Managed {
        blob_id: AttachmentBlobId::new(),
    };
    let mut reference = FileReference::new(
        FileReferenceId::new(),
        project.id,
        PathBuf::from(r"源码\你好.rs"),
    );
    reference.lines = Some(LineRange::new(2, 8).unwrap());
    let mut draft = ComposerDraft::new(session.id(), at);
    draft.content = vec![
        InputPart::Text {
            text: "请看这张图片和文件\n保留中文路径。".into(),
        },
        InputPart::Attachment {
            attachment_id: attachment.id,
        },
        InputPart::FileReference {
            reference: reference.clone(),
        },
        InputPart::AgentCommand {
            command_id: AgentCommandId::new("agent/review"),
            arguments: "只读".into(),
        },
    ];
    let queued = QueuedInput::from_draft(QueuedInputId::new(), &draft, at);
    let mut submission = InputSubmission::from_draft(
        SubmissionId::new(),
        &draft,
        SubmissionIntent::SteerRun {
            run_id: context.run_id,
        },
        at,
    );
    submission.state = SubmissionState::OutcomeUnknown {
        epoch,
        observed_at: at,
        reason: "接收回执丢失，先核对".into(),
    };
    draft.revision += 1;
    draft.content = vec![InputPart::Text {
        text: "后来的草稿".into(),
    }];
    let mut queue = InputQueue::new(session.id());
    queue.entries.push(queued);
    queue.dispatch = QueueDispatchState::Paused {
        reason: QueuePauseReason::SubmissionOutcomeUnknown,
        paused_at: at,
    };

    let mut tool = ToolCall::new(ToolCallId::new(), context, epoch, "读取文件", at);
    tool.external_id = Some(ExternalToolCallId::new("agent/tool-1"));
    tool.state = ToolState::Running;
    tool.arguments = Some(json!({"path": r"源码\你好.rs", "lines": [2, 8]}));
    let mut plan = Plan::new(PlanId::new(), context, epoch, at);
    let mut step = PlanStep::new(PlanStepId::new(), "检查中文文件");
    step.state = PlanStepState::InProgress;
    plan.steps.push(step);
    let mut change = FileChange::new(
        FileChangeId::new(),
        context,
        epoch,
        at,
        reference.path.clone(),
        FileChangeKind::Modified,
        FileChangeSource::AgentReport {
            tool_call_id: Some(tool.id),
        },
    );
    change.diff = FileDiff::Unified {
        text: "--- a/源码/你好.rs\n+++ b/源码/你好.rs\n@@ -1 +1 @@\n-旧\n+新\n".into(),
    };
    change.additions = Some(1);
    change.deletions = Some(1);

    let option = PermissionOption::new(
        CapabilityOption::new(AgentOptionId::new("allow-once-from-agent"), "本次允许"),
        PermissionDecisionKind::AllowOnce,
    );
    let mut requests = SessionRequests::new(session.id());
    let mut permission = PermissionRequest::new(
        RequestContext::new(RequestId::new(), context, epoch, at),
        "读取文件",
        vec![option.clone()],
    );
    permission.context.tool_call_id = Some(tool.id);
    permission.context.external_id = Some(ExternalRequestId::new("permission-甲"));
    permission.scope.resources.push(PermissionResource::File {
        path: reference.path.clone(),
    });
    requests
        .permissions
        .insert(permission.context.id, permission);
    let mut second = PermissionRequest::new(
        RequestContext::new(RequestId::new(), context, epoch, at),
        "运行命令",
        vec![option.clone()],
    );
    second.status = RequestStatus::Submitting {
        attempt_id: ResponseAttemptId::new(),
        submitted_at: at,
        response: PermissionResponse::Selected {
            option_id: option.option.id,
        },
    };
    requests.permissions.insert(second.context.id, second);
    let field = QuestionField::new(
        AgentFieldId::new("target"),
        "先检查哪项？",
        QuestionFieldKind::SingleChoice {
            options: vec![CapabilityOption::new(AgentOptionId::new("code"), "代码")],
            allows_custom: false,
        },
    );
    let question = QuestionRequest::new(
        RequestContext::new(RequestId::new(), context, epoch, at),
        vec![field],
    );
    requests.questions.insert(question.context.id, question);

    let mut message = Message::new(MessageId::new(), context, MessageRole::User, at);
    message.submission_id = Some(submission.id);
    message.state = MessageState::Complete;
    message.blocks = vec![
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::Text {
                text: "请看图片".into(),
                format: TextFormat::Plain,
            },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::Image {
                attachment_id: attachment.id,
                alt: Some("截图".into()),
            },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::FileReference { reference },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::AgentCommand {
                command_id: AgentCommandId::new("agent/review"),
                arguments: "只读".into(),
            },
        ),
    ];
    let mut reply = Message::new(MessageId::new(), context, MessageRole::Agent, at);
    reply.blocks = vec![
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::Text {
                text: "## 检查中\n```rust\nfn main() {}\n".into(),
                format: TextFormat::Markdown,
            },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::ToolCall {
                tool_call_id: tool.id,
            },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::Plan { plan_id: plan.id },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::FileChange {
                file_change_id: change.id,
            },
        ),
        ContentBlock::new(
            ContentBlockId::new(),
            ContentKind::Unsupported {
                type_name: "agent/unknown".into(),
                payload: json!({"原文": [1, true, null]}),
            },
        ),
    ];
    let restored = round_trip(ConversationExample {
        project,
        agent,
        sessions: vec![session, other_session],
        run,
        attachment,
        messages: vec![message, reply],
        tool,
        plan,
        change,
        requests,
        draft,
        queue,
        submission,
    });
    assert_ne!(restored.sessions[0].id(), restored.sessions[1].id());
    assert_eq!(
        restored.sessions[0].project_id(),
        restored.sessions[1].project_id()
    );
    assert_eq!(
        restored.project.directory,
        PathBuf::from(r"C:\示例项目\你好")
    );
    assert_eq!(restored.requests.waiting_ids().count(), 3);
    assert!(
        restored
            .requests
            .questions
            .values()
            .next()
            .unwrap()
            .context
            .external_id
            .is_none()
    );
    assert_eq!(restored.tool.state, ToolState::Running);
    assert_eq!(
        restored.queue.entries[0].content,
        restored.submission.content
    );
    assert_ne!(restored.draft.content, restored.submission.content);
    assert_eq!(
        restored.run.context.session_id,
        restored.attachment.session_id
    );
    assert_eq!(restored.sessions[1].current_run_id, None);
    assert!(matches!(
        restored.submission.state,
        SubmissionState::OutcomeUnknown { .. }
    ));
}

/// Requested, in-flight and effective values differ; readable history is not recovered work.
#[test]
fn configuration_and_recovery_preserve_distinct_facts() {
    let at = Timestamp::new(1_700_000_000_000);
    let epoch = ConnectionEpoch::new(9);
    let mut session = SessionRecord::new(
        SessionId::new(),
        ProjectId::new(),
        AgentId::new(),
        "恢复示例",
        at,
    );
    assert!(session.configuration.effective.is_none());
    assert_eq!(
        session.configuration.requested.permission,
        PermissionSelection::DefaultFullAccess
    );
    let mut requested = RequestedConfiguration::default();
    requested.model = Some(AgentOptionId::new("model-new"));
    requested.permission = PermissionSelection::AgentOption {
        id: AgentOptionId::new("restricted-by-agent"),
    };
    session.configuration.requested = requested.clone();
    let mut pending = PendingConfiguration::new(RequestId::new(), epoch, at, requested);
    pending.timing = ConfigurationApplyTiming::NextRun;
    let previous = PendingConfiguration::new(
        RequestId::new(),
        epoch,
        at,
        RequestedConfiguration::default(),
    );
    let rejected = ConfigurationRejection::new(previous, at, "Agent 拒绝修改权限");
    session.configuration.last_rejection = Some(rejected);
    session.configuration.pending = Some(pending);
    // A later user edit must not rewrite the already-sent pending snapshot.
    session.configuration.requested.model = Some(AgentOptionId::new("model-latest"));
    let mut effective = EffectiveConfiguration::new(epoch, 2, at);
    effective.model = Some(AgentOptionId::new("model-old"));
    effective.reasoning_effort = Some(AgentOptionId::new("effort-old-model"));
    effective.permission = Some(AgentOptionId::new("restricted-by-agent"));
    session.configuration.effective = Some(effective);
    let mut capabilities = AgentCapabilities::new(epoch, 2);
    let mut model = ModelCapability::new(CapabilityOption::new(
        AgentOptionId::new("model-old"),
        "原模型",
    ));
    model.reasoning_efforts.push(CapabilityOption::new(
        AgentOptionId::new("effort-old-model"),
        "原档位",
    ));
    capabilities.models.push(model);
    capabilities
        .permission_modes
        .push(PermissionCapability::new(
            CapabilityOption::new(AgentOptionId::new("restricted-by-agent"), "受限"),
            AccessMode::Restricted,
        ));
    capabilities.session_resume = CapabilitySupport::Supported;
    capabilities.active_run_recovery = CapabilitySupport::Unsupported {
        reason: Some("只能恢复会话".into()),
    };
    session.capabilities = Some(capabilities);
    session.connection.epoch = epoch;
    session.connection.phase = ConnectionPhase::Ready;
    session.external = Some(ExternalSessionBinding::new(
        ExternalSessionId::new("registered-only"),
        at,
    ));
    session.history = HistoryAvailability::Readable;
    session.resume = SessionResumeState::Resumed { epoch };
    let mut run = Run::new(
        RunContext::new(session.id(), RunId::new()),
        ConnectionEpoch::new(8),
        at,
    );
    run.state = ExecutionState::Interrupted {
        observed_at: at,
        reason: "上次退出时未收到结束".into(),
    };
    run.recovery = RunRecoveryState::Unavailable {
        reason: "Agent 不支持恢复活跃任务".into(),
    };
    let mut queue = InputQueue::new(session.id());
    queue.dispatch = QueueDispatchState::Paused {
        reason: QueuePauseReason::ApplicationRestart,
        paused_at: at,
    };
    let (restored, run, _) = round_trip((session, run, queue));
    let configuration = restored.configuration;
    assert_ne!(
        configuration.requested.model,
        configuration.pending.as_ref().unwrap().requested.model
    );
    assert_ne!(
        configuration.requested.model,
        configuration.effective.as_ref().unwrap().model
    );
    assert!(matches!(
        restored.resume,
        SessionResumeState::Resumed { .. }
    ));
    assert!(!run.state.is_terminal());
    let states = round_trip(vec![
        ExecutionState::Cancelling {
            request_id: RequestId::new(),
            requested_at: at,
        },
        ExecutionState::Cancelled { ended_at: at },
        ExecutionState::Failed {
            ended_at: at,
            reason: "执行失败".into(),
        },
        ExecutionState::OutcomeUnknown {
            observed_at: at,
            reason: "未收到结果".into(),
        },
    ]);
    assert!(!states[0].is_terminal());
    assert!(states[1].is_terminal());
    assert!(states[2].is_terminal());
    assert!(!states[3].is_terminal());
    let request: RequestStatus<QuestionResponse> = round_trip(RequestStatus::Invalidated {
        invalidated_at: at,
        reason: RequestInvalidationReason::Disconnected,
    });
    assert!(!request.is_actionable());
    assert!(!request.is_waiting());
    let submitted: RequestStatus<QuestionResponse> = round_trip(RequestStatus::ResponseSubmitted {
        submitted_at: at,
        response: QuestionResponse::Cancelled,
    });
    assert!(!submitted.is_actionable());
    assert!(!submitted.is_waiting());
}

#[test]
fn unsupported_schema_and_invalid_line_ranges_are_rejected() {
    let record = json!({"model_version": 2, "record": {"anything": "future schema"}});
    assert!(serde_json::from_value::<Versioned<serde_json::Value>>(record).is_err());
    assert!(serde_json::from_value::<Versioned<serde_json::Value>>(json!({"record": {}})).is_err());
    assert!(serde_json::from_value::<LineRange>(json!({"start": 0, "end": 2})).is_err());
    assert!(serde_json::from_value::<LineRange>(json!({"start": 4, "end": 2})).is_err());
    assert_eq!(ConnectionEpoch::new(u64::MAX).next(), None);
}
