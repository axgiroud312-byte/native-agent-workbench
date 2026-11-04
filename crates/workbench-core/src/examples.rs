//! Shared, fictional event/command examples for UI, Storage and Runtime contract checks.
//! IDs and order are fixed; no filesystem, network, random generation or Agent is involved.
use crate::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContractExample {
    pub records: Vec<JournalRecord>,
    pub sessions: [SessionId; 2],
    pub runs: [RunId; 2],
    pub attempts: [AttemptId; 2],
    pub transport_requests: [RequestId; 2],
    pub permissions: [RequestId; 2],
    pub question: RequestId,
    pub queue_id: QueuedInputId,
    pub attachment_id: AttachmentId,
    pub messages: [MessageId; 2],
    pub blocks: [ContentBlockId; 2],
}

pub fn interleaved_example() -> ContractExample {
    let at = Timestamp::new(1000);
    let epoch = ConnectionEpoch::new(1);
    let project = Project::new(
        ProjectId::from_uuid(Uuid::from_u128(1)),
        "中文项目",
        r"C:\示例项目\你好".into(),
        at,
    );
    let agent = AgentDefinition::new(
        AgentId::from_uuid(Uuid::from_u128(2)),
        "示例 Agent",
        AgentSource::LocalConfiguration,
    );
    let sessions = [
        SessionId::from_uuid(Uuid::from_u128(10)),
        SessionId::from_uuid(Uuid::from_u128(11)),
    ];
    let runs = [
        RunId::from_uuid(Uuid::from_u128(20)),
        RunId::from_uuid(Uuid::from_u128(21)),
    ];
    let attempts = [
        AttemptId::from_uuid(Uuid::from_u128(30)),
        AttemptId::from_uuid(Uuid::from_u128(31)),
    ];
    let transport_requests = [
        RequestId::from_uuid(Uuid::from_u128(40)),
        RequestId::from_uuid(Uuid::from_u128(41)),
    ];
    let messages = [
        MessageId::from_uuid(Uuid::from_u128(50)),
        MessageId::from_uuid(Uuid::from_u128(51)),
    ];
    let blocks = [
        ContentBlockId::from_uuid(Uuid::from_u128(60)),
        ContentBlockId::from_uuid(Uuid::from_u128(61)),
    ];
    let permissions = [
        RequestId::from_uuid(Uuid::from_u128(70)),
        RequestId::from_uuid(Uuid::from_u128(71)),
    ];
    let question = RequestId::from_uuid(Uuid::from_u128(72));
    let attachment_id = AttachmentId::from_uuid(Uuid::from_u128(80));
    let queue_id = QueuedInputId::from_uuid(Uuid::from_u128(90));
    let mut builder = Builder {
        records: Vec::new(),
    };
    builder.command(
        Scope::default(),
        Command::PutProject {
            project: project.clone(),
        },
    );
    builder.command(
        Scope::default(),
        Command::PutAgent {
            agent: agent.clone(),
        },
    );
    for ix in 0..2 {
        let session =
            SessionRecord::new(sessions[ix], project.id, agent.id, format!("会话 {ix}"), at);
        builder.command(Scope::default(), Command::CreateSession { session });
        builder.command(
            Scope::session(sessions[ix], ConnectionEpoch::INITIAL),
            Command::Connect { resume: false },
        );
        let scope = Scope::session(sessions[ix], epoch);
        builder.event(
            scope,
            Event::ConnectionChanged {
                phase: ConnectionPhase::Ready,
            },
        );
        builder.event(
            scope,
            Event::SessionRegistered {
                binding: ExternalSessionBinding::new(
                    ExternalSessionId::new(format!("fixture-{ix}")),
                    at,
                ),
                resumed: false,
            },
        );
        let (capabilities, effective) = confirmed_example(epoch, 1);
        builder.event(
            scope,
            Event::ConfigurationReported {
                effective,
                capabilities,
            },
        );
        let mut draft = ComposerDraft::new(sessions[ix], at);
        draft.content = vec![InputPart::Text {
            text: format!("检查文件 {ix}"),
        }];
        let submission = InputSubmission::from_draft(
            SubmissionId::from_uuid(Uuid::from_u128(100 + ix as u128)),
            &draft,
            SubmissionIntent::StartRun { run_id: runs[ix] },
            at,
        );
        let run_scope = scope
            .with_run(runs[ix])
            .with_request(transport_requests[ix]);
        builder.command(
            run_scope,
            Command::Submit {
                submission,
                attempt_id: attempts[ix],
            },
        );
        builder.event(
            run_scope,
            Event::Attempt {
                attempt_id: attempts[ix],
                outcome: AttemptOutcome::PromptAccepted,
            },
        );
    }
    let mut attachment = Attachment::new(
        attachment_id,
        sessions[0],
        at,
        "截图.png",
        "image/png",
        AttachmentKind::Image {
            width: Some(64),
            height: Some(48),
        },
        AttachmentSource::Clipboard,
    );
    attachment.storage = AttachmentStorage::Managed {
        blob_id: AttachmentBlobId::from_uuid(Uuid::from_u128(81)),
    };
    attachment.byte_length = Some(100);
    builder.command(
        Scope::session(sessions[0], epoch),
        Command::PutAttachment { attachment },
    );
    for ix in 0..2 {
        let context = RunContext::new(sessions[ix], runs[ix]);
        let scope = Scope::session(sessions[ix], epoch).with_run(runs[ix]);
        let mut message = Message::new(messages[ix], context, MessageRole::Agent, at);
        message.blocks.push(ContentBlock::new(
            blocks[ix],
            ContentKind::Text {
                text: String::new(),
                format: TextFormat::Markdown,
            },
        ));
        if ix == 0 {
            message.blocks.push(ContentBlock::new(
                ContentBlockId::from_uuid(Uuid::from_u128(62)),
                ContentKind::Image {
                    attachment_id,
                    alt: Some("图片".into()),
                },
            ));
            message.blocks.push(ContentBlock::new(
                ContentBlockId::from_uuid(Uuid::from_u128(63)),
                ContentKind::FileReference {
                    reference: FileReference::new(
                        FileReferenceId::from_uuid(Uuid::from_u128(64)),
                        project.id,
                        r"源码\你好.rs".into(),
                    ),
                },
            ));
        }
        builder.event(scope, Event::MessageStarted { message });
    }
    for (ix, text) in [(0, "甲"), (1, "乙"), (0, "继续"), (1, "继续")] {
        builder.event(
            Scope::session(sessions[ix], epoch).with_run(runs[ix]),
            Event::TextDelta {
                message_id: messages[ix],
                block_id: blocks[ix],
                delta: text.into(),
            },
        );
    }
    let context = RunContext::new(sessions[0], runs[0]);
    let scope = Scope::session(sessions[0], epoch).with_run(runs[0]);
    let mut tool = ToolCall::new(
        ToolCallId::from_uuid(Uuid::from_u128(110)),
        context,
        epoch,
        "读取文件",
        at,
    );
    tool.state = ToolState::Failed {
        ended_at: at,
        reason: "文件读取失败".into(),
    };
    builder.event(scope, Event::Tool { tool });
    for id in permissions {
        let request = PermissionRequest::new(
            RequestContext::new(id, context, epoch, at),
            "运行命令",
            vec![PermissionOption::new(
                CapabilityOption::new(AgentOptionId::new("allow-once"), "本次允许"),
                PermissionDecisionKind::AllowOnce,
            )],
        );
        builder.event(scope.with_request(id), Event::Permission { request });
    }
    let field = QuestionField::new(
        AgentFieldId::new("target"),
        "检查目标？",
        QuestionFieldKind::Text { multiline: false },
    );
    builder.event(
        scope.with_request(question),
        Event::Question {
            request: QuestionRequest::new(
                RequestContext::new(question, context, epoch, at),
                vec![field],
            ),
        },
    );
    let mut draft = ComposerDraft::new(sessions[0], at);
    draft.content = vec![
        InputPart::Text {
            text: "排队继续".into(),
        },
        InputPart::Attachment { attachment_id },
    ];
    builder.command(
        Scope::session(sessions[0], epoch),
        Command::Enqueue {
            input: QueuedInput::from_draft(queue_id, &draft, at),
        },
    );
    ContractExample {
        records: builder.records,
        sessions,
        runs,
        attempts,
        transport_requests,
        permissions,
        question,
        queue_id,
        attachment_id,
        messages,
        blocks,
    }
}

