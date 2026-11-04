use crate::*;
use serde::{Deserialize, Serialize};

/// Safe typed UI data. No arbitrary serde_json::Value or SDK handle crosses this boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WorkbenchSnapshot {
    pub sequence: u64,
    pub projects: Vec<Project>,
    pub agents: Vec<AgentDefinition>,
    pub sessions: Vec<SessionSummary>,
    pub errors: Vec<BusinessError>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionSummary {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: String,
    pub connection: ConnectionState,
    pub execution: Option<ExecutionState>,
    pub waiting_requests: usize,
    pub queue: QueueDispatchState,
    pub closing: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentPresentation {
    Text {
        text: String,
        format: TextFormat,
    },
    Image {
        attachment_id: AttachmentId,
        alt: Option<String>,
    },
    Attachment {
        attachment_id: AttachmentId,
    },
    FileReference {
        reference: FileReference,
    },
    AgentCommand {
        command_id: AgentCommandId,
        arguments: String,
    },
    ToolCall {
        tool_call_id: ToolCallId,
    },
    Plan {
        plan_id: PlanId,
    },
    FileChange {
        file_change_id: FileChangeId,
    },
    Unsupported {
        type_name: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BlockSnapshot {
    pub id: ContentBlockId,
    pub content: ContentPresentation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MessageSnapshot {
    pub id: MessageId,
    pub context: RunContext,
    pub role: MessageRole,
    pub state: MessageState,
    pub submission_id: Option<SubmissionId>,
    pub blocks: Vec<BlockSnapshot>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolSnapshot {
    pub id: ToolCallId,
    pub context: RunContext,
    pub name: String,
    pub summary: Option<String>,
    pub state: ToolState,
    pub arguments_text: Option<String>,
    pub result_text: Option<String>,
    pub output: Vec<BlockSnapshot>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldPresentation {
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
    Unsupported {
        type_name: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionFieldSnapshot {
    pub id: AgentFieldId,
    pub prompt: String,
    pub required: bool,
    pub is_secret: bool,
    pub kind: FieldPresentation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuestionSnapshot {
    pub context: RequestContext,
    pub title: Option<String>,
    pub fields: Vec<QuestionFieldSnapshot>,
    pub status: RequestStatus<QuestionResponse>,
    pub is_actionable: bool,
    pub accepts_answers: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PermissionSnapshot {
    pub request: PermissionRequest,
    pub is_actionable: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DiagnosticSummary {
    pub event_id: EventId,
    pub type_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionSnapshot {
    pub record: SessionRecord,
    pub closing: bool,
    pub closing_handoff: Option<ClosingResourceHandoff>,
    pub runs: Vec<Run>,
    pub messages: Vec<MessageSnapshot>,
    pub tools: Vec<ToolSnapshot>,
    pub plans: Vec<Plan>,
    pub changes: Vec<FileChange>,
    pub attachments: Vec<Attachment>,
    pub permissions: Vec<PermissionSnapshot>,
    pub questions: Vec<QuestionSnapshot>,
    pub draft: ComposerDraft,
    pub queue: InputQueue,
    pub submissions: Vec<InputSubmission>,
    pub attempts: Vec<TransportAttempt>,
    pub can_start_prompt: bool,
    pub can_steer: bool,
    pub can_dispatch_queue: bool,
    pub diagnostics: Vec<DiagnosticSummary>,
    pub errors: Vec<BusinessError>,
}

impl WorkbenchState {
    pub fn snapshot(&self) -> WorkbenchSnapshot {
        WorkbenchSnapshot {
            sequence: self.sequence,
            projects: self.projects.values().cloned().collect(),
            agents: self.agents.values().cloned().collect(),
            errors: self.global_errors.clone(),
            sessions: self
                .sessions
                .values()
                .map(|s| SessionSummary {
                    id: s.record.id(),
                    project_id: s.record.project_id(),
                    title: s.record.title.clone(),
                    connection: s.record.connection.clone(),
                    execution: s.current_run().map(|r| r.state.clone()),
                    waiting_requests: s.requests.waiting_ids().count(),
                    queue: s.queue.dispatch.clone(),
                    closing: s.closing,
                })
                .collect(),
        }
    }
    pub fn session_snapshot(&self, id: SessionId) -> Option<SessionSnapshot> {
        let s = self.sessions.get(&id)?;
        let live_request = |c: &RequestContext| {
            !s.closing
                && c.epoch == s.record.connection.epoch
                && s.record.current_run_id == Some(c.run.run_id)
                && s.current_run().is_some_and(|r| {
                    matches!(r.state, ExecutionState::Running | ExecutionState::Starting)
                })
        };
        let base_ready = crate::reducer::is_fresh_prompt(s);
        let unsettled = crate::reducer::has_unsettled_input(s);
        let head = s.queue.entries.iter().find(|q| {
            matches!(
                q.state,
                QueuedInputState::Waiting
                    | QueuedInputState::Editing
                    | QueuedInputState::Dispatching { .. }
            )
        });
        let can_dispatch_queue = base_ready
            && !unsettled
            && matches!(s.queue.dispatch, QueueDispatchState::Ready)
            && head.is_some_and(|q| {
                q.state == QueuedInputState::Waiting
                    && crate::reducer::valid_input(s, &q.content, &q.configuration)
            });
        let can_steer = crate::reducer::is_steering_eligible(s);
        Some(SessionSnapshot {
            record: s.record.clone(),
            closing: s.closing,
            closing_handoff: s.closing_handoff.clone(),
            runs: s.runs.values().cloned().collect(),
            messages: s
                .messages
                .iter()
                .map(|m| MessageSnapshot {
                    id: m.id,
                    context: m.context,
                    role: m.role,
                    state: m.state.clone(),
                    submission_id: m.submission_id,
                    blocks: m.blocks.iter().map(block).collect(),
                })
                .collect(),
            tools: s
                .tools
                .values()
                .map(|t| ToolSnapshot {
                    id: t.id,
                    context: t.context,
                    name: t.name.clone(),
                    summary: t.summary.clone(),
                    state: t.state.clone(),
                    arguments_text: t.arguments.as_ref().map(|v| v.to_string()),
                    result_text: t.result.as_ref().map(|v| v.to_string()),
                    output: t.output.iter().map(block).collect(),
                })
                .collect(),
            plans: s.plans.values().cloned().collect(),
            changes: s.changes.values().cloned().collect(),
            attachments: s.attachments.values().cloned().collect(),
            permissions: s
                .requests
                .permissions
                .values()
                .map(|r| PermissionSnapshot {
                    request: r.clone(),
                    is_actionable: r.status.is_actionable() && live_request(&r.context),
                })
                .collect(),
            questions: s
                .requests
                .questions
                .values()
                .map(|r| QuestionSnapshot {
                    context: r.context.clone(),
                    title: r.title.clone(),
                    fields: r.fields.iter().map(field).collect(),
                    status: r.status.clone(),
                    is_actionable: r.status.is_actionable() && live_request(&r.context),
                    accepts_answers: !r.fields.iter().any(|f| {
                        f.is_secret
                            || f.required && matches!(f.kind, QuestionFieldKind::Unsupported { .. })
                    }),
                })
                .collect(),
            draft: s.draft.clone(),
            queue: s.queue.clone(),
            submissions: s.submissions.values().cloned().collect(),
            attempts: s.attempts.values().cloned().collect(),
            can_start_prompt: base_ready
                && !unsettled
                && head.is_none()
                && crate::reducer::valid_input(s, &s.draft.content, &s.draft.configuration),
            can_steer,
            can_dispatch_queue,
            diagnostics: s
                .diagnostics
                .iter()
                .map(|d| DiagnosticSummary {
                    event_id: d.event_id,
                    type_name: d.type_name.clone(),
                })
                .collect(),
            errors: s.errors.clone(),
        })
    }
}

fn block(b: &ContentBlock) -> BlockSnapshot {
    let content = match &b.content {
        ContentKind::Text { text, format } => ContentPresentation::Text {
            text: text.clone(),
            format: *format,
        },
        ContentKind::Image { attachment_id, alt } => ContentPresentation::Image {
            attachment_id: *attachment_id,
            alt: alt.clone(),
        },
        ContentKind::Attachment { attachment_id } => ContentPresentation::Attachment {
            attachment_id: *attachment_id,
        },
        ContentKind::FileReference { reference } => ContentPresentation::FileReference {
            reference: reference.clone(),
        },
        ContentKind::AgentCommand {
            command_id,
            arguments,
        } => ContentPresentation::AgentCommand {
            command_id: command_id.clone(),
            arguments: arguments.clone(),
        },
        ContentKind::ToolCall { tool_call_id } => ContentPresentation::ToolCall {
            tool_call_id: *tool_call_id,
        },
        ContentKind::Plan { plan_id } => ContentPresentation::Plan { plan_id: *plan_id },
        ContentKind::FileChange { file_change_id } => ContentPresentation::FileChange {
            file_change_id: *file_change_id,
        },
        ContentKind::Unsupported { type_name, .. } => ContentPresentation::Unsupported {
            type_name: type_name.clone(),
        },
    };
    BlockSnapshot { id: b.id, content }
}
fn field(f: &QuestionField) -> QuestionFieldSnapshot {
    let kind = match &f.kind {
        QuestionFieldKind::Text { multiline } => FieldPresentation::Text {
            multiline: *multiline,
        },
        QuestionFieldKind::SingleChoice {
            options,
            allows_custom,
        } => FieldPresentation::SingleChoice {
            options: options.clone(),
            allows_custom: *allows_custom,
        },
        QuestionFieldKind::MultipleChoice {
            options,
            allows_custom,
        } => FieldPresentation::MultipleChoice {
            options: options.clone(),
            allows_custom: *allows_custom,
        },
        QuestionFieldKind::Unsupported { type_name, .. } => FieldPresentation::Unsupported {
            type_name: type_name.clone(),
        },
    };
    QuestionFieldSnapshot {
        id: f.id.clone(),
        prompt: f.prompt.clone(),
        required: f.required,
        is_secret: f.is_secret,
        kind,
    }
}
