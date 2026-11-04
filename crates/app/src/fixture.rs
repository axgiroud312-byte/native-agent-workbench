use std::str::FromStr;

use workbench_core::{
    AccessMode, AgentCapabilities, AgentCommand, AgentCommandId, AgentDefinition, AgentDiscovery,
    AgentId, AgentOptionId, AgentSource, ApplyMode, AttemptId, BusinessError, CapabilityOption,
    Command, CommandEnvelope, CommandId, ConnectionEpoch, ConnectionPhase, ContentBlock,
    ContentBlockId, EffectiveConfiguration, ErrorKind, Event, EventEnvelope, EventId, EventOrigin,
    EventSource, EventSourceId, ExternalSessionBinding, ExternalSessionId, InputPart, JournalEntry,
    JournalRecord, Message, MessageId, MessageRole, ModelCapability, PermissionCapability, Project,
    ProjectId, QueuedInput, QueuedInputId, RetryAdvice, RunContext, RunEnd, RunId, RunResult,
    Scope, SessionId, SessionRecord, TextFormat, Timestamp, WorkbenchState, WorkspacePreferences,
    examples, reduce, replay,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixturePreset {
    RichWorkspace,
    InterleavedContract,
    EmptyProject,
}

impl FixturePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::RichWorkspace => "开发样例：多项目与多状态",
            Self::InterleavedContract => "开发样例：T02 契约交错样例",
            Self::EmptyProject => "开发样例：空项目无会话",
        }
    }

    pub fn from_env_or_args() -> Option<Self> {
        for arg in std::env::args().skip(1) {
            if arg == "--dev-fixture" {
                return Some(Self::RichWorkspace);
            }
            if let Some(rest) = arg.strip_prefix("--dev-fixture=") {
                return Self::from_name(rest);
            }
        }
        if let Ok(val) = std::env::var("AGENT_WORKBENCH_DEV_FIXTURE") {
            return Self::from_name(&val);
        }
        None
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim() {
            "" | "0" | "false" | "off" | "live" => None,
            "1" | "true" | "rich" | "all" => Some(Self::RichWorkspace),
            "contract" | "interleaved" | "t02" => Some(Self::InterleavedContract),
            "empty-project" | "empty" => Some(Self::EmptyProject),
            _ => Some(Self::RichWorkspace),
        }
    }
}

fn fixed_id<T: FromStr>(suffix: u128) -> T {
    let text = format!("00000000-0000-0000-0000-{suffix:012x}");
    text.parse()
        .unwrap_or_else(|_| panic!("invalid fixed UUID suffix: {suffix}"))
}

struct RecordSeq {
    records: Vec<JournalRecord>,
}

impl RecordSeq {
    fn new(records: Vec<JournalRecord>) -> Self {
        Self { records }
    }

    fn command(&mut self, scope: Scope, command: Command) {
        let sequence = self.records.len() as u64 + 1;
        let id: CommandId = fixed_id(50_000 + sequence as u128);
        let at = Timestamp::new(2_000 + sequence as i64);
        self.records.push(JournalRecord::new(
            sequence,
            JournalEntry::Command(CommandEnvelope::new(id, at, scope, command)),
        ));
    }

    fn event(&mut self, scope: Scope, event: Event) {
        let sequence = self.records.len() as u64 + 1;
        let id: EventId = fixed_id(60_000 + sequence as u128);
        let source_id: EventSourceId = fixed_id(3_000);
        let at = Timestamp::new(2_000 + sequence as i64);
        self.records.push(JournalRecord::new(
            sequence,
            JournalEntry::Event(EventEnvelope::new(
                id,
                at,
                scope,
                EventSource::new(
                    source_id,
                    EventOrigin::Runtime {
                        adapter: "codex-acp-fixture".into(),
                        version: "0.1.0".into(),
                    },
                ),
                event,
            )),
        ));
    }
}