/// Complete fictional report used by all fixtures and available to adapter contract tests.
pub fn confirmed_example(
    epoch: ConnectionEpoch,
    revision: u64,
) -> (AgentCapabilities, EffectiveConfiguration) {
    let mut capabilities = AgentCapabilities::new(epoch, revision);
    let mut model = ModelCapability::new(CapabilityOption::new(
        AgentOptionId::new("fixture-model"),
        "示例模型",
    ));
    model.reasoning_efforts.push(CapabilityOption::new(
        AgentOptionId::new("fixture-effort"),
        "示例档位",
    ));
    capabilities.models.push(model);
    capabilities
        .permission_modes
        .push(PermissionCapability::new(
            CapabilityOption::new(AgentOptionId::new("fixture-full"), "完整访问"),
            AccessMode::FullAccess,
        ));
    capabilities.attachments.images = CapabilitySupport::Supported;
    capabilities.questions.text = CapabilitySupport::Supported;
    capabilities.client_questions.text = CapabilitySupport::Supported;
    capabilities.configuration_during_run = CapabilitySupport::Supported;
    capabilities.steering = CapabilitySupport::Supported;
    capabilities.steering_prompt_required = CapabilitySupport::Supported;
    capabilities.steering_source = Some(ProtocolFeatureSource::Extension {
        method: "_session/steering".into(),
        adapter: "fixture".into(),
        version: "verified-fixture".into(),
    });
    let mut effective =
        EffectiveConfiguration::new(epoch, revision, Timestamp::new(1000 + revision as i64));
    effective.model = Some(AgentOptionId::new("fixture-model"));
    effective.permission = Some(AgentOptionId::new("fixture-full"));
    (capabilities, effective)
}

struct Builder {
    records: Vec<JournalRecord>,
}
impl Builder {
    fn command(&mut self, scope: Scope, command: Command) {
        let sequence = self.records.len() as u64 + 1;
        self.records.push(JournalRecord::new(
            sequence,
            JournalEntry::Command(CommandEnvelope::new(
                CommandId::from_uuid(Uuid::from_u128(1000 + sequence as u128)),
                Timestamp::new(sequence as i64),
                scope,
                command,
            )),
        ));
    }
    fn event(&mut self, scope: Scope, event: Event) {
        let sequence = self.records.len() as u64 + 1;
        self.records.push(JournalRecord::new(
            sequence,
            JournalEntry::Event(EventEnvelope::new(
                EventId::from_uuid(Uuid::from_u128(2000 + sequence as u128)),
                Timestamp::new(sequence as i64),
                scope,
                EventSource::new(
                    EventSourceId::from_uuid(Uuid::from_u128(3000)),
                    EventOrigin::Runtime {
                        adapter: "fixture".into(),
                        version: "1".into(),
                    },
                ),
                event,
            )),
        ));
    }
}
