//! Agent installation metadata and local discovery. Persistent Agent IDs belong to the service.
mod api;
pub use api::*;
mod discovery;
mod install;
mod registry;
#[cfg(test)]
mod tests;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workbench_core::{AgentDefinition, AgentId, Timestamp};

pub(crate) struct State {
    registry: Option<registry::RegistryData>,
    local: LocalDiscovery,
    plans: BTreeMap<InstallPlanId, install::PlanRecord>,
}
struct Inner {
    root: PathBuf,
    transport: Arc<dyn registry::RegistryTransport>,
    state: Mutex<State>,
    refresh: tokio::sync::Mutex<()>,
    installer: Arc<dyn install::Installer>,
}
/// Cloneable catalog handle. Persistent definitions/AgentId allocation stay in WorkbenchService.
#[derive(Clone)]
pub struct AgentCatalog {
    inner: Arc<Inner>,
}
impl AgentCatalog {
    /// Uses an absolute application-owned data directory; does not install or discover on construction.
    pub fn new(data_root: PathBuf) -> Result<Self, CatalogError> {
        if !data_root.is_absolute() {
            return Err(failure(
                CatalogErrorKind::Io,
                "application data root must be absolute",
                false,
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                failure(
                    CatalogErrorKind::Network,
                    "cannot initialize Registry HTTPS client",
                    true,
                )
            })?;
        Ok(Self {
            inner: Arc::new(Inner {
                root: data_root,
                transport: Arc::new(registry::HttpRegistry(client)),
                state: Mutex::new(State {
                    registry: None,
                    local: LocalDiscovery::default(),
                    plans: BTreeMap::new(),
                }),
                refresh: tokio::sync::Mutex::new(()),
                installer: Arc::new(install::NpmInstaller),
            }),
        })
    }
    /// Fetches official metadata and validates existing local definitions without replacing their IDs/launches.
    pub async fn refresh(
        &self,
        configured: Vec<AgentDefinition>,
        policy: RefreshPolicy,
    ) -> Result<CatalogSnapshot, CatalogError> {
        let _refresh = self.inner.refresh.lock().await;
        let (registry, local) = tokio::join!(
            registry::load(self.inner.transport.as_ref(), &self.inner.root, policy),
            discovery::discover(&self.inner.root, configured)
        );
        let local = local?;
        let snapshot = CatalogSnapshot {
            registry: registry.evidence.clone(),
            available: registry.agents.clone(),
            local: local.clone(),
        };
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?;
        state.registry = Some(registry);
        state.local = local;
        Ok(snapshot)
    }
    /// Local discovery only. Package presence is not an ACP connection or authentication result.
    pub async fn discover(
        &self,
        configured: Vec<AgentDefinition>,
    ) -> Result<LocalDiscovery, CatalogError> {
        let local = discovery::discover(&self.inner.root, configured).await?;
        self.inner
            .state
            .lock()
            .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?
            .local = local.clone();
        Ok(local)
    }
    /// Returns an immutable, process-local plan. No target directories or installer processes are created.
    pub async fn prepare_install(
        &self,
        registry_id: &str,
        agent_id: AgentId,
    ) -> Result<InstallPlan, CatalogError> {
        install::prepare(self, registry_id, agent_id).await
    }
    /// Executes only a previously reviewed plan owned by this catalog. The service owns this future
    /// through completion, independently of a settings window. A failed plan may be explicitly retried.
    /// Success is installation/version validation; the service must still connect through Runtime.
    pub async fn confirm_install(&self, id: InstallPlanId) -> Result<InstallOutcome, CatalogError> {
        install::confirm(self, id).await
    }
    /// Transient execution state, not a persisted Agent business state.
    pub fn plan_state(&self, id: InstallPlanId) -> Option<InstallState> {
        self.inner
            .state
            .lock()
            .ok()?
            .plans
            .get(&id)
            .map(|record| record.state.clone())
    }
    /// Validates non-secret launch prerequisites and compatibility; does not launch an Agent session.
    pub async fn validate_manual(
        &self,
        agent_id: AgentId,
        name: &str,
        launch: ManualLaunch,
    ) -> Result<ManualValidation, CatalogError> {
        discovery::manual(agent_id, name, launch).await
    }
    /// Revalidates a discovered candidate using an ID allocated/reused by the service.
    pub async fn adopt_candidate(
        &self,
        key: &str,
        agent_id: AgentId,
    ) -> Result<AgentDefinition, CatalogError> {
        let candidate = self
            .inner
            .state
            .lock()
            .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?
            .local
            .candidates
            .iter()
            .find(|candidate| candidate.key == key)
            .cloned()
            .ok_or_else(|| {
                failure(
                    CatalogErrorKind::NotFound,
                    "local candidate is no longer available; refresh discovery",
                    true,
                )
            })?;
        Ok(self
            .validate_manual(
                agent_id,
                &candidate.name,
                ManualLaunch::new(candidate.launch.executable, candidate.launch.args),
            )
            .await?
            .definition)
    }
}
pub(crate) fn now() -> Timestamp {
    Timestamp::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64,
    )
}
pub(crate) fn failure(
    kind: CatalogErrorKind,
    message: impl Into<String>,
    retryable: bool,
) -> CatalogError {
    CatalogError {
        kind,
        message: message.into(),
        retryable,
    }
}