fn rich_capabilities(
    epoch: ConnectionEpoch,
    revision: u64,
) -> (AgentCapabilities, EffectiveConfiguration) {
    let (mut capabilities, mut effective) = examples::confirmed_example(epoch, revision);
    capabilities.models.clear();

    let mut primary_model = ModelCapability::new(CapabilityOption::new(
        AgentOptionId::new("gpt-5.4"),
        "GPT-5.4",
    ));
    primary_model.reasoning_efforts = vec![
        CapabilityOption::new(AgentOptionId::new("low"), "低"),
        CapabilityOption::new(AgentOptionId::new("medium"), "中"),
        CapabilityOption::new(AgentOptionId::new("high"), "高"),
        CapabilityOption::new(AgentOptionId::new("xhigh"), "极高"),
    ];
    capabilities.models.push(primary_model);

    let mut secondary_model = ModelCapability::new(CapabilityOption::new(
        AgentOptionId::new("gpt-5.3-codex"),
        "GPT-5.3 Codex",
    ));
    secondary_model.reasoning_efforts = vec![
        CapabilityOption::new(AgentOptionId::new("medium"), "中"),
        CapabilityOption::new(AgentOptionId::new("high"), "高"),
    ];
    capabilities.models.push(secondary_model);

    capabilities.permission_modes = vec![
        PermissionCapability::new(
            CapabilityOption::new(AgentOptionId::new("agent-full-access"), "完整访问"),
            AccessMode::FullAccess,
        ),
        PermissionCapability::new(
            CapabilityOption::new(AgentOptionId::new("workspace-write"), "工作区写入"),
            AccessMode::Restricted,
        ),
        PermissionCapability::new(
            CapabilityOption::new(AgentOptionId::new("read-only"), "只读访问"),
            AccessMode::Restricted,
        ),
    ];

    capabilities.commands = vec![
        AgentCommand::new(AgentCommandId::new("review"), "/review"),
        AgentCommand::new(AgentCommandId::new("compact"), "/compact"),
        AgentCommand::new(AgentCommandId::new("status"), "/status"),
    ];

    effective.model = Some(AgentOptionId::new("gpt-5.4"));
    effective.reasoning_effort = Some(AgentOptionId::new("high"));
    effective.permission = Some(AgentOptionId::new("agent-full-access"));
    (capabilities, effective)
}

