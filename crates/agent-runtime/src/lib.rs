//! Background ACP process adapter. Business state remains in workbench-core.
//!
//! Enqueue effects in committed sequence/ordinal order, then forward their tickets
//! concurrently. `execute` enqueues when called, before its future is polled.
//! Keep Connect's forwarding task at application scope until release.

mod authentication;
mod connection;
mod interaction;
mod mapping;
mod process;
#[cfg(test)]
mod tests;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot, watch};
use workbench_core::*;

/// Application-owned attachment access. No SDK, file path guessing, or business state copy.
pub trait AttachmentResolver: Send + Sync {
    fn resolve(
        &self,
        session: SessionId,
        attachment: AttachmentId,
    ) -> futures::future::BoxFuture<'_, Result<(Attachment, Vec<u8>), BusinessError>>;
}
/// Application catalog lookup, returning the existing shared definition without credentials.
pub trait AgentResolver: Send + Sync {
    fn resolve(
        &self,
        id: AgentId,
    ) -> futures::future::BoxFuture<'_, Result<AgentDefinition, BusinessError>>;
}

#[derive(Clone)]
pub struct RuntimeOptions {
    pub(crate) codex_executable: Option<PathBuf>,
    pub(crate) attachments: Option<Arc<dyn AttachmentResolver>>,
    pub(crate) agents: Option<Arc<dyn AgentResolver>>,
    pub(crate) event_capacity: usize,
    pub(crate) operation_capacity: usize,
    pub(crate) setup_timeout: Duration,
}
impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            codex_executable: None,
            attachments: None,
            agents: None,
            event_capacity: 512,
            operation_capacity: 64,
            setup_timeout: Duration::from_secs(60),
        }
    }
}
impl RuntimeOptions {
    pub fn new() -> Self {
        Self::default()
    }
    /// Explicit probe override. Product launches use each adapter's validated local dependency.
    pub fn with_codex_executable(mut self, path: PathBuf) -> Self {
        self.codex_executable = Some(path);
        self
    }
    pub fn with_attachments(mut self, resolver: Arc<dyn AttachmentResolver>) -> Self {
        self.attachments = Some(resolver);
        self
    }
    pub fn with_agents(mut self, resolver: Arc<dyn AgentResolver>) -> Self {
        self.agents = Some(resolver);
        self
    }
    pub fn with_event_capacity(mut self, capacity: usize) -> Self {
        self.event_capacity = capacity.max(16);
        self
    }
}

