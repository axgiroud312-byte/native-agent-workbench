mod catalog;
mod navigation;
#[cfg(test)]
mod navigation_tests;
#[cfg(test)]
mod tests;

use crate::{
    api::*,
    bridge::{self, Bridge, EventSink},
};
use agent_catalog::{AgentCatalog, InstallPlan, InstallPlanId, ManualLaunch, RefreshPolicy};
use agent_runtime::AgentRuntime;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot, watch};
use workbench_core::*;
use workbench_storage::{CommittedUpdate, SqliteStorage, WorkbenchService};

pub(crate) type Reply<T> = oneshot::Sender<Result<T, BusinessError>>;
const INTENT_CAPACITY: usize = 128;
const EVENT_CAPACITY: usize = 512;
const MAX_EVENT_BATCH: usize = 32;

pub(crate) struct Owner(watch::Sender<bool>);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

pub(crate) enum Message {
    #[cfg(test)]
    FailNextViewSync {
        reply: Reply<()>,
    },
    #[cfg(test)]
    FailNextPreferencesWrite {
        reply: Reply<()>,
    },
    Command {
        command: CommandEnvelope,
        reply: Reply<CommandReceipt>,
    },
    OpenProject {
        directory: PathBuf,
        name: String,
        reply: Reply<Project>,
    },
    QuerySession {
        session_id: SessionId,
        reply: Reply<SessionReply>,
    },
    SelectProject {
        project_id: Option<ProjectId>,
        reply: Reply<u64>,
    },
    CreateSession {
        project_id: ProjectId,
        agent_id: AgentId,
        title: String,
        reply: Reply<SessionId>,
    },
    RemoveSession {
        session_id: SessionId,
        reply: Reply<()>,
    },
    RemoveProject {
        project_id: ProjectId,
        reply: Reply<()>,
    },
    RelocateProject {
        project_id: ProjectId,
        directory: PathBuf,
        reply: Reply<Project>,
    },
    SelectSession {
        session_id: Option<SessionId>,
        reply: Reply<u64>,
    },
    Preferences {
        expected_revision: u64,
        preferences: WorkspacePreferences,
        reply: Reply<u64>,
    },
    DiscoverAuthentication {
        agent_id: AgentId,
        reply: Reply<AuthenticationScope>,
    },
    Authentication {
        scope: AuthenticationScope,
        action: AuthenticationAction,
        reply: Reply<RequestId>,
    },
    RefreshAgents {
        policy: RefreshPolicy,
        reply: Reply<()>,
    },
    DiscoverAgents {
        reply: Reply<()>,
    },
    PrepareInstall {
        registry_id: String,
        reply: Reply<InstallPlan>,
    },
    Install {
        plan_id: InstallPlanId,
        reply: Reply<AgentId>,
    },
    ConfigureAgent {
        agent_id: Option<AgentId>,
        name: String,
        launch: ManualLaunch,
        reply: Reply<AgentId>,
    },
    Shutdown {
        reply: Reply<()>,
    },
    ResolveAgent {
        id: AgentId,
        reply: Reply<AgentDefinition>,
    },
    ResolveAttachment {
        session: SessionId,
        attachment: AttachmentId,
        reply: Reply<(Attachment, Vec<u8>)>,
    },
    AuthenticationNotice(AuthenticationNotice),
    Catalog(catalog::Completion),
    TicketDone(Result<(), BusinessError>),
    RuntimeStopped(Result<(), BusinessError>),
    AuthenticationStopped,
}