pub fn build_fixture(preset: FixturePreset) -> (WorkbenchState, WorkspacePreferences) {
    match preset {
        FixturePreset::InterleavedContract => {
            let example = examples::interleaved_example();
            let state = replay(WorkbenchState::new(), &example.records)
                .expect("T02 interleaved example must replay cleanly");
            let mut prefs = WorkspacePreferences::default();
            let first_project_id: ProjectId = fixed_id(1);
            prefs.selected_project_id = Some(first_project_id);
            prefs.selected_session_id = Some(example.sessions[0]);
            prefs.expanded_projects.insert(first_project_id);
            (state, prefs)
        }
        FixturePreset::EmptyProject => {
            let at = Timestamp::new(1_000);
            let project_id: ProjectId = fixed_id(301);
            let agent_id: AgentId = fixed_id(302);
            let mut seq = RecordSeq::new(Vec::new());
            seq.command(
                Scope::default(),
                Command::PutProject {
                    project: Project::new(
                        project_id,
                        "跨平台原生智能代理工作台与本地终端工具链深度集成工程",
                        r"D:\开源工作区\跨平台原生智能代理工作台".into(),
                        at,
                    ),
                },
            );
            let mut agent =
                AgentDefinition::new(agent_id, "Codex", AgentSource::LocalConfiguration);
            agent.discovery = AgentDiscovery::Installed {
                executable: r"codex-acp.exe".into(),
                version: Some("2.5.0".into()),
                distribution_id: Some("codex-acp-win32-x64".into()),
            };
            seq.command(Scope::default(), Command::PutAgent { agent });
            let state =
                replay(WorkbenchState::new(), &seq.records).expect("empty project fixture replays");
            let mut prefs = WorkspacePreferences::default();
            prefs.selected_project_id = Some(project_id);
            prefs.expanded_projects.insert(project_id);
            (state, prefs)
        }
        FixturePreset::RichWorkspace => {
            let base = examples::interleaved_example();
            let mut seq = RecordSeq::new(base.records.clone());
            let epoch = ConnectionEpoch::new(1);
            let at = Timestamp::new(3_000);

            let project_1_id: ProjectId = fixed_id(1);
            let project_2_id: ProjectId = fixed_id(201);
            let agent_id: AgentId = fixed_id(2);

            let mut codex_agent =
                AgentDefinition::new(agent_id, "Codex", AgentSource::LocalConfiguration);
            codex_agent.discovery = AgentDiscovery::Installed {
                executable: r"codex-acp.exe".into(),
                version: Some("2.5.0".into()),
                distribution_id: Some("codex-acp-win32-x64".into()),
            };
            seq.command(Scope::default(), Command::PutAgent { agent: codex_agent });

            // Second project with a realistic long Chinese name.
            seq.command(
                Scope::default(),
                Command::PutProject {
                    project: Project::new(
                        project_2_id,
                        "跨平台原生智能代理工作台与本地终端工具链深度集成工程",
                        r"D:\开源工作区\跨平台原生智能代理工作台与本地终端工具链深度集成工程"
                            .into(),
                        at,
                    ),
                },
            );

            let s0_scope = Scope::session(base.sessions[0], epoch);
            let s0_run_scope = s0_scope.with_run(base.runs[0]);
            seq.command(
                s0_scope,
                Command::RenameSession {
                    title: "建立克制的炭灰双栏工作区与焦点滚动归属".into(),
                },
            );
            let (caps, effective) = rich_capabilities(epoch, 2);
            seq.event(
                s0_scope,
                Event::ConfigurationReported {
                    effective: effective.clone(),
                    capabilities: caps.clone(),
                },
            );

            let user_msg_id: MessageId = fixed_id(501);
            let user_block_id: ContentBlockId = fixed_id(601);
            let mut user_msg = Message::new(
                user_msg_id,
                RunContext::new(base.sessions[0], base.runs[0]),
                MessageRole::User,
                at,
            );
            user_msg.submission_id = Some(fixed_id(100));
            user_msg.blocks.push(ContentBlock::new(
                user_block_id,
                workbench_core::ContentKind::Text {
                    text: "请核对共享快照契约，整理双栏连续表面、侧栏伸缩与独立滚动归属，保持界面无多余说明文字。".into(),
                    format: TextFormat::Plain,
                },
            ));
            seq.event(s0_run_scope, Event::MessageStarted { message: user_msg });
            seq.event(
                s0_run_scope,
                Event::MessageCompleted {
                    message_id: user_msg_id,
                },
            );

            seq.event(
                s0_run_scope,
                Event::TextDelta {
                    message_id: base.messages[0],
                    block_id: base.blocks[0],
                    delta: "：\n\n已完成 `workbench-core` 快照与工作区视图边界核对，核心对齐关系如下：\n\n| 区域 | 状态来源 | 滚动归属 |\n| --- | --- | --- |\n| Sidebar 会话树 | `WorkbenchSnapshot` | 侧栏列表独立滚动 |\n| 对话消息流 | `SessionSnapshot` | `MessageScroller` 尾随滚动 |\n| 底部 Composer | `ComposerDraft` + `InputQueue` | 固定容器，超长时内部滚动 |\n\n```rust\npub fn sync_session_snapshot(&mut self, session_id: SessionId) {\n    self.active_session = self.state.session_snapshot(session_id);\n}\n```".into(),
                },
            );

            let s1_scope = Scope::session(base.sessions[1], epoch);
            seq.command(
                s1_scope,
                Command::RenameSession {
                    title: "并行会话：验证同目录多任务状态与草稿互不串扰".into(),
                },
            );
            seq.event(
                s1_scope,
                Event::ConfigurationReported {
                    effective: effective.clone(),
                    capabilities: caps.clone(),
                },
            );

            // Session 2 in Project 2: Completed conversation with saved draft.
            let session_2_id: SessionId = fixed_id(210);
            let run_2_id: RunId = fixed_id(220);
            let attempt_2_id: AttemptId = fixed_id(230);
            let req_2_id: workbench_core::RequestId = fixed_id(240);
            seq.command(
                Scope::default(),
                Command::CreateSession {
                    session: SessionRecord::new(
                        session_2_id,
                        project_2_id,
                        agent_id,
                        "清理启动页重复说明与中央完整路径副标题",
                        at,
                    ),
                },
            );
            seq.command(
                Scope::session(session_2_id, ConnectionEpoch::INITIAL),
                Command::Connect { resume: false },
            );
            let s2_scope = Scope::session(session_2_id, epoch);
            seq.event(
                s2_scope,
                Event::ConnectionChanged {
                    phase: ConnectionPhase::Ready,
                },
            );
            seq.event(
                s2_scope,
                Event::SessionRegistered {
                    binding: ExternalSessionBinding::new(
                        ExternalSessionId::new("fixture-session-2"),
                        at,
                    ),
                    resumed: false,
                },
            );
            seq.event(
                s2_scope,
                Event::ConfigurationReported {
                    effective: effective.clone(),
                    capabilities: caps.clone(),
                },
            );
            let mut s2_draft = workbench_core::ComposerDraft::new(session_2_id, at);
            s2_draft.revision = 1;
            s2_draft.content = vec![InputPart::Text {
                text: "检查最小窗口 980×640 与 150% 缩放下的底部控件密度。".into(),
            }];
            let s2_submission = workbench_core::InputSubmission::from_draft(
                fixed_id(250),
                &s2_draft,
                workbench_core::SubmissionIntent::StartRun { run_id: run_2_id },
                at,
            );
            let s2_run_scope = s2_scope.with_run(run_2_id).with_request(req_2_id);
            seq.command(
                s2_run_scope,
                Command::Submit {
                    submission: s2_submission,
                    attempt_id: attempt_2_id,
                },
            );
            seq.event(
                s2_run_scope,
                Event::Attempt {
                    attempt_id: attempt_2_id,
                    outcome: workbench_core::AttemptOutcome::PromptAccepted,
                },
            );
            let s2_agent_msg_id: MessageId = fixed_id(270);
            let mut s2_agent_msg = Message::new(
                s2_agent_msg_id,
                RunContext::new(session_2_id, run_2_id),
                MessageRole::Agent,
                at,
            );
            s2_agent_msg.blocks.push(ContentBlock::new(
                fixed_id(271),
                workbench_core::ContentKind::Text {
                    text: "已清理启动页中的重复项目名、中央完整路径以及“工作目录已选择”说明文字，空状态仅保留主操作入口。".into(),
                    format: TextFormat::Markdown,
                },
            ));
            seq.event(
                s2_run_scope,
                Event::MessageStarted {
                    message: s2_agent_msg,
                },
            );
            seq.event(
                s2_run_scope,
                Event::MessageCompleted {
                    message_id: s2_agent_msg_id,
                },
            );
            seq.event(
                s2_run_scope,
                Event::Attempt {
                    attempt_id: attempt_2_id,
                    outcome: workbench_core::AttemptOutcome::PromptReturned {
                        end: RunEnd::Completed {
                            result: RunResult::EndOfTurn,
                        },
                    },
                },
            );
            seq.command(
                s2_scope,
                Command::SaveDraft {
                    draft: s2_draft,
                    expected_revision: 0,
                },
            );

            // Session 3 in Project 2: Authenticating / Connecting state.
            let session_3_id: SessionId = fixed_id(211);
            seq.command(
                Scope::default(),
                Command::CreateSession {
                    session: SessionRecord::new(
                        session_3_id,
                        project_2_id,
                        agent_id,
                        "连接中：正在协商 ACP 能力与登录状态",
                        at,
                    ),
                },
            );
            seq.command(
                Scope::session(session_3_id, ConnectionEpoch::INITIAL),
                Command::Connect { resume: false },
            );
            seq.event(
                Scope::session(session_3_id, epoch),
                Event::ConnectionChanged {
                    phase: ConnectionPhase::Authenticating,
                },
            );

            // Session 4 in Project 2: Failed connection & paused queue recovery state.
            let session_4_id: SessionId = fixed_id(212);
            seq.command(
                Scope::default(),
                Command::CreateSession {
                    session: SessionRecord::new(
                        session_4_id,
                        project_2_id,
                        agent_id,
                        "连接异常：未检测到有效认证或进程已断开",
                        at,
                    ),
                },
            );
            seq.command(
                Scope::session(session_4_id, ConnectionEpoch::INITIAL),
                Command::Connect { resume: false },
            );
            let s4_scope = Scope::session(session_4_id, epoch);
            let mut s4_queued_draft = workbench_core::ComposerDraft::new(session_4_id, at);
            s4_queued_draft.content = vec![InputPart::Text {
                text: "恢复连接后继续执行回归检查".into(),
            }];
            let s4_queue_id: QueuedInputId = fixed_id(290);
            seq.command(
                s4_scope,
                Command::Enqueue {
                    input: QueuedInput::from_draft(s4_queue_id, &s4_queued_draft, at),
                },
            );
            seq.event(
                s4_scope,
                Event::ConnectionChanged {
                    phase: ConnectionPhase::Failed {
                        reason: "本地 codex-acp 认证失败：请完成 ChatGPT 登录或检查 API Key 配置"
                            .into(),
                    },
                },
            );
            seq.event(
                s4_scope,
                Event::Error {
                    error: BusinessError::new(
                        ErrorKind::Authentication,
                        "Agent 要求先完成认证后才能启动会话运行；未发送队列已暂停并保留草稿。",
                        RetryAdvice::AfterReconnect,
                    ),
                },
            );

            let mut state = WorkbenchState::new();
            for record in &seq.records {
                let transition = reduce(&state, record, ApplyMode::Replay)
                    .expect("rich fixture record must apply cleanly");
                state = transition.into_parts().0;
            }

            let mut prefs = WorkspacePreferences::default();
            prefs.selected_project_id = Some(project_1_id);
            prefs.selected_session_id = Some(base.sessions[0]);
            prefs.expanded_projects.insert(project_1_id);
            prefs.expanded_projects.insert(project_2_id);
            (state, prefs)
        }
    }
}