struct Work {
    effect: RuntimeEffect,
    facts: mpsc::Sender<EventEnvelope>,
    done: oneshot::Sender<Result<(), BusinessError>>,
}
struct Service {
    commands: mpsc::Sender<Work>,
    authentication: mpsc::Sender<authentication::AuthWork>,
    shutdown: watch::Sender<bool>,
    stopped: watch::Receiver<Option<Result<(), BusinessError>>>,
    capacity: usize,
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Cloneable command handle. The worker owns all processes independently of UI entities.
#[derive(Clone)]
pub struct AgentRuntime {
    service: Arc<Service>,
}
/// Local admission receipt and facts/completion stream; never a remote ACK.
#[must_use = "forward the ticket at application scope so its facts are consumed"]
pub struct RuntimeTicket {
    admitted: bool,
    events: mpsc::Receiver<EventEnvelope>,
    result: oneshot::Receiver<Result<(), BusinessError>>,
    lost: EventEnvelope,
}
impl RuntimeTicket {
    pub fn is_enqueued(&self) -> bool {
        self.admitted
    }
    pub fn forward<'a>(mut self, sink: &'a dyn RuntimeEventSink) -> PortFuture<'a, ()> {
        Box::pin(async move {
            while let Some(event) = self.events.recv().await {
                sink.emit(event).await?;
            }
            match self.result.await {
                Ok(result) => result,
                Err(_) => {
                    sink.emit(self.lost).await?;
                    Err(error(
                        ErrorKind::Transport,
                        "Runtime operation lost its owner; delivery needs reconciliation",
                        RetryAdvice::ReconcileFirst,
                    ))
                }
            }
        })
    }
}
impl AgentRuntime {
    pub fn new(options: RuntimeOptions) -> Result<Self, BusinessError> {
        let (tx, rx) = mpsc::channel(options.operation_capacity);
        let (auth, auth_rx) = mpsc::channel(options.operation_capacity);
        let (shutdown, stop) = watch::channel(false);
        let (exited, stopped) = watch::channel(None);
        let capacity = options.event_capacity;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                error(
                    ErrorKind::ResourceUnavailable,
                    "cannot initialize Runtime executor",
                    RetryAdvice::AfterCorrection,
                )
            })?;
        std::thread::Builder::new()
            .name("agent-runtime".into())
            .spawn(move || {
                let result = tokio::task::LocalSet::new()
                    .block_on(&runtime, serve(rx, auth_rx, stop, options));
                let _ = exited.send(Some(result));
            })
            .map_err(|_| {
                error(
                    ErrorKind::ResourceUnavailable,
                    "cannot create Runtime thread",
                    RetryAdvice::AfterCorrection,
                )
            })?;
        Ok(Self {
            service: Arc::new(Service {
                commands: tx,
                authentication: auth,
                shutdown,
                stopped,
                capacity,
            }),
        })
    }
    /// Call synchronously in JournalRecord.sequence and effect-list order. Rejected
    /// tickets still forward attributable facts such as Submit/NotSent.
    pub fn enqueue(&self, effect: RuntimeEffect) -> RuntimeTicket {
        let (facts, events) = mpsc::channel(self.service.capacity);
        let (done, result) = oneshot::channel();
        let lost = Facts::new(effect.scope, facts.clone()).envelope(
            effect.scope,
            failure_event(
                &effect,
                error(
                    ErrorKind::DeliveryUnknown,
                    "operation owner closed without a final receipt",
                    RetryAdvice::ReconcileFirst,
                ),
                false,
            ),
            command(&effect.cause),
        );
        let admitted = match self.service.commands.try_send(Work {
            effect,
            facts,
            done,
        }) {
            Ok(()) => true,
            Err(failed) => {
                reject(
                    failed.into_inner(),
                    error(
                        ErrorKind::Exhausted,
                        "Runtime admission queue is full or closed; operation was not started",
                        RetryAdvice::AfterReconnect,
                    ),
                );
                false
            }
        };
        RuntimeTicket {
            admitted,
            events,
            result,
            lost,
        }
    }
    /// Waits for owned cleanup. Existing Connect tickets must continue forwarding facts.
    pub fn shutdown(&self) -> PortFuture<'_, ()> {
        let _ = self.service.shutdown.send(true);
        let mut stopped = self.service.stopped.clone();
        Box::pin(async move {
            loop {
                let result = stopped.borrow_and_update().clone();
                if let Some(result) = result {
                    return result;
                }
                stopped.changed().await.map_err(|_| {
                    error(
                        ErrorKind::Transport,
                        "Runtime shutdown could not be confirmed",
                        RetryAdvice::ReconcileFirst,
                    )
                })?;
            }
        })
    }
}
impl AuthenticationPort for AgentRuntime {
    fn execute_authentication<'a>(
        &'a self,
        request: AuthenticationRequest,
        sink: &'a dyn AuthenticationNoticeSink,
    ) -> PortFuture<'a, ()> {
        let (notices, mut events) = mpsc::channel(64);
        let (done, result) = oneshot::channel();
        let scope = request.scope;
        let id = request.id;
        if let Err(failed) = self
            .service
            .authentication
            .try_send(authentication::AuthWork {
                request,
                notices,
                done,
            })
        {
            authentication::fail(failed.into_inner(), AuthenticationFailure::NoLiveConnection);
        }
        Box::pin(async move {
            while let Some(notice) = events.recv().await {
                sink.emit_authentication(notice).await?;
            }
            match result.await {
                Ok(result) => result,
                Err(_) => {
                    sink.emit_authentication(
                        AuthenticationNotice::new(
                            scope,
                            AuthenticationUpdate::Status {
                                status: AuthenticationStatus::Failed {
                                    failure: AuthenticationFailure::Transport,
                                },
                            },
                        )
                        .with_request(id),
                    )
                    .await?;
                    Err(error(
                        ErrorKind::Authentication,
                        "authentication owner closed without a receipt",
                        RetryAdvice::AfterReconnect,
                    ))
                }
            }
        })
    }
}
impl RuntimePort for AgentRuntime {
    fn execute<'a>(
        &'a self,
        effect: RuntimeEffect,
        sink: &'a dyn RuntimeEventSink,
    ) -> PortFuture<'a, ()> {
        self.enqueue(effect).forward(sink)
    }
}

