use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};
use tokio::sync::{mpsc, oneshot, watch};
use workbench_core::*;

/// Cross-thread UI reply only. Non-Send core/Storage futures stay on the backend LocalSet.
pub type ServiceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BusinessError>> + Send + 'a>>;

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ServiceOptions {
    pub data_root: Option<PathBuf>,
}
impl ServiceOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_data_root(mut self, root: PathBuf) -> Self {
        self.data_root = Some(root);
        self
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SelectionSnapshot {
    pub generation: u64,
    pub session_id: Option<SessionId>,
    pub committed_sequence: u64,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SessionReply {
    pub session_id: SessionId,
    pub selection_generation: u64,
    pub committed_sequence: u64,
    pub snapshot: Option<SessionSnapshot>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AuthenticationView {
    pub scope: AuthenticationScope,
    pub request_id: Option<RequestId>,
    pub methods: Vec<AuthenticationMethod>,
    pub status: AuthenticationStatus,
    pub interactions: Vec<AuthenticationInteraction>,
}

/// Atomic read projection of confirmed service state. Never a mutable business aggregate.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ServiceSnapshot {
    pub revision: u64,
    pub workbench: WorkbenchSnapshot,
    pub preferences: WorkspacePreferences,
    pub preferences_revision: u64,
    pub selection: SelectionSnapshot,
    pub active_session: Option<SessionSnapshot>,
    pub authentication: Vec<AuthenticationView>,
    pub catalog: Option<agent_catalog::CatalogSnapshot>,
    pub catalog_busy: bool,
    pub prepared_installations: Vec<agent_catalog::InstallPlan>,
    pub running_installations: Vec<agent_catalog::InstallPlanId>,
    pub last_error: Option<BusinessError>,
    pub shutting_down: bool,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CommandReceipt {
    pub revision: u64,
    pub committed_sequence: u64,
    pub command_id: CommandId,
    pub result: CommandStatus,
}

/// One app-level subscription. Keep it outside settings/page/view lifetime.
pub struct ServiceSubscription {
    pub(crate) receiver: watch::Receiver<Arc<ServiceSnapshot>>,
}
impl ServiceSubscription {
    pub fn current(&self) -> Arc<ServiceSnapshot> {
        self.receiver.borrow().clone()
    }
    pub fn changed(&mut self) -> ServiceFuture<'_, Arc<ServiceSnapshot>> {
        Box::pin(async move {
            self.receiver.changed().await.map_err(|_| closed())?;
            Ok(self.receiver.borrow_and_update().clone())
        })
    }
}

#[derive(Clone)]
pub struct ApplicationService {
    pub(crate) sender: mpsc::Sender<crate::actor::Message>,
    pub(crate) updates: watch::Receiver<Arc<ServiceSnapshot>>,
    pub(crate) _owner: Arc<crate::actor::Owner>,
}
impl ApplicationService {
    pub fn start(options: ServiceOptions) -> ServiceFuture<'static, Self> {
        crate::actor::start(options)
    }
    pub fn snapshot(&self) -> Arc<ServiceSnapshot> {
        self.updates.borrow().clone()
    }
    pub fn subscribe(&self) -> ServiceSubscription {
        ServiceSubscription {
            receiver: self.updates.clone(),
        }
    }
    pub fn execute(&self, command: CommandEnvelope) -> ServiceFuture<'static, CommandReceipt> {
        self.request(move |reply| crate::actor::Message::Command { command, reply })
    }
    pub fn open_project(
        &self,
        directory: PathBuf,
        name: String,
    ) -> ServiceFuture<'static, Project> {
        self.request(move |reply| crate::actor::Message::OpenProject {
            directory,
            name,
            reply,
        })
    }
    pub fn query_session(&self, session_id: SessionId) -> ServiceFuture<'static, SessionReply> {
        self.request(move |reply| crate::actor::Message::QuerySession { session_id, reply })
    }
    pub fn select_session(&self, session_id: Option<SessionId>) -> ServiceFuture<'static, u64> {
        self.request(move |reply| crate::actor::Message::SelectSession { session_id, reply })
    }
    pub fn save_preferences(
        &self,
        expected_revision: u64,
        preferences: WorkspacePreferences,
    ) -> ServiceFuture<'static, u64> {
        self.request(move |reply| crate::actor::Message::Preferences {
            expected_revision,
            preferences,
            reply,
        })
    }
    pub fn discover_authentication(
        &self,
        agent_id: AgentId,
    ) -> ServiceFuture<'static, AuthenticationScope> {
        self.request(move |reply| crate::actor::Message::DiscoverAuthentication { agent_id, reply })
    }
    pub fn refresh_agents(&self) -> ServiceFuture<'static, ()> {
        self.request(|reply| crate::actor::Message::RefreshAgents {
            policy: agent_catalog::RefreshPolicy::Revalidate,
            reply,
        })
    }
    pub fn discover_agents(&self) -> ServiceFuture<'static, ()> {
        self.request(|reply| crate::actor::Message::DiscoverAgents { reply })
    }
    pub fn prepare_install(
        &self,
        registry_id: String,
    ) -> ServiceFuture<'static, agent_catalog::InstallPlan> {
        self.request(move |reply| crate::actor::Message::PrepareInstall { registry_id, reply })
    }
    /// Only a plan returned by this running service can be explicitly confirmed, once.
    pub fn install(
        &self,
        plan_id: agent_catalog::InstallPlanId,
    ) -> ServiceFuture<'static, AgentId> {
        self.request(move |reply| crate::actor::Message::Install { plan_id, reply })
    }
    pub fn configure_agent(
        &self,
        agent_id: Option<AgentId>,
        name: String,
        launch: agent_catalog::ManualLaunch,
    ) -> ServiceFuture<'static, AgentId> {
        self.request(move |reply| crate::actor::Message::ConfigureAgent {
            agent_id,
            name,
            launch,
            reply,
        })
    }
    pub fn authentication(
        &self,
        scope: AuthenticationScope,
        action: AuthenticationAction,
    ) -> ServiceFuture<'static, RequestId> {
        self.request(move |reply| crate::actor::Message::Authentication {
            scope,
            action,
            reply,
        })
    }
    pub fn shutdown(&self) -> ServiceFuture<'static, ()> {
        self.request(|reply| crate::actor::Message::Shutdown { reply })
    }
    pub(crate) fn request<T: Send + 'static>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, BusinessError>>) -> crate::actor::Message,
    ) -> ServiceFuture<'static, T> {
        let (reply, response) = oneshot::channel();
        let message = build(reply);
        let admitted = self.sender.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => BusinessError::new(
                ErrorKind::Busy,
                "application command queue is full; intent was not admitted",
                RetryAdvice::AfterCorrection,
            ),
            mpsc::error::TrySendError::Closed(_) => closed(),
        });
        Box::pin(async move {
            admitted?;
            response.await.map_err(|_| closed())?
        })
    }
}
pub(crate) fn closed() -> BusinessError {
    BusinessError::new(
        ErrorKind::NotReady,
        "application background service is closed",
        RetryAdvice::AfterReconnect,
    )
}