pub(crate) fn start(options: ServiceOptions) -> ServiceFuture<'static, ApplicationService> {
    // Starting the returned future is unnecessary for thread admission, just as for commands.
    let (ready, response) = oneshot::channel();
    let started = std::thread::Builder::new()
        .name("workbench-service".into())
        .spawn(move || {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match executor {
                Ok(executor) => tokio::task::LocalSet::new().block_on(&executor, async move {
                    match Actor::new(options).await {
                        Ok((actor, handle, receiver, events, owner)) => {
                            if ready.send(Ok(handle)).is_ok() {
                                actor.serve(receiver, events, owner).await;
                            }
                        }
                        Err(error) => {
                            let _ = ready.send(Err(error));
                        }
                    }
                }),
                Err(_) => {
                    let _ = ready.send(Err(fail(
                        ErrorKind::ResourceUnavailable,
                        "cannot initialize application executor",
                    )));
                }
            }
        });
    Box::pin(async move {
        started.map_err(|_| {
            fail(
                ErrorKind::ResourceUnavailable,
                "cannot start application worker",
            )
        })?;
        response.await.map_err(|_| closed())?
    })
}

struct Actor {
    storage: WorkbenchService,
    runtime: AgentRuntime,
    catalog: AgentCatalog,
    frame: ServiceSnapshot,
    updates: watch::Sender<Arc<ServiceSnapshot>>,
    sender: mpsc::Sender<Message>,
    events: EventSink,
    auth: Option<mpsc::Sender<AuthenticationRequest>>,
    auth_epoch: u64,
    auth_operations: BTreeMap<AuthenticationScope, RequestId>,
    plans: BTreeMap<InstallPlanId, (InstallPlan, u64)>,
    agent_edits: BTreeMap<AgentId, u64>,
    catalog_generation: u64,
    jobs: usize,
    tickets: usize,
    shutdown_replies: Vec<Reply<()>>,
    runtime_stopped: bool,
    auth_stopped: bool,
    shutdown_error: Option<BusinessError>,
    // A failed fact transaction halts admissions; no facts are dropped or forwarded again.
    failed_events: Option<Vec<EventEnvelope>>,
    removals: BTreeMap<RemovalTarget, navigation::Removal>,
    #[cfg(test)]
    fail_next_view_sync: bool,
    #[cfg(test)]
    fail_next_preferences_write: bool,
}
impl Actor {
    async fn new(
        options: ServiceOptions,
    ) -> Result<
        (
            Self,
            ApplicationService,
            mpsc::Receiver<Message>,
            mpsc::Receiver<EventEnvelope>,
            watch::Receiver<bool>,
        ),
        BusinessError,
    > {
        let disk = match options.data_root {
            Some(root) => SqliteStorage::open_at(root).await?,
            None => SqliteStorage::open_default().await?,
        };
        let catalog =
            AgentCatalog::new(disk.data_directory().to_path_buf()).map_err(catalog::error)?;
        let storage = WorkbenchService::start(disk, CommandId::new(), now()).await?;
        let workbench = storage.snapshot().await?;
        let (preferences_revision, preferences) = storage.preferences_with_revision().await?;
        let session_id = preferences.selected_session_id;
        let active_session = match session_id {
            Some(id) => storage.session(id).await?,
            None => None,
        };
        let mut project_directories = Vec::new();
        for project in &workbench.projects {
            let error = storage.validate_project_directory(project.id).await.err();
            project_directories.push(ProjectDirectorySnapshot {
                project_id: project.id,
                directory: project.directory.clone(),
                checked_at: now(),
                error,
            });
        }
        let frame = ServiceSnapshot {
            revision: 1,
            selection: SelectionSnapshot {
                generation: 1,
                session_id,
                committed_sequence: workbench.sequence,
            },
            workbench,
            preferences,
            preferences_revision,
            active_session,
            authentication: Vec::new(),
            catalog: None,
            catalog_busy: false,
            prepared_installations: Vec::new(),
            running_installations: Vec::new(),
            last_error: None,
            pending_removals: Vec::new(),
            project_directories,
            shutting_down: false,
        };
        let (updates, watcher) = watch::channel(Arc::new(frame.clone()));
        let (sender, receiver) = mpsc::channel(INTENT_CAPACITY);
        let (facts, events) = mpsc::channel(EVENT_CAPACITY);
        let (auth, requests) = mpsc::channel(INTENT_CAPACITY);
        let runtime = bridge::runtime(sender.clone())?;
        bridge::authentication_dispatcher(runtime.clone(), Bridge(sender.clone()), requests);
        let (owner, owner_receiver) = watch::channel(false);
        let handle = ApplicationService {
            sender: sender.clone(),
            updates: watcher,
            _owner: Arc::new(Owner(owner)),
        };
        Ok((
            Self {
                storage,
                runtime,
                catalog,
                frame,
                updates,
                sender,
                events: EventSink(facts),
                auth: Some(auth),
                auth_epoch: 0,
                auth_operations: BTreeMap::new(),
                plans: BTreeMap::new(),
                agent_edits: BTreeMap::new(),
                catalog_generation: 0,
                jobs: 0,
                tickets: 0,
                shutdown_replies: Vec::new(),
                runtime_stopped: false,
                auth_stopped: false,
                shutdown_error: None,
                failed_events: None,
                removals: BTreeMap::new(),
                #[cfg(test)]
                fail_next_view_sync: false,
                #[cfg(test)]
                fail_next_preferences_write: false,
            },
            handle,
            receiver,
            events,
            owner_receiver,
        ))
    }
    async fn serve(
        mut self,
        mut messages: mpsc::Receiver<Message>,
        mut facts: mpsc::Receiver<EventEnvelope>,
        mut owner: watch::Receiver<bool>,
    ) {
        let mut owner_gone = false;
        loop {
            let removal_deadline = self.removals.values().map(|r| r.deadline).min();
            tokio::select! {
                _=owner.changed(),if !owner_gone=>{owner_gone=true; self.begin_shutdown().await;},
                message=messages.recv()=>match message {Some(message)=>self.handle(message).await,None=>self.begin_shutdown().await},
                event=facts.recv(),if self.failed_events.is_none()=>if let Some(event)=event {
                    let mut batch=vec![event];
                    while batch.len()<MAX_EVENT_BATCH {match facts.try_recv(){Ok(event)=>batch.push(event),Err(_)=>break}}
                    match self.storage.ingest_batch(batch.clone()).await {
                        Ok(update)=>if let Err(error)=self.committed(update).await {self.record_error(error);},
                        Err(error)=>{self.failed_events=Some(batch);self.record_error(error);self.begin_shutdown().await;},
                    }
                },
                _=async {tokio::time::sleep_until(removal_deadline.unwrap_or_else(tokio::time::Instant::now)).await;},if removal_deadline.is_some()=>{},
            }
            if !self.removals.is_empty() {
                self.advance_removals().await;
            }
            if self.frame.shutting_down
                && self.runtime_stopped
                && self.auth_stopped
                && self.jobs == 0
                && self.tickets == 0
                && facts.is_empty()
            {
                let result = self.storage.storage().clone().shutdown().await;
                if let Err(error) = result {
                    self.shutdown_error.get_or_insert(error);
                }
                let result = self.shutdown_error.clone().map_or(Ok(()), Err);
                for reply in self.shutdown_replies.drain(..) {
                    let _ = reply.send(result.clone());
                }
                break;
            }
            // If disk failed, retain the undelivered batch until the backend closes. Backpressure
            // keeps later facts intact; cancellation may be unconfirmed and shutdown reports it.
            if self.failed_events.is_some() && self.runtime_stopped && self.jobs == 0 {
                self.shutdown_error.get_or_insert_with(||fail(ErrorKind::Storage,"runtime facts could not be durably committed; restart requires reconciliation"));
                let _ = self.storage.storage().clone().shutdown().await;
                for reply in self.shutdown_replies.drain(..) {
                    let _ = reply.send(Err(self.shutdown_error.clone().unwrap()));
                }
                break;
            }
        }
    }
    async fn handle(&mut self, message: Message) {
        match message {
            #[cfg(test)]
            Message::FailNextPreferencesWrite { reply } => {
                self.fail_next_preferences_write = true;
                let _ = reply.send(Ok(()));
            }
            Message::SelectProject { project_id, reply } => {
                let result = self.select_project(project_id).await;
                self.reply(reply, result);
            }
            Message::CreateSession {
                project_id,
                agent_id,
                title,
                reply,
            } => {
                let result = self.create_session(project_id, agent_id, title).await;
                self.reply(reply, result);
            }
            Message::RemoveSession { session_id, reply } => {
                self.remove(RemovalTarget::Session(session_id), reply).await
            }
            Message::RemoveProject { project_id, reply } => {
                self.remove(RemovalTarget::Project(project_id), reply).await
            }
            Message::RelocateProject {
                project_id,
                directory,
                reply,
            } => {
                let result = self.relocate(project_id, directory).await;
                self.reply(reply, result);
            }
            #[cfg(test)]
            Message::FailNextViewSync { reply } => {
                self.fail_next_view_sync = true;
                let _ = reply.send(Ok(()));
            }
            Message::Shutdown { reply } => {
                self.shutdown_replies.push(reply);
                self.begin_shutdown().await;
            }
            Message::TicketDone(result) => {
                self.tickets = self.tickets.saturating_sub(1);
                if let Err(error) = result {
                    self.record_error(error);
                }
            }
            Message::RuntimeStopped(result) => {
                self.runtime_stopped = true;
                if let Err(error) = result {
                    self.shutdown_error = Some(error.clone());
                    self.record_error(error);
                }
            }
            Message::AuthenticationStopped => self.auth_stopped = true,
            Message::AuthenticationNotice(notice) => self.auth_notice(notice),
            Message::Catalog(completion) => {
                self.jobs = self.jobs.saturating_sub(1);
                self.catalog_complete(completion).await;
            }
            Message::ResolveAgent { id, reply } => {
                let result = self.agent(id);
                let _ = reply.send(result);
            }
            Message::ResolveAttachment {
                session,
                attachment,
                reply,
            } => {
                let result = self.attachment(session, attachment).await;
                let _ = reply.send(result);
            }
            Message::Command { command, reply } => {
                let result = async {
                    self.accept()?;
                    if let Err(error) = self.command_admission(&command).await {
                        if let Some(result) = self.storage.command_result(command.id).await? {
                            return Ok(CommandReceipt {
                                revision: self.frame.revision,
                                committed_sequence: self.frame.workbench.sequence,
                                command_id: command.id,
                                result: result.status,
                            });
                        }
                        return Err(error);
                    }
                    let edited_agent = if let Command::PutAgent { agent } = &command.command {
                        Some((
                            agent.id,
                            self.agent(agent.id)
                                .ok()
                                .is_none_or(|old| old.launch != agent.launch),
                        ))
                    } else {
                        None
                    };
                    let id = command.id;
                    let directory_project =
                        if let Command::PutProject { project } = &command.command {
                            Some(project.id)
                        } else {
                            None
                        };
                    let (status, fresh) = self.commit_command(command).await?;
                    if fresh && matches!(status, CommandStatus::Applied) {
                        if let Some((id, changed_launch)) = edited_agent {
                            *self.agent_edits.entry(id).or_default() += 1;
                            if changed_launch {
                                self.frame.authentication.retain(|view| {
                                    view.scope.agent_id() != id || view.scope.session_id().is_some()
                                });
                                self.auth_operations.retain(|scope, _| {
                                    scope.agent_id() != id || scope.session_id().is_some()
                                });
                                self.publish();
                            }
                        }
                        if let Some(project_id) = directory_project {
                            let _ = self.check_directory(project_id).await;
                        }
                    }
                    Ok(CommandReceipt {
                        revision: self.frame.revision,
                        committed_sequence: self.frame.workbench.sequence,
                        command_id: id,
                        result: status,
                    })
                }
                .await;
                self.reply(reply, result);
            }
            Message::OpenProject {
                directory,
                name,
                reply,
            } => {
                let result = async {
                    self.accept()?;
                    let project = self
                        .storage
                        .open_project(CommandId::new(), directory, name, now())
                        .await?;
                    self.storage.select_project(project.id).await?;
                    self.frame.workbench = self.storage.snapshot().await?;
                    let _ = self.check_directory(project.id).await;
                    self.sync_preferences().await?;
                    self.publish();
                    Ok(project)
                }
                .await;
                self.reply(reply, result);
            }
            Message::QuerySession { session_id, reply } => {
                let result = async {
                    self.accept()?;
                    Ok(SessionReply {
                        session_id,
                        selection_generation: self.frame.selection.generation,
                        committed_sequence: self.frame.workbench.sequence,
                        snapshot: self.storage.session(session_id).await?,
                    })
                }
                .await;
                self.reply(reply, result);
            }
            Message::SelectSession { session_id, reply } => {
                let result = async {
                    self.accept()?;
                    let mut preferences = self.frame.preferences.clone();
                    preferences.selected_session_id = session_id;
                    if let Some(id) = session_id {
                        preferences.selected_project_id = Some(
                            self.storage
                                .session(id)
                                .await?
                                .ok_or_else(|| {
                                    fail(ErrorKind::NotFound, "session no longer exists")
                                })?
                                .record
                                .project_id(),
                        );
                    }
                    self.store_preferences(self.frame.preferences_revision, preferences)
                        .await?;
                    Ok(self.frame.selection.generation)
                }
                .await;
                self.reply(reply, result);
            }
            Message::Preferences {
                expected_revision,
                preferences,
                reply,
            } => {
                let result = async {
                    self.accept()?;
                    self.store_preferences(expected_revision, preferences).await
                }
                .await;
                self.reply(reply, result);
            }
            Message::DiscoverAuthentication { agent_id, reply } => {
                let result = self.accept().and_then(|_| self.discover_auth(agent_id));
                self.reply(reply, result);
            }
            Message::Authentication {
                scope,
                action,
                reply,
            } => {
                let result = async {
                    self.accept()?;
                    self.authenticate(scope, action).await
                }
                .await;
                self.reply(reply, result);
            }
            Message::RefreshAgents { policy, reply } => {
                if let Err(error) = self.accept() {
                    self.reply(reply, Err(error));
                } else {
                    let _ = self.refresh_catalog(policy, reply);
                }
            }
            Message::DiscoverAgents { reply } => {
                if let Err(error) = self.accept() {
                    self.reply(reply, Err(error));
                } else {
                    self.discover_catalog(reply);
                }
            }
            Message::PrepareInstall { registry_id, reply } => {
                self.prepare_install(registry_id, reply).await;
            }
            Message::Install { plan_id, reply } => {
                self.install(plan_id, reply);
            }
            Message::ConfigureAgent {
                agent_id,
                name,
                launch,
                reply,
            } => {
                self.configure(agent_id, name, launch, reply);
            }
        }
    }
    fn accept(&self) -> Result<(), BusinessError> {
        if self.frame.shutting_down || self.failed_events.is_some() {
            Err(closed())
        } else {
            Ok(())
        }
    }
    fn publish(&mut self) {
        self.frame.revision = self
            .frame
            .revision
            .checked_add(1)
            .expect("service revision exhausted");
        self.frame.selection.committed_sequence = self.frame.workbench.sequence;
        self.updates.send_replace(Arc::new(self.frame.clone()));
    }
    fn record_error(&mut self, error: BusinessError) {
        self.frame.last_error = Some(error);
        self.publish();
    }
    fn reply<T>(&mut self, reply: Reply<T>, result: Result<T, BusinessError>) {
        if let Err(error) = &result {
            self.record_error(error.clone());
        }
        let _ = reply.send(result);
    }
    async fn committed(&mut self, update: CommittedUpdate) -> Result<(), BusinessError> {
        self.frame.workbench = update.snapshot;
        if let Some(id) = self.frame.selection.session_id {
            if let Some(session) = update.sessions.into_iter().find(|s| s.record.id() == id) {
                self.frame.active_session = Some(session);
            }
        }
        if self
            .frame
            .selection
            .session_id
            .is_some_and(|id| !self.frame.workbench.sessions.iter().any(|s| s.id == id))
        {
            self.frame.selection.generation += 1;
            self.frame.selection.session_id = None;
            self.frame.active_session = None;
        }
        // A preference read is ancillary to an already committed business transaction.
        // Its failure cannot suppress that transaction's snapshots or owned effects.
        if let Err(error) = self.sync_preferences().await {
            self.frame.last_error = Some(error);
        }
        self.frame.authentication.retain(|view| {
            view.scope.session_id().is_none_or(|id| {
                self.frame
                    .workbench
                    .sessions
                    .iter()
                    .any(|s| s.id == id && s.connection.epoch == view.scope.epoch() && !s.closing)
            })
        });
        self.frame.project_directories.retain(|entry| {
            self.frame
                .workbench
                .projects
                .iter()
                .any(|p| p.id == entry.project_id)
        });
        if let Some(error) = update.maintenance_errors.last() {
            self.frame.last_error = Some(error.clone());
        }
        // The atomic confirmed view is visible before the first enqueue for this transaction.
        self.publish();
        let mut previous = None;
        for item in update.effects {
            debug_assert!(previous.is_none_or(|order| order < (item.sequence, item.ordinal)));
            previous = Some((item.sequence, item.ordinal));
            let ticket = self.runtime.enqueue(item.effect);
            self.tickets += 1;
            let sink = self.events.clone();
            let done = self.sender.clone();
            tokio::task::spawn_local(async move {
                let result = ticket.forward(&sink).await;
                let _ = done.send(Message::TicketDone(result)).await;
            });
        }
        Ok(())
    }
    async fn sync_preferences(&mut self) -> Result<(), BusinessError> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_view_sync) {
            return Err(fail(
                ErrorKind::Storage,
                "injected ancillary view read failure",
            ));
        }
        let (revision, preferences) = self.storage.preferences_with_revision().await?;
        let selected = preferences.selected_session_id;
        let changed = self.frame.selection.session_id != selected;
        self.frame.preferences = preferences;
        self.frame.preferences_revision = revision;
        if changed {
            self.frame.selection.generation += 1;
            self.frame.selection.session_id = selected;
            self.frame.active_session = None;
            self.frame.active_session = match selected {
                Some(id) => self.storage.session(id).await?,
                None => None,
            };
        }
        Ok(())
    }
    async fn store_preferences(
        &mut self,
        expected: u64,
        preferences: WorkspacePreferences,
    ) -> Result<u64, BusinessError> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_preferences_write) {
            return Err(fail(ErrorKind::Storage, "injected preference save failure"));
        }
        if !(240..=320).contains(&preferences.sidebar_width) || preferences.zoom_percent == 0 {
            return Err(fail(
                ErrorKind::InvalidInput,
                "invalid sidebar width or zoom",
            ));
        }
        if preferences
            .selected_project_id
            .is_some_and(|id| !self.frame.workbench.projects.iter().any(|p| p.id == id))
            || preferences
                .selected_session_id
                .is_some_and(|id| !self.frame.workbench.sessions.iter().any(|s| s.id == id))
        {
            return Err(fail(
                ErrorKind::NotFound,
                "selected project/session no longer exists",
            ));
        }
        if preferences.selected_session_id.is_some_and(|id| {
            self.frame
                .workbench
                .sessions
                .iter()
                .find(|s| s.id == id)
                .is_some_and(|s| Some(s.project_id) != preferences.selected_project_id)
        }) {
            return Err(fail(
                ErrorKind::InvalidInput,
                "selected project must match the selected session",
            ));
        }
        self.storage
            .save_preferences_if_revision(expected, preferences)
            .await?;
        self.sync_preferences().await?;
        self.publish();
        Ok(self.frame.preferences_revision)
    }
    fn agent(&self, id: AgentId) -> Result<AgentDefinition, BusinessError> {
        self.frame
            .workbench
            .agents
            .iter()
            .find(|agent| agent.id == id)
            .cloned()
            .ok_or_else(|| fail(ErrorKind::NotFound, "Agent is not in the committed catalog"))
    }
    async fn attachment(
        &self,
        session: SessionId,
        id: AttachmentId,
    ) -> Result<(Attachment, Vec<u8>), BusinessError> {
        let snapshot = self
            .storage
            .session(session)
            .await?
            .ok_or_else(|| fail(ErrorKind::NotFound, "attachment session is absent"))?;
        let attachment = snapshot
            .attachments
            .into_iter()
            .find(|a| a.id == id && a.session_id == session)
            .ok_or_else(|| {
                fail(
                    ErrorKind::NotFound,
                    "attachment does not belong to this session",
                )
            })?;
        let AttachmentStorage::Managed { blob_id } = attachment.storage else {
            return Err(fail(ErrorKind::NotReady, "attachment blob is unavailable"));
        };
        let bytes = self.storage.storage().read_attachment(blob_id).await?;
        Ok((attachment, bytes))
    }
    fn discover_auth(&mut self, id: AgentId) -> Result<AuthenticationScope, BusinessError> {
        self.agent(id)?;
        self.auth_epoch = self
            .auth_epoch
            .checked_add(1)
            .ok_or_else(|| fail(ErrorKind::Exhausted, "authentication epoch exhausted"))?;
        let scope = AuthenticationScope::agent(id, ConnectionEpoch::new(self.auth_epoch));
        let request = RequestId::new();
        self.send_auth(AuthenticationRequest::new(
            request,
            scope,
            AuthenticationAction::Discover,
        ))?;
        self.frame
            .authentication
            .retain(|v| v.scope.session_id().is_some() || v.scope.agent_id() != id);
        self.auth_operations
            .retain(|s, _| s.session_id().is_some() || s.agent_id() != id);
        self.frame.authentication.push(AuthenticationView {
            scope,
            request_id: Some(request),
            methods: Vec::new(),
            status: AuthenticationStatus::Discovering,
            interactions: Vec::new(),
        });
        self.auth_operations.insert(scope, request);
        self.publish();
        Ok(scope)
    }
    fn send_auth(&self, request: AuthenticationRequest) -> Result<(), BusinessError> {
        self.auth
            .as_ref()
            .ok_or_else(closed)?
            .try_send(request)
            .map_err(|_| {
                fail(
                    ErrorKind::Busy,
                    "authentication admission is full or closed",
                )
            })
    }
    async fn authenticate(
        &mut self,
        scope: AuthenticationScope,
        action: AuthenticationAction,
    ) -> Result<RequestId, BusinessError> {
        if let Some(id) = scope.session_id() {
            let session = self.storage.session(id).await?.ok_or_else(|| {
                fail(ErrorKind::StaleIdentity, "authentication session is absent")
            })?;
            if session.record.agent_id() != scope.agent_id()
                || session.record.connection.epoch != scope.epoch()
                || session.closing
            {
                return Err(fail(
                    ErrorKind::StaleIdentity,
                    "authentication connection was replaced",
                ));
            }
            if !self.frame.authentication.iter().any(|v| v.scope == scope) {
                if action != AuthenticationAction::Discover {
                    return Err(fail(
                        ErrorKind::NotReady,
                        "discover authentication methods first",
                    ));
                }
                self.frame.authentication.push(AuthenticationView {
                    scope,
                    request_id: None,
                    methods: Vec::new(),
                    status: AuthenticationStatus::Unknown,
                    interactions: Vec::new(),
                });
            }
        }
        let view = self
            .frame
            .authentication
            .iter()
            .find(|v| v.scope == scope)
            .ok_or_else(|| fail(ErrorKind::StaleIdentity, "authentication scope has expired"))?;
        let primary = matches!(
            action,
            AuthenticationAction::Discover | AuthenticationAction::Authenticate { .. }
        );
        match &action {
            AuthenticationAction::Authenticate { method_id } => {
                if matches!(
                    view.status,
                    AuthenticationStatus::InProgress { .. } | AuthenticationStatus::Discovering
                ) {
                    return Err(fail(
                        ErrorKind::Busy,
                        "authentication operation is already active",
                    ));
                }
                if !view.methods.iter().any(|m| {
                    m.id == *method_id
                        && m.availability == AuthenticationMethodAvailability::Available
                        && m.kind == AuthenticationMethodKind::AgentManaged
                }) {
                    return Err(fail(
                        ErrorKind::Unsupported,
                        "authentication method is not available",
                    ));
                }
            }
            AuthenticationAction::Cancel { request_id } => {
                if self.auth_operations.get(&scope) != Some(request_id) {
                    return Err(fail(
                        ErrorKind::StaleIdentity,
                        "authentication request has expired",
                    ));
                }
            }
            AuthenticationAction::Interact { interaction_id, .. } => {
                if !view.interactions.iter().any(|i| i.id == *interaction_id) {
                    return Err(fail(
                        ErrorKind::RequestExpired,
                        "authentication interaction has expired",
                    ));
                }
            }
            AuthenticationAction::Discover => {
                if matches!(
                    view.status,
                    AuthenticationStatus::InProgress { .. } | AuthenticationStatus::Discovering
                ) {
                    return Err(fail(
                        ErrorKind::Busy,
                        "authentication operation is already active",
                    ));
                }
            }
        }
        let request = RequestId::new();
        self.send_auth(AuthenticationRequest::new(request, scope, action.clone()))?;
        if primary {
            self.auth_operations.insert(scope, request);
            let view = self
                .frame
                .authentication
                .iter_mut()
                .find(|v| v.scope == scope)
                .unwrap();
            view.request_id = Some(request);
            view.interactions.clear();
            view.status = match action {
                AuthenticationAction::Authenticate { method_id } => {
                    AuthenticationStatus::InProgress { method_id }
                }
                _ => AuthenticationStatus::Discovering,
            };
            self.publish();
        }
        Ok(request)
    }
    fn auth_notice(&mut self, notice: AuthenticationNotice) {
        if notice
            .request_id
            .is_some_and(|id| self.auth_operations.get(&notice.scope) != Some(&id))
        {
            return;
        }
        let Some(view) = self
            .frame
            .authentication
            .iter_mut()
            .find(|v| v.scope == notice.scope)
        else {
            return;
        };
        match notice.update {
            AuthenticationUpdate::Methods { methods } => view.methods = methods,
            AuthenticationUpdate::Status { status } => {
                if matches!(
                    status,
                    AuthenticationStatus::Cancelled
                        | AuthenticationStatus::Authenticated
                        | AuthenticationStatus::Failed { .. }
                ) {
                    view.interactions.clear();
                }
                view.status = status;
            }
            AuthenticationUpdate::InteractionRequired { interaction } => {
                view.interactions.retain(|i| i.id != interaction.id);
                view.interactions.push(interaction);
            }
            AuthenticationUpdate::InteractionClosed { interaction_id, .. } => {
                view.interactions.retain(|i| i.id != interaction_id)
            }
        }
        self.publish();
    }
    async fn begin_shutdown(&mut self) {
        if self.frame.shutting_down {
            return;
        }
        self.frame.shutting_down = true;
        let removals = std::mem::take(&mut self.removals);
        for (_, removal) in removals {
            for reply in removal.replies {
                let _ = reply.send(Err(closed()));
            }
        }
        self.frame.pending_removals.clear();
        self.publish();
        for session in self.frame.workbench.sessions.clone() {
            let command = CommandEnvelope::new(
                CommandId::new(),
                now(),
                Scope::session(session.id, session.connection.epoch),
                Command::PauseQueue,
            );
            match self.storage.execute(command).await {
                Ok(update) => {
                    if let Err(error) = self.committed(update).await {
                        self.shutdown_error = Some(error);
                    }
                }
                Err(error) => self.shutdown_error = Some(error),
            }
        }
        self.auth.take();
        let runtime = self.runtime.clone();
        let done = self.sender.clone();
        tokio::task::spawn_local(async move {
            let result = runtime.shutdown().await;
            let _ = done.send(Message::RuntimeStopped(result)).await;
        });
    }
}
fn fail(kind: ErrorKind, message: &str) -> BusinessError {
    BusinessError::new(kind, message, RetryAdvice::AfterCorrection)
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