async fn serve(
    mut commands: mpsc::Receiver<Work>,
    mut auth_commands: mpsc::Receiver<authentication::AuthWork>,
    mut shutdown: watch::Receiver<bool>,
    options: RuntimeOptions,
) -> Result<(), BusinessError> {
    use std::collections::BTreeMap;
    let mut sessions: BTreeMap<(SessionId, ConnectionEpoch), mpsc::Sender<Work>> = BTreeMap::new();
    let mut auth_routes: BTreeMap<AuthenticationScope, mpsc::Sender<authentication::AuthWork>> =
        BTreeMap::new();
    let mut agents: BTreeMap<AgentId, AgentDefinition> = BTreeMap::new();
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _=shutdown.changed()=>break,
            Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
            auth=auth_commands.recv()=>{
                let Some(work)=auth else {continue};let scope=work.request.scope;
                if let Some(route)=auth_routes.get(&scope) {
                    if let Err(failed)=route.try_send(work) {authentication::fail(failed.into_inner(),AuthenticationFailure::NoLiveConnection);}
                } else if scope.session_id().is_some() {authentication::fail(work,AuthenticationFailure::NoLiveConnection);}
                else if !matches!(work.request.action,AuthenticationAction::Discover|AuthenticationAction::Authenticate {..}) {authentication::fail(work,AuthenticationFailure::InteractionExpired);}
                else if auth_routes.keys().any(|old|old.agent_id()==scope.agent_id()&&old.session_id().is_none()&&old.epoch()>=scope.epoch()) {authentication::fail(work,AuthenticationFailure::StaleScope);}
                else {
                    auth_routes.retain(|old,_|old.agent_id()!=scope.agent_id()||old.session_id().is_some());
                    let (tx,rx)=mpsc::channel(options.operation_capacity);auth_routes.insert(scope,tx);
                    tasks.spawn_local(authentication::run(work,rx,shutdown.clone(),options.clone(),agents.get(&scope.agent_id()).cloned()));
                }
            }
            work=commands.recv()=>{
                let Some(work)=work else {break};
                let scope=work.effect.scope;
                let Some(key)=scope.session_id.zip(scope.epoch) else { reject(work,error(ErrorKind::InvalidInput,"Runtime needs a session and epoch",RetryAdvice::Never)); continue; };
                if matches!(work.effect.operation,RuntimeOperation::Connect {..}) {
                    if sessions.get(&key).is_some_and(|tx|!tx.is_closed()) {
                        let _=work.done.send(Err(error(ErrorKind::Busy,"connection already exists",RetryAdvice::Never))); continue;
                    }
                    let (tx,rx)=mpsc::channel(options.operation_capacity);
                    let (auth_tx,auth_rx)=mpsc::channel(options.operation_capacity);
                    if let RuntimeOperation::Connect {agent,..}=&work.effect.operation {
                        agents.insert(agent.id,agent.clone());auth_routes.insert(AuthenticationScope::session(agent.id,key.0,key.1),auth_tx);
                    }
                    sessions.insert(key,tx);
                    tasks.spawn_local(connection::run(work,rx,auth_rx,shutdown.clone(),options.clone()));
                } else if let RuntimeOperation::ReconcileClosingResources { abandoned_epoch }=work.effect.operation {
                    // No persisted OS ownership handle survives this instance. Never guess a PID.
                    let facts=Facts::new(scope,work.facts);
                    let result=facts.emit(scope,Event::ClosingResourcesReconciled {abandoned_epoch,
                        outcome:ResourceCleanupOutcome::Unconfirmed {reason:"no surviving owned process handle for the abandoned epoch".into()}},None);
                    let _=work.done.send(result);
                } else if let Some(tx)=sessions.get(&key) {
                    if let Err(err)=tx.try_send(work) {reject(err.into_inner(),error(ErrorKind::Exhausted,"session operation queue is full or closed",RetryAdvice::AfterReconnect));}
                } else {
                    reject(work,error(ErrorKind::NotReady,"session has no live connection",RetryAdvice::AfterReconnect));
                }
            }
        }
    }
    commands.close();
    while let Some(work) = commands.recv().await {
        reject(
            work,
            error(
                ErrorKind::NotReady,
                "Runtime is shutting down; operation was not started",
                RetryAdvice::AfterReconnect,
            ),
        );
    }
    auth_commands.close();
    while let Some(work) = auth_commands.recv().await {
        authentication::fail(work, AuthenticationFailure::NoLiveConnection);
    }
    sessions.clear();
    auth_routes.clear();
    if tokio::time::timeout(Duration::from_secs(10), async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        return Err(error(
            ErrorKind::Transport,
            "shutdown exceeded its cleanup deadline; resources require reconciliation",
            RetryAdvice::ReconcileFirst,
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct Facts {
    base: Scope,
    source: EventSource,
    sender: mpsc::Sender<EventEnvelope>,
}
impl Facts {
    fn new(base: Scope, sender: mpsc::Sender<EventEnvelope>) -> Self {
        Self {
            base,
            sender,
            source: EventSource::new(
                EventSourceId::new(),
                EventOrigin::Runtime {
                    adapter: "codex-acp".into(),
                    version: "2.5.0".into(),
                },
            ),
        }
    }
    fn envelope(&self, scope: Scope, event: Event, command: Option<CommandId>) -> EventEnvelope {
        let mut envelope =
            EventEnvelope::new(EventId::new(), now(), scope, self.source.clone(), event);
        envelope.command_id = command;
        envelope
    }
    fn emit(
        &self,
        scope: Scope,
        event: Event,
        command: Option<CommandId>,
    ) -> Result<(), BusinessError> {
        self.sender.try_send(self.envelope(scope,event,command)).map_err(|_|error(ErrorKind::Exhausted,"Runtime event channel is full or closed; execution status requires reconciliation",RetryAdvice::ReconcileFirst))
    }
    async fn final_event(&self, event: Event) {
        let _ = self
            .sender
            .send(self.envelope(self.base, event, None))
            .await;
    }
}
fn now() -> Timestamp {
    Timestamp::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64,
    )
}
fn error(kind: ErrorKind, message: impl Into<String>, retry: RetryAdvice) -> BusinessError {
    BusinessError::new(kind, message, retry)
}
fn command(cause: &EffectCause) -> Option<CommandId> {
    match cause {
        EffectCause::Command { id } => Some(*id),
        _ => None,
    }
}
fn reject(work: Work, reason: BusinessError) {
    let scope = work.effect.scope;
    let cause = command(&work.effect.cause);
    let facts = Facts::new(scope, work.facts);
    if matches!(work.effect.operation, RuntimeOperation::Connect { .. }) {
        let _ = facts.emit(
            scope,
            Event::Error {
                error: reason.clone(),
            },
            cause,
        );
        let _ = facts.emit(
            scope,
            Event::ConnectionChanged {
                phase: ConnectionPhase::Failed {
                    reason: reason.message.clone(),
                },
            },
            cause,
        );
        let _ = facts.emit(scope, Event::ResourcesReleased, cause);
    } else {
        let _ = facts.emit(
            scope,
            failure_event(&work.effect, reason.clone(), true),
            cause,
        );
    }
    let _ = work.done.send(Err(reason));
}
fn failure_event(effect: &RuntimeEffect, reason: BusinessError, not_started: bool) -> Event {
    match &effect.operation {
        RuntimeOperation::Submit { attempt, .. } => Event::Attempt {
            attempt_id: attempt.id,
            outcome: if not_started {
                AttemptOutcome::NotSent {
                    reason: reason.message.clone(),
                }
            } else {
                AttemptOutcome::Unknown {
                    reason: reason.message.clone(),
                }
            },
        },
        RuntimeOperation::Configure { .. } => Event::ConfigurationRejected {
            reason: reason.message.clone(),
        },
        RuntimeOperation::RespondPermission { attempt_id, .. } => Event::PermissionDelivery {
            attempt_id: *attempt_id,
            delivery: ResponseDelivery::Unknown {
                reason: reason.message.clone(),
            },
        },
        RuntimeOperation::RespondQuestion { attempt_id, .. } => Event::QuestionDelivery {
            attempt_id: *attempt_id,
            delivery: ResponseDelivery::Unknown {
                reason: reason.message.clone(),
            },
        },
        RuntimeOperation::ReconcileSubmission {
            submission_id,
            attempt,
        } => Event::SubmissionReconciled {
            submission_id: *submission_id,
            attempt_id: attempt.id,
            attempt_epoch: attempt.epoch,
            evidence: ConsumptionEvidence::Unknown {
                reason: reason.message.clone(),
            },
        },
        RuntimeOperation::ReconcileClosingResources { abandoned_epoch } => {
            Event::ClosingResourcesReconciled {
                abandoned_epoch: *abandoned_epoch,
                outcome: ResourceCleanupOutcome::Unconfirmed {
                    reason: reason.message.clone(),
                },
            }
        }
        RuntimeOperation::Connect { .. } => Event::ConnectionChanged {
            phase: ConnectionPhase::Failed {
                reason: reason.message.clone(),
            },
        },
        _ => Event::Error {
            error: reason.clone(),
        },
    }
}
