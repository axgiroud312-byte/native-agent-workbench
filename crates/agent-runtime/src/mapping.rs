use crate::{Facts, error, now};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use workbench_core::*;

pub(crate) struct Mapper {
    pub facts: Facts,
    pub external: Option<String>,
    pub active: Option<TransportAttempt>,
    context: Option<RunContext>,
    pub capabilities: AgentCapabilities,
    pub effective: EffectiveConfiguration,
    pub options: Vec<Value>,
    pub replay: bool,
    submission: Option<InputSubmission>,
    accepted: bool,
    plan_id: PlanId,
    plan_steps: BTreeMap<String, PlanStepId>,
    plan_published: bool,
    current_agent_message: Option<String>,
    changes: BTreeMap<String, FileChangeId>,
    published_changes: BTreeSet<FileChangeId>,
    messages: BTreeMap<String, (MessageId, ContentBlockId)>,
    pub tools: BTreeMap<String, ToolCall>,
}
impl Mapper {
    pub fn new(facts: Facts) -> Self {
        let epoch = facts.base.epoch.unwrap_or(ConnectionEpoch::INITIAL);
        Self {
            facts,
            external: None,
            active: None,
            context: None,
            capabilities: AgentCapabilities::new(epoch, 0),
            effective: EffectiveConfiguration::new(epoch, 0, now()),
            options: Vec::new(),
            replay: false,
            submission: None,
            accepted: false,
            plan_id: PlanId::new(),
            plan_steps: BTreeMap::new(),
            plan_published: false,
            current_agent_message: None,
            changes: BTreeMap::new(),
            published_changes: BTreeSet::new(),
            messages: BTreeMap::new(),
            tools: BTreeMap::new(),
        }
    }
    pub fn session_scope(&self) -> Scope {
        self.facts.base
    }
    pub fn run_scope(&self) -> Option<Scope> {
        self.context
            .map(|context| self.facts.base.with_run(context.run_id))
    }
    pub fn emit(&self, scope: Scope, event: Event) -> Result<(), BusinessError> {
        self.facts
            .emit(scope, event, self.active.as_ref().map(|a| a.command_id))
    }
    pub fn negotiated(&mut self, init: &Value) {
        let caps = &init["agentCapabilities"];
        self.capabilities.attachments.images =
            support(caps["promptCapabilities"]["image"].as_bool());
        self.capabilities.attachments.files = CapabilitySupport::Supported;
        self.capabilities.session_resume = support(
            caps["sessionCapabilities"]
                .get("resume")
                .map(|v| !v.is_null() && v != false),
        );
        self.capabilities.history_load = support(caps["loadSession"].as_bool());
        self.capabilities.active_run_recovery = CapabilitySupport::Unsupported {
            reason: Some("v1 session resume is not active-run recovery".into()),
        };
        let known = init["agentInfo"]["version"].as_str() == Some("2.5.0");
        self.capabilities.steering = support(init["_meta"]["steering"]["supported"].as_bool());
        if known && self.capabilities.steering == CapabilitySupport::Supported {
            self.capabilities.steering_prompt_required = CapabilitySupport::Supported;
            self.capabilities.steering_source = Some(ProtocolFeatureSource::Extension {
                method: "_session/steering".into(),
                adapter: "codex-acp".into(),
                version: "2.5.0".into(),
            });
        }
        self.capabilities.configuration_during_run = if known {
            CapabilitySupport::Supported
        } else {
            CapabilitySupport::Unknown
        };
        self.capabilities.client_questions.text = CapabilitySupport::Supported;
        self.capabilities.client_questions.single_choice = CapabilitySupport::Supported;
        self.capabilities.client_questions.multiple_choice = CapabilitySupport::Supported;
        self.capabilities.client_questions.forms = CapabilitySupport::Supported;
        if known {
            self.capabilities.questions = self.capabilities.client_questions.clone();
        }
    }
    pub fn configuration(&mut self, options: &Value) -> Result<u64, BusinessError> {
        let options = options.as_array().ok_or_else(|| {
            error(
                ErrorKind::Protocol,
                "Agent did not report a complete config option list",
                RetryAdvice::AfterReconnect,
            )
        })?;
        if self.options == *options && self.capabilities.revision > 0 {
            return Ok(self.capabilities.revision);
        }
        self.options = options.clone();
        self.capabilities.revision =
            self.capabilities.revision.checked_add(1).ok_or_else(|| {
                error(
                    ErrorKind::Exhausted,
                    "capability revision exhausted",
                    RetryAdvice::Never,
                )
            })?;
        let revision = self.capabilities.revision;
        let mut effective =
            EffectiveConfiguration::new(self.facts.base.epoch.unwrap(), revision, now());
        let mut models = Vec::new();
        let mut efforts = Vec::new();
        let mut permissions = Vec::new();
        for option in options {
            let id = option["id"].as_str().unwrap_or("");
            let values = flatten_options(&option["options"]);
            let current = option["currentValue"]
                .as_str()
                .map(|s| AgentOptionId::new(s));
            match id {
                "model" => {
                    effective.model = current;
                    models = values.into_iter().map(ModelCapability::new).collect();
                }
                "reasoning_effort" => {
                    effective.reasoning_effort = current;
                    efforts = values;
                }
                "mode" => {
                    effective.permission = current;
                    permissions = values
                        .into_iter()
                        .map(|option| {
                            let mode = match option.id.as_str() {
                                "agent-full-access" => AccessMode::FullAccess,
                                "read-only" | "workspace-write" | "agent" => AccessMode::Restricted,
                                _ => AccessMode::AgentDefined,
                            };
                            PermissionCapability::new(option, mode)
                        })
                        .collect();
                }
                _ => {}
            }
        }
        for model in &mut models {
            if effective.model.as_ref() == Some(&model.option.id) {
                model.reasoning_efforts = efforts.clone();
            }
        }
        self.capabilities.models = models;
        self.capabilities.permission_modes = permissions;
        self.effective = effective;
        self.report()?;
        Ok(revision)
    }
    pub fn report(&self) -> Result<(), BusinessError> {
        self.facts.emit(
            self.facts.base,
            Event::ConfigurationReported {
                effective: self.effective.clone(),
                capabilities: self.capabilities.clone(),
            },
            None,
        )
    }
    pub fn begin(&mut self, attempt: TransportAttempt, submission: InputSubmission) {
        self.context = Some(attempt.context);
        self.active = Some(attempt);
        self.submission = Some(submission);
        self.accepted = false;
        self.messages.clear();
        self.tools.clear();
        self.plan_id = PlanId::new();
        self.plan_steps.clear();
        self.plan_published = false;
        self.current_agent_message = None;
        self.changes.clear();
        self.published_changes.clear();
    }
    pub fn finish_attempt(&mut self, id: AttemptId) {
        if self.active.as_ref().is_some_and(|attempt| attempt.id == id) {
            self.active = None;
            self.context = None;
        }
    }
    pub fn start_adapter_turn(
        &mut self,
        attempt: TransportAttempt,
        submission: InputSubmission,
        run_id: RunId,
    ) -> Result<(), BusinessError> {
        self.begin(attempt, submission.clone());
        self.active = None;
        self.context = Some(RunContext::new(self.facts.base.session_id.unwrap(), run_id));
        self.accepted = true;
        self.steered_user_message(&submission, run_id)
    }
    pub fn steered_user_message(
        &self,
        submission: &InputSubmission,
        run_id: RunId,
    ) -> Result<(), BusinessError> {
        let scope = self.facts.base.with_run(run_id);
        let mut message = Message::new(
            MessageId::new(),
            RunContext::new(scope.session_id.unwrap(), run_id),
            MessageRole::User,
            now(),
        );
        message.submission_id = Some(submission.id);
        message.state = MessageState::Complete;
        message.blocks = submission
            .content
            .iter()
            .map(|part| ContentBlock::new(ContentBlockId::new(), input_block(part)))
            .collect();
        self.emit(scope, Event::MessageStarted { message })
    }
    pub fn accepted(&mut self) -> Result<(), BusinessError> {
        if self.accepted {
            return Ok(());
        }
        let Some(attempt) = self.active.clone() else {
            return Ok(());
        };
        self.facts.emit(
            self.facts
                .base
                .with_run(attempt.context.run_id)
                .with_request(attempt.request_id),
            Event::Attempt {
                attempt_id: attempt.id,
                outcome: AttemptOutcome::PromptAccepted,
            },
            Some(attempt.command_id),
        )?;
        if let Some(submission) = &self.submission {
            let mut message =
                Message::new(MessageId::new(), attempt.context, MessageRole::User, now());
            message.submission_id = Some(submission.id);
            message.state = MessageState::Complete;
            message.blocks = submission
                .content
                .iter()
                .map(|part| ContentBlock::new(ContentBlockId::new(), input_block(part)))
                .collect();
            self.emit(
                self.facts.base.with_run(attempt.context.run_id),
                Event::MessageStarted { message },
            )?;
        }
        self.accepted = true;
        Ok(())
    }
    pub fn complete_messages(&mut self) -> Result<(), BusinessError> {
        if let Some(scope) = self.run_scope() {
            for (message_id, _) in self.messages.values() {
                self.emit(
                    scope,
                    Event::MessageCompleted {
                        message_id: *message_id,
                    },
                )?;
            }
        }
        self.messages.clear();
        Ok(())
    }
    pub fn notification(&mut self, params: Value) -> Result<(), BusinessError> {
        if self
            .external
            .as_deref()
            .is_some_and(|id| params["sessionId"].as_str() != Some(id))
        {
            return Err(error(
                ErrorKind::StaleIdentity,
                "Agent notification belongs to another session",
                RetryAdvice::Never,
            ));
        }
        let update = &params["update"];
        let kind = update["sessionUpdate"].as_str().unwrap_or("unknown");
        if self.replay && !["config_option_update", "available_commands_update"].contains(&kind) {
            let mut envelope = self.facts.envelope(
                self.facts.base,
                Event::Unsupported {
                    type_name: kind.into(),
                    diagnostic: update.clone(),
                },
                None,
            );
            envelope.source.origin = EventOrigin::AgentReplay {
                adapter: "codex-acp".into(),
                version: "2.5.0".into(),
            };
            return self.facts.sender.try_send(envelope).map_err(|_| {
                error(
                    ErrorKind::Exhausted,
                    "history replay queue exhausted",
                    RetryAdvice::ReconcileFirst,
                )
            });
        }
        match kind {
            "config_option_update" => {
                self.configuration(&update["configOptions"])?;
            }
            "available_commands_update" => {
                self.capabilities.commands = update["availableCommands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| {
                        let name = item["name"].as_str()?;
                        let mut c = AgentCommand::new(AgentCommandId::new(name), name);
                        c.description = item["description"].as_str().map(str::to_owned);
                        Some(c)
                    })
                    .collect();
                if self.capabilities.revision > 0 {
                    self.capabilities.revision += 1;
                    self.effective.capability_revision = self.capabilities.revision;
                    self.effective.confirmed_at = now();
                    self.report()?;
                }
            }
            "agent_message_chunk" | "agent_thought_chunk" | "user_message_chunk" => {
                let Some(scope) = self.run_scope() else {
                    return self.unsupported(kind, update);
                };
                self.accepted()?;
                // This adapter does not emit a live user echo; if one appears, retain it as
                // a diagnostic rather than duplicating the host's accepted user message.
                if kind == "user_message_chunk" {
                    return self.unsupported(kind, update);
                }
                let remote = update["messageId"].as_str().unwrap_or("");
                let key = format!("{kind}:{remote}");
                if kind == "agent_message_chunk" {
                    self.current_agent_message = Some(key.clone());
                }
                let (message_id, block_id) = if let Some(ids) = self.messages.get(&key) {
                    *ids
                } else {
                    let ids = (MessageId::new(), ContentBlockId::new());
                    let mut message = Message::new(
                        ids.0,
                        RunContext::new(scope.session_id.unwrap(), scope.run_id.unwrap()),
                        if kind == "user_message_chunk" {
                            MessageRole::User
                        } else {
                            MessageRole::Agent
                        },
                        now(),
                    );
                    if !remote.is_empty() {
                        message.external_id = Some(ExternalMessageId::new(remote));
                    }
                    self.emit(scope, Event::MessageStarted { message })?;
                    self.emit(
                        scope,
                        Event::ContentBlock {
                            message_id: ids.0,
                            block: ContentBlock::new(
                                ids.1,
                                ContentKind::Text {
                                    text: String::new(),
                                    format: TextFormat::Markdown,
                                },
                            ),
                        },
                    )?;
                    self.messages.insert(key, ids);
                    ids
                };
                if let Some(text) = update["content"]["text"].as_str() {
                    self.emit(
                        scope,
                        Event::TextDelta {
                            message_id,
                            block_id,
                            delta: text.into(),
                        },
                    )?;
                } else {
                    self.emit(
                        scope,
                        Event::ContentBlock {
                            message_id,
                            block: ContentBlock::new(
                                ContentBlockId::new(),
                                ContentKind::Unsupported {
                                    type_name: update["content"]["type"]
                                        .as_str()
                                        .unwrap_or("unknown")
                                        .into(),
                                    payload: update["content"].clone(),
                                },
                            ),
                        },
                    )?;
                }
            }
            "tool_call" | "tool_call_update" => self.tool(update)?,
            "plan" => self.plan(update)?,
            _ => self.unsupported(kind, update)?,
        }
        Ok(())
    }
    pub fn unsupported(&self, name: &str, value: &Value) -> Result<(), BusinessError> {
        // Keep diagnostic structure without logging unknown secret-bearing values.
        let keys = value
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        self.facts.emit(
            self.run_scope().unwrap_or(self.facts.base),
            Event::Unsupported {
                type_name: name.into(),
                diagnostic: json!({"keys":keys}),
            },
            None,
        )
    }
    pub fn tool(&mut self, update: &Value) -> Result<(), BusinessError> {
        let Some(scope) = self.run_scope() else {
            return self.unsupported("tool_without_run", update);
        };
        self.accepted()?;
        let remote = update["toolCallId"].as_str().ok_or_else(|| {
            error(
                ErrorKind::Protocol,
                "tool update has no identity",
                RetryAdvice::Never,
            )
        })?;
        let first = !self.tools.contains_key(remote);
        let mut tool = self.tools.get(remote).cloned().unwrap_or_else(|| {
            let mut tool = ToolCall::new(
                ToolCallId::new(),
                RunContext::new(scope.session_id.unwrap(), scope.run_id.unwrap()),
                scope.epoch.unwrap(),
                update["title"].as_str().unwrap_or("Agent tool"),
                now(),
            );
            tool.external_id = Some(ExternalToolCallId::new(remote));
            tool
        });
        if let Some(title) = update["title"].as_str() {
            tool.name = title.into();
        }
        if let Some(value) = update.get("rawInput") {
            tool.arguments = Some(value.clone());
        }
        if let Some(value) = update.get("rawOutput") {
            tool.result = Some(value.clone());
        }
        match update["status"].as_str() {
            Some("pending") => tool.state = ToolState::Pending,
            Some("in_progress") => tool.state = ToolState::Running,
            Some("completed") => tool.state = ToolState::Completed { ended_at: now() },
            Some("failed") => {
                tool.state = ToolState::Failed {
                    ended_at: now(),
                    reason: "Agent reported tool failure".into(),
                }
            }
            _ => {}
        }
        let mut changes = Vec::new();
        if let Some(content) = update["content"].as_array() {
            tool.output.clear();
            for item in content {
                if item["type"] == "content" {
                    let content = if let Some(text) = item["content"]["text"].as_str() {
                        ContentKind::Text {
                            text: text.into(),
                            format: TextFormat::Plain,
                        }
                    } else {
                        ContentKind::Unsupported {
                            type_name: item["content"]["type"].as_str().unwrap_or("unknown").into(),
                            payload: item["content"].clone(),
                        }
                    };
                    tool.output
                        .push(ContentBlock::new(ContentBlockId::new(), content));
                }
                if item["type"] == "diff" {
                    let Some(path) = item["path"].as_str() else {
                        continue;
                    };
                    let old = item["oldText"].as_str();
                    let new = item["newText"].as_str();
                    let kind = if old.is_none() {
                        FileChangeKind::Added
                    } else if new.is_none() {
                        FileChangeKind::Deleted
                    } else {
                        FileChangeKind::Modified
                    };
                    let key = format!("{remote}:{path}");
                    let id = *self.changes.entry(key).or_insert_with(FileChangeId::new);
                    let mut change = FileChange::new(
                        id,
                        tool.context,
                        tool.epoch,
                        now(),
                        path.into(),
                        kind,
                        FileChangeSource::AgentReport {
                            tool_call_id: Some(tool.id),
                        },
                    );
                    if old.is_some() || new.is_some() {
                        change.diff = FileDiff::Unified {
                            text: full_diff(path, old.unwrap_or(""), new.unwrap_or("")),
                        };
                    }
                    changes.push(change);
                }
            }
        }
        let tool_id = tool.id;
        self.emit(scope, Event::Tool { tool: tool.clone() })?;
        self.tools.insert(remote.into(), tool);
        if first {
            self.structure(ContentKind::ToolCall {
                tool_call_id: tool_id,
            })?;
        }
        for change in changes {
            let id = change.id;
            self.emit(scope, Event::FileChange { change })?;
            if self.published_changes.insert(id) {
                self.structure(ContentKind::FileChange { file_change_id: id })?;
            }
        }
        Ok(())
    }
    fn plan(&mut self, update: &Value) -> Result<(), BusinessError> {
        let Some(scope) = self.run_scope() else {
            return self.unsupported("plan_without_run", update);
        };
        self.accepted()?;
        let mut plan = Plan::new(
            self.plan_id,
            RunContext::new(scope.session_id.unwrap(), scope.run_id.unwrap()),
            scope.epoch.unwrap(),
            now(),
        );
        let mut occurrences: BTreeMap<String, usize> = BTreeMap::new();
        for entry in update["entries"].as_array().into_iter().flatten() {
            let content = entry["content"].as_str().unwrap_or("");
            let occurrence = occurrences.entry(content.into()).or_default();
            let key = format!("{content}\0{occurrence}");
            *occurrence += 1;
            let id = *self.plan_steps.entry(key).or_insert_with(PlanStepId::new);
            let mut step = PlanStep::new(id, content);
            step.state = match entry["status"].as_str() {
                Some("completed") => PlanStepState::Completed,
                Some("in_progress") => PlanStepState::InProgress,
                _ => PlanStepState::Pending,
            };
            plan.steps.push(step);
        }
        self.emit(scope, Event::Plan { plan })?;
        if !self.plan_published {
            self.structure(ContentKind::Plan {
                plan_id: self.plan_id,
            })?;
            self.plan_published = true;
        }
        Ok(())
    }
    fn structure(&mut self, content: ContentKind) -> Result<(), BusinessError> {
        let Some(scope) = self.run_scope() else {
            return Ok(());
        };
        let key = self
            .current_agent_message
            .clone()
            .unwrap_or_else(|| "agent_message_chunk:".into());
        let message_id = if let Some((id, _)) = self.messages.get(&key) {
            *id
        } else {
            let id = MessageId::new();
            self.emit(
                scope,
                Event::MessageStarted {
                    message: Message::new(
                        id,
                        RunContext::new(scope.session_id.unwrap(), scope.run_id.unwrap()),
                        MessageRole::Agent,
                        now(),
                    ),
                },
            )?;
            id
        };
        self.emit(
            scope,
            Event::ContentBlock {
                message_id,
                block: ContentBlock::new(ContentBlockId::new(), content),
            },
        )?;
        let block_id = ContentBlockId::new();
        self.emit(
            scope,
            Event::ContentBlock {
                message_id,
                block: ContentBlock::new(
                    block_id,
                    ContentKind::Text {
                        text: String::new(),
                        format: TextFormat::Markdown,
                    },
                ),
            },
        )?;
        self.messages.insert(key.clone(), (message_id, block_id));
        self.current_agent_message = Some(key);
        Ok(())
    }
}
fn full_diff(path: &str, old: &str, new: &str) -> String {
    let old_count = old.lines().count();
    let new_count = new.lines().count();
    let mut diff = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@\n",
        usize::from(old_count > 0),
        old_count,
        usize::from(new_count > 0),
        new_count
    );
    for (prefix, text) in [('-', old), ('+', new)] {
        for line in text.lines() {
            diff.push(prefix);
            diff.push_str(line);
            diff.push('\n');
        }
        if !text.is_empty() && !text.ends_with('\n') {
            diff.push_str("\\ No newline at end of file\n");
        }
    }
    diff
}
fn input_block(part: &InputPart) -> ContentKind {
    match part {
        InputPart::Text { text } => ContentKind::Text {
            text: text.clone(),
            format: TextFormat::Plain,
        },
        InputPart::Attachment { attachment_id } => ContentKind::Attachment {
            attachment_id: *attachment_id,
        },
        InputPart::FileReference { reference } => ContentKind::FileReference {
            reference: reference.clone(),
        },
        InputPart::AgentCommand {
            command_id,
            arguments,
        } => ContentKind::AgentCommand {
            command_id: command_id.clone(),
            arguments: arguments.clone(),
        },
    }
}
fn support(value: Option<bool>) -> CapabilitySupport {
    match value {
        Some(true) => CapabilitySupport::Supported,
        Some(false) => CapabilitySupport::Unsupported { reason: None },
        None => CapabilitySupport::Unknown,
    }
}
fn flatten_options(options: &Value) -> Vec<CapabilityOption> {
    options
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|value| {
            if let Some(group) = value.get("options") {
                flatten_options(group)
            } else {
                value["value"]
                    .as_str()
                    .map(|id| {
                        vec![CapabilityOption::new(
                            AgentOptionId::new(id),
                            value["name"].as_str().unwrap_or(id),
                        )]
                    })
                    .unwrap_or_default()
            }
        })
        .collect()
}
