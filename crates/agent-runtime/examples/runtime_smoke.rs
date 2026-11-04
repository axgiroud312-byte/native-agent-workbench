//! Real shared-contract workflow. Requires the user's officially authenticated Codex.
//! All journal state here is in memory; this does not test SQLite or the UI.
use agent_runtime::{AgentResolver, AgentRuntime, RuntimeOptions};
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use workbench_core::*;

#[derive(Clone)]
struct Sink(mpsc::Sender<EventEnvelope>);
impl RuntimeEventSink for Sink {
    fn emit(&self, event: EventEnvelope) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.0.send(event).await.map_err(|_| {
                BusinessError::new(
                    ErrorKind::Transport,
                    "smoke receiver closed",
                    RetryAdvice::Never,
                )
            })
        })
    }
}
struct Driver {
    state: WorkbenchState,
    runtime: AgentRuntime,
    sink: Sink,
    events: mpsc::Receiver<EventEnvelope>,
    failures: mpsc::UnboundedReceiver<String>,
    failure_tx: mpsc::UnboundedSender<String>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    chunks: usize,
    tools: usize,
    configurations: usize,
}
impl Driver {
    fn new(runtime: AgentRuntime) -> Self {
        let (tx, rx) = mpsc::channel(512);
        let (failure_tx, failures) = mpsc::unbounded_channel();
        Self {
            state: WorkbenchState::new(),
            runtime,
            sink: Sink(tx),
            events: rx,
            failures,
            failure_tx,
            tasks: Vec::new(),
            chunks: 0,
            tools: 0,
            configurations: 0,
        }
    }
    fn record(&mut self, entry: JournalEntry) -> Result<(), Box<dyn std::error::Error>> {
        let record = JournalRecord::new(self.state.sequence() + 1, entry);
        let transition = reduce(&self.state, &record, ApplyMode::Live)?;
        if let ApplyDisposition::Rejected(error) = transition.disposition() {
            return Err(format!("shared reducer rejected a real fact/command: {error}").into());
        }
        let (state, effects, _) = transition.into_parts();
        self.state = state;
        for effect in effects {
            // This synchronous admission happens before any forwarding future is spawned.
            let ticket = self.runtime.enqueue(effect);
            let sink = self.sink.clone();
            let failures = self.failure_tx.clone();
            self.tasks.push(tokio::task::spawn_local(async move {
                if let Err(error) = ticket.forward(&sink).await {
                    let _ = failures.send(error.to_string());
                }
            }));
        }
        Ok(())
    }
    fn command(
        &mut self,
        scope: Scope,
        command: Command,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.record(JournalEntry::Command(CommandEnvelope::new(
            CommandId::new(),
            now(),
            scope,
            command,
        )))
    }
    async fn until(
        &mut self,
        label: &str,
        condition: impl Fn(&WorkbenchState) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = tokio::time::sleep(Duration::from_secs(90));
        tokio::pin!(deadline);
        while !condition(&self.state) {
            tokio::select! {
                _=&mut deadline=>return Err(format!("timed out: {label}").into()),
                Some(error)=self.failures.recv()=>return Err(format!("RuntimePort failed while {label}: {error}").into()),
                event=self.events.recv()=>{
                    let event=event.ok_or("Runtime event stream closed")?;
                    match &event.event {Event::TextDelta {..}=>self.chunks+=1,Event::Tool {..}=>self.tools+=1,Event::ConfigurationReported {..}=>self.configurations+=1,Event::Error {error}=>eprintln!("Agent diagnostic: {error}"),_=>{}}
                    self.record(JournalEntry::Event(event))?;
                }
            }
        }
        println!("verified: {label}");
        Ok(())
    }
    fn scope(&self, id: SessionId) -> Scope {
        Scope::session(
            id,
            self.state
                .session_snapshot(id)
                .unwrap()
                .record
                .connection
                .epoch,
        )
    }
    fn submit(
        &mut self,
        session: SessionId,
        text: &str,
    ) -> Result<RunId, Box<dyn std::error::Error>> {
        let mut draft = self.state.session_snapshot(session).unwrap().draft;
        let revision = draft.revision;
        draft.revision += 1;
        draft.content = vec![InputPart::Text { text: text.into() }];
        draft.updated_at = now();
        self.command(
            self.scope(session),
            Command::SaveDraft {
                draft: draft.clone(),
                expected_revision: revision,
            },
        )?;
        let run = RunId::new();
        let submission = InputSubmission::from_draft(
            SubmissionId::new(),
            &draft,
            SubmissionIntent::StartRun { run_id: run },
            now(),
        );
        self.command(
            self.scope(session)
                .with_run(run)
                .with_request(RequestId::new()),
            Command::Submit {
                submission,
                attempt_id: AttemptId::new(),
            },
        )?;
        Ok(run)
    }
}
fn now() -> Timestamp {
    Timestamp::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
    )
}
fn ended(state: &WorkbenchState, id: SessionId, run: RunId) -> bool {
    state
        .session_snapshot(id)
        .unwrap()
        .runs
        .iter()
        .any(|r| r.context.run_id == run && r.state.is_terminal())
}
fn text(state: &WorkbenchState, id: SessionId) -> String {
    state
        .session_snapshot(id)
        .unwrap()
        .messages
        .into_iter()
        .filter(|m| m.role == MessageRole::Agent)
        .flat_map(|m| m.blocks)
        .filter_map(|b| match b.content {
            ContentPresentation::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}
struct Catalog(AgentDefinition);
impl AgentResolver for Catalog {
    fn resolve(
        &self,
        id: AgentId,
    ) -> futures::future::BoxFuture<'_, Result<AgentDefinition, BusinessError>> {
        let agent = self.0.clone();
        Box::pin(async move {
            if agent.id == id {
                Ok(agent)
            } else {
                Err(BusinessError::new(
                    ErrorKind::NotFound,
                    "Agent not registered",
                    RetryAdvice::Never,
                ))
            }
        })
    }
}
#[derive(Default)]
struct AuthSink(RefCell<Vec<AuthenticationNotice>>);
impl AuthenticationNoticeSink for AuthSink {
    fn emit_authentication(&self, notice: AuthenticationNotice) -> PortFuture<'_, ()> {
        self.0.borrow_mut().push(notice);
        Box::pin(async { Ok(()) })
    }
}
async fn authentication_only(args: &[PathBuf]) -> Result<(), Box<dyn std::error::Error>> {
    let mut agent = AgentDefinition::new(AgentId::new(), "Codex", AgentSource::LocalConfiguration);
    agent.launch = Some(AgentLaunchConfiguration::new(
        args[0].clone(),
        vec![args[1].to_string_lossy().into()],
    ));
    let scope = AuthenticationScope::agent(agent.id, ConnectionEpoch::new(1));
    let runtime = AgentRuntime::new(
        RuntimeOptions::new()
            .with_codex_executable(args[2].clone())
            .with_agents(Arc::new(Catalog(agent))),
    )?;
    let sink = AuthSink::default();
    let result=async {
        runtime.execute_authentication(AuthenticationRequest::new(RequestId::new(),scope,AuthenticationAction::Discover),&sink).await?;
        let methods=sink.0.borrow().iter().find_map(|notice|match &notice.update {AuthenticationUpdate::Methods {methods}=>Some(methods.clone()),_=>None}).ok_or("typed authentication methods were not delivered")?;
        if !methods.iter().any(|m|m.id.as_str()=="chat-gpt"&&m.availability==AuthenticationMethodAvailability::Available) {return Err("official ChatGPT method not available".into());}
        if !methods.iter().any(|m|m.id.as_str()=="api-key"&&m.availability==AuthenticationMethodAvailability::Unavailable {reason:AuthenticationUnavailableReason::SecretEntryUnsupported}) {return Err("secret entry method was not marked unavailable".into());}
        for method in methods {println!("typed authentication: id={} kind={:?} availability={:?}",method.id.as_str(),method.kind,method.availability);}
        let denied=runtime.execute_authentication(AuthenticationRequest::new(RequestId::new(),scope,AuthenticationAction::Authenticate {method_id:AuthenticationMethodId::new("api-key")}),&sink).await;
        if denied.is_ok()||!sink.0.borrow().iter().any(|notice|matches!(notice.update,AuthenticationUpdate::Status {status:AuthenticationStatus::Failed {failure:AuthenticationFailure::MethodUnavailable}})) {return Err("unavailable credential method was unexpectedly invoked".into());}
        println!("authentication_port_ok: real initialize -> typed discovery; credential entry unavailable; no browser login or journal writes");
        Ok::<(),Box<dyn std::error::Error>>(())
    }.await;
    runtime.shutdown().await?;
    result
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let auth_only = raw.last().is_some_and(|arg| arg == "--auth-only");
    if auth_only {
        raw.pop();
    }
    let args = raw.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: runtime_smoke NODE_EXE ADAPTER_JS CODEX_EXE ISOLATED_DIRECTORY [--auth-only]"
                .into(),
        );
    }
    if auth_only {
        return authentication_only(&args).await;
    }
    std::fs::create_dir_all(&args[3])?;
    let runtime = AgentRuntime::new(RuntimeOptions::new().with_codex_executable(args[2].clone()))?;
    let runtime_cleanup = runtime.clone();
    let result=tokio::task::LocalSet::new().run_until(async move {
        let mut driver=Driver::new(runtime);
        let project=Project::new(ProjectId::new(),"Runtime isolated smoke",args[3].clone(),now());
        let mut agent=AgentDefinition::new(AgentId::new(),"Codex",AgentSource::LocalConfiguration);
        agent.launch=Some(AgentLaunchConfiguration::new(args[0].clone(),vec![args[1].to_string_lossy().into()]));
        driver.command(Scope::default(),Command::PutProject {project:project.clone()})?;
        driver.command(Scope::default(),Command::PutAgent {agent:agent.clone()})?;
        let sessions=[SessionId::new(),SessionId::new()];
        for id in sessions {
            driver.command(Scope::default(),Command::CreateSession {session:SessionRecord::new(id,project.id,agent.id,"smoke",now())})?;
            driver.command(Scope::session(id,ConnectionEpoch::INITIAL),Command::Connect {resume:false})?;
        }
        driver.until("two independent real connections",|state|sessions.iter().all(|id|state.session_snapshot(*id).is_some_and(|s|s.record.connection.phase==ConnectionPhase::Ready))).await?;
        let external=sessions.map(|id|driver.state.session_snapshot(id).unwrap().record.external.unwrap().id);
        if external[0]==external[1] {return Err("sessions unexpectedly share an external identity".into());}
        for id in sessions {
            let snapshot=driver.state.session_snapshot(id).unwrap();let caps=snapshot.record.capabilities.unwrap();let effective=snapshot.record.configuration.effective.unwrap();
            println!("capabilities: models={} permission_modes={} model={} effort={} permission={} steering={:?} resume={:?}",caps.models.len(),caps.permission_modes.len(),effective.model.as_ref().map_or("unknown",|v|v.as_str()),effective.reasoning_effort.as_ref().map_or("unknown",|v|v.as_str()),effective.permission.as_ref().map_or("unknown",|v|v.as_str()),caps.steering,caps.session_resume);
        }
        let first=driver.submit(sessions[0],"Reply with exactly RUNTIME_A. Do not use tools.")?;
        let second=driver.submit(sessions[1],"Reply with exactly RUNTIME_B. Do not use tools.")?;
        driver.until("interleaved prompts finish in their original sessions",|state|ended(state,sessions[0],first)&&ended(state,sessions[1],second)).await?;
        let a=text(&driver.state,sessions[0]);let b=text(&driver.state,sessions[1]);
        if !a.contains("RUNTIME_A")||a.contains("RUNTIME_B")||!b.contains("RUNTIME_B")||b.contains("RUNTIME_A") {return Err("real session output routing failed".into());}
        // Actual option IDs come from the Agent; this verifies a full snapshot plus settlement.
        let snapshot=driver.state.session_snapshot(sessions[1]).unwrap();
        let mut requested=snapshot.record.configuration.requested;
        requested.model=snapshot.record.configuration.effective.as_ref().and_then(|c|c.model.clone());
        requested.reasoning_effort=snapshot.record.configuration.effective.as_ref().and_then(|c|c.reasoning_effort.clone());
        let revision=snapshot.record.capabilities.unwrap().revision;
        driver.command(driver.scope(sessions[1]).with_request(RequestId::new()),Command::Configure {requested,capability_revision:revision})?;
        driver.until("configuration settles on a complete confirmed snapshot",|state|state.session_snapshot(sessions[1]).unwrap().record.configuration.pending.is_none()).await?;
        let waiting=driver.submit(sessions[0],"Use the shell to run PowerShell Start-Sleep -Seconds 30. Do not write any files. After the command finishes, reply RUNTIME_WAIT_DONE.")?;
        let quick=driver.submit(sessions[1],"Reply with exactly RUNTIME_OTHER_RUNNING. Do not use tools.")?;
        driver.until("another session completes while the first runs",|state|ended(state,sessions[1],quick)).await?;
        if ended(&driver.state,sessions[0],waiting) {return Err("long run ended before concurrency could be observed".into());}
        driver.until("real tool is running before cancellation",|state|state.session_snapshot(sessions[0]).unwrap().tools.iter().any(|t|t.context.run_id==waiting&&matches!(t.state,ToolState::Running))).await?;
        let mut queued=driver.state.session_snapshot(sessions[0]).unwrap().draft;
        queued.content=vec![InputPart::Text {text:"UNSENT_PRESERVED".into()}];
        driver.command(driver.scope(sessions[0]),Command::Enqueue {input:QueuedInput::from_draft(QueuedInputId::new(),&queued,now())})?;
        driver.command(driver.scope(sessions[0]).with_run(waiting).with_request(RequestId::new()),Command::Stop)?;
        driver.until("original prompt confirms cancellation",|state|state.session_snapshot(sessions[0]).unwrap().runs.iter().any(|r|r.context.run_id==waiting&&matches!(r.state,ExecutionState::Cancelled {..}))).await?;
        let stopped=driver.state.session_snapshot(sessions[0]).unwrap();
        if !matches!(stopped.queue.dispatch,QueueDispatchState::Paused {..})||stopped.can_dispatch_queue||!stopped.queue.entries.iter().any(|input|input.content==queued.content) {return Err("stop did not preserve and pause the queue".into());}
        println!("verified: stopped queue remains paused with unsent content");
        for id in sessions {driver.command(driver.scope(id),Command::CloseSession)?;}
        driver.until("both owned resources are released",|state|sessions.iter().all(|id|state.session_snapshot(*id).is_some_and(|s|s.record.connection.phase==ConnectionPhase::Closed&&!s.closing))).await?;
        println!("real_runtime_ok: streamed_chunks={} tool_updates={} configuration_reports={} journal_records={}",driver.chunks,driver.tools,driver.configurations,driver.state.sequence());
        Ok::<(),Box<dyn std::error::Error>>(())
    }).await;
    runtime_cleanup.shutdown().await?;
    result
}
