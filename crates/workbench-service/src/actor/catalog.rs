use super::*;
use agent_catalog::{
    CatalogError, CatalogErrorKind, CatalogSnapshot, InstallOutcome, LocalDiscovery,
    ManualValidation, RuntimeCompatibility,
};

pub(crate) enum Completion {
    Discovery {
        generation: u64,
        base: Vec<AgentDefinition>,
        snapshot: Option<CatalogSnapshot>,
        result: Result<(LocalDiscovery, Vec<AgentDefinition>), BusinessError>,
        reply: Reply<()>,
    },
    Prepared {
        edit: u64,
        result: Result<InstallPlan, BusinessError>,
        reply: Reply<InstallPlan>,
    },
    Installed {
        plan: InstallPlan,
        edit: u64,
        result: Result<InstallOutcome, BusinessError>,
        reply: Reply<AgentId>,
    },
    Configured {
        id: AgentId,
        explicit: bool,
        edit: u64,
        result: Result<ManualValidation, BusinessError>,
        reply: Reply<AgentId>,
    },
}
pub(super) fn error(error: CatalogError) -> BusinessError {
    if error.kind == CatalogErrorKind::CleanupUnconfirmed {
        return BusinessError::new(
            ErrorKind::DeliveryUnknown,
            error.message,
            RetryAdvice::ReconcileFirst,
        );
    }
    let kind = match error.kind {
        CatalogErrorKind::Network | CatalogErrorKind::Timeout => ErrorKind::Transport,
        CatalogErrorKind::Cache | CatalogErrorKind::Io => ErrorKind::Storage,
        CatalogErrorKind::NotFound => ErrorKind::NotFound,
        CatalogErrorKind::Unsupported => ErrorKind::Unsupported,
        CatalogErrorKind::InvalidLaunch
        | CatalogErrorKind::InvalidPlan
        | CatalogErrorKind::InvalidRegistry => ErrorKind::InvalidInput,
        CatalogErrorKind::MissingTool => ErrorKind::NotReady,
        CatalogErrorKind::InstallInProgress => ErrorKind::Busy,
        CatalogErrorKind::InstallFailed => ErrorKind::RemoteExecution,
        CatalogErrorKind::CleanupUnconfirmed => ErrorKind::DeliveryUnknown,
    };
    BusinessError::new(
        kind,
        error.message,
        if error.retryable {
            RetryAdvice::AfterCorrection
        } else {
            RetryAdvice::Never
        },
    )
}
impl Actor {
    pub(super) fn refresh_catalog(
        &mut self,
        policy: RefreshPolicy,
        reply: Reply<()>,
    ) -> Result<(), BusinessError> {
        self.catalog_generation += 1;
        let generation = self.catalog_generation;
        let catalog = self.catalog.clone();
        let base = self.frame.workbench.agents.clone();
        let done = self.sender.clone();
        self.jobs += 1;
        self.frame.catalog_busy = true;
        self.publish();
        tokio::task::spawn_local(async move {
            let (snapshot, result) = match catalog.refresh(base.clone(), policy).await {
                Ok(snapshot) => {
                    let result = adopt(&catalog, &base, snapshot.local.clone()).await;
                    (Some(snapshot), result)
                }
                Err(problem) => (None, Err(error(problem))),
            };
            let _ = done
                .send(Message::Catalog(Completion::Discovery {
                    generation,
                    base,
                    snapshot,
                    result,
                    reply,
                }))
                .await;
        });
        Ok(())
    }
    pub(super) fn discover_catalog(&mut self, reply: Reply<()>) {
        self.catalog_generation += 1;
        let generation = self.catalog_generation;
        let catalog = self.catalog.clone();
        let base = self.frame.workbench.agents.clone();
        let done = self.sender.clone();
        self.jobs += 1;
        self.frame.catalog_busy = true;
        self.publish();
        tokio::task::spawn_local(async move {
            let result = match catalog.discover(base.clone()).await {
                Ok(local) => adopt(&catalog, &base, local).await,
                Err(problem) => Err(error(problem)),
            };
            let _ = done
                .send(Message::Catalog(Completion::Discovery {
                    generation,
                    base,
                    snapshot: None,
                    result,
                    reply,
                }))
                .await;
        });
    }
    pub(super) async fn prepare_install(&mut self, registry_id: String, reply: Reply<InstallPlan>) {
        let result = async {
            self.accept()?;
            let id = if let Some(agent) = self.frame.workbench.agents.iter().find(
                |a| matches!(&a.source,AgentSource::Registry{registry_id:key} if *key==registry_id),
            ) {
                agent.id
            } else {
                let entry = self
                    .frame
                    .catalog
                    .as_ref()
                    .and_then(|c| c.available.iter().find(|a| a.registry_id == registry_id))
                    .ok_or_else(|| {
                        fail(
                            ErrorKind::NotReady,
                            "refresh Registry before preparing installation",
                        )
                    })?;
                let mut definition = AgentDefinition::new(
                    AgentId::new(),
                    entry.name.clone(),
                    AgentSource::Registry {
                        registry_id: registry_id.clone(),
                    },
                );
                definition.discovery = AgentDiscovery::NotInstalled;
                let id = definition.id;
                self.put_agent(definition).await?;
                id
            };
            if self
                .frame
                .running_installations
                .iter()
                .any(|plan| self.plans.get(plan).is_some_and(|(p, _)| p.agent_id == id))
            {
                return Err(fail(
                    ErrorKind::Busy,
                    "this Agent installation is already running",
                ));
            }
            Ok((id, *self.agent_edits.get(&id).unwrap_or(&0)))
        }
        .await;
        let (id, edit) = match result {
            Ok(value) => value,
            Err(error) => {
                self.reply(reply, Err(error));
                return;
            }
        };
        self.jobs += 1;
        let catalog = self.catalog.clone();
        let done = self.sender.clone();
        tokio::task::spawn_local(async move {
            let result = catalog
                .prepare_install(&registry_id, id)
                .await
                .map_err(error);
            let _ = done
                .send(Message::Catalog(Completion::Prepared {
                    edit,
                    result,
                    reply,
                }))
                .await;
        });
    }
    pub(super) fn install(&mut self, plan_id: InstallPlanId, reply: Reply<AgentId>) {
        let result = async_install_check(self, plan_id);
        let (plan, edit) = match result {
            Ok(value) => value,
            Err(error) => {
                self.reply(reply, Err(error));
                return;
            }
        };
        self.frame
            .prepared_installations
            .retain(|p| p.id != plan_id);
        self.frame.running_installations.push(plan_id);
        self.jobs += 1;
        self.publish();
        let catalog = self.catalog.clone();
        let done = self.sender.clone();
        tokio::task::spawn_local(async move {
            let result = catalog.confirm_install(plan_id).await.map_err(error);
            let _ = done
                .send(Message::Catalog(Completion::Installed {
                    plan,
                    edit,
                    result,
                    reply,
                }))
                .await;
        });
    }
    pub(super) fn configure(
        &mut self,
        id: Option<AgentId>,
        name: String,
        launch: ManualLaunch,
        reply: Reply<AgentId>,
    ) {
        let explicit = id.is_some();
        let result = (|| {
            self.accept()?;
            if name.trim().is_empty() {
                return Err(fail(ErrorKind::InvalidInput, "Agent name is empty"));
            }
            let id = match id {
                Some(id) => {
                    self.agent(id)?;
                    id
                }
                None => self
                    .frame
                    .workbench
                    .agents
                    .iter()
                    .find(|a| {
                        a.launch.as_ref().is_some_and(|v| {
                            v.executable == launch.executable && v.args == launch.args
                        })
                    })
                    .map(|a| a.id)
                    .unwrap_or_default(),
            };
            if self
                .frame
                .running_installations
                .iter()
                .any(|plan| self.plans.get(plan).is_some_and(|(p, _)| p.agent_id == id))
            {
                return Err(fail(
                    ErrorKind::Busy,
                    "installation must finish before editing this Agent",
                ));
            }
            let edit = self.agent_edits.entry(id).or_default();
            *edit += 1;
            Ok((id, *edit))
        })();
        let (id, edit) = match result {
            Ok(value) => value,
            Err(error) => {
                self.reply(reply, Err(error));
                return;
            }
        };
        self.jobs += 1;
        let catalog = self.catalog.clone();
        let done = self.sender.clone();
        tokio::task::spawn_local(async move {
            let result = catalog
                .validate_manual(id, &name, launch)
                .await
                .map_err(error);
            let _ = done
                .send(Message::Catalog(Completion::Configured {
                    id,
                    explicit,
                    edit,
                    result,
                    reply,
                }))
                .await;
        });
    }
    pub(super) async fn catalog_complete(&mut self, completion: Completion) {
        match completion {
            Completion::Discovery {
                generation,
                base,
                snapshot,
                result,
                reply,
            } => {
                let result = async {
                    if generation != self.catalog_generation {
                        return Err(fail(
                            ErrorKind::StaleIdentity,
                            "a newer catalog operation superseded this result",
                        ));
                    }
                    self.frame.catalog_busy = false;
                    let (local, adopted) = result?;
                    // Discovery may refresh evidence only when that same committed launch is still
                    // current. A slower process probe cannot overwrite a settings edit.
                    for definition in &local.configured {
                        if let (Some(before), Ok(current)) = (
                            base.iter().find(|a| a.id == definition.id),
                            self.agent(definition.id),
                        ) {
                            if current == *before && current != *definition {
                                self.put_agent(definition.clone()).await?;
                            }
                        }
                    }
                    for definition in adopted {
                        if let Ok(current) = self.agent(definition.id) {
                            if current.launch.is_none()
                                && base.iter().find(|a| a.id == current.id) == Some(&current)
                            {
                                self.put_agent(definition).await?;
                            }
                            continue;
                        }
                        if self
                            .frame
                            .workbench
                            .agents
                            .iter()
                            .any(|a| same_identity(a, &definition))
                        {
                            continue;
                        }
                        self.put_agent(definition).await?;
                    }
                    match snapshot {
                        Some(mut snapshot) => {
                            snapshot.local = local;
                            self.frame.catalog = Some(snapshot);
                        }
                        None => {
                            if let Some(snapshot) = &mut self.frame.catalog {
                                snapshot.local = local;
                            } else {
                                // Local discovery is independent of network availability; Registry
                                // evidence remains explicitly unavailable until a deliberate refresh.
                                self.frame.catalog =
                                    Some(CatalogSnapshot::local_only(local, now()));
                            }
                        }
                    }
                    self.sync_catalog_configured();
                    self.publish();
                    Ok(())
                }
                .await;
                self.reply(reply, result);
            }
            Completion::Prepared {
                edit,
                result,
                reply,
            } => {
                let result = result.and_then(|plan| {
                    if self.frame.shutting_down
                        || *self.agent_edits.get(&plan.agent_id).unwrap_or(&0) != edit
                    {
                        return Err(fail(
                            ErrorKind::StaleIdentity,
                            "Agent changed while preparing installation",
                        ));
                    }
                    self.plans.insert(plan.id, (plan.clone(), edit));
                    self.frame.prepared_installations.push(plan.clone());
                    self.publish();
                    Ok(plan)
                });
                self.reply(reply, result);
            }
            Completion::Installed {
                plan,
                edit,
                result,
                reply,
            } => {
                self.frame.running_installations.retain(|id| *id != plan.id);
                self.plans.remove(&plan.id);
                let result = async {
                    let outcome = result?;
                    if outcome.plan_id != plan.id
                        || outcome.definition.id != plan.agent_id
                        || *self.agent_edits.get(&plan.agent_id).unwrap_or(&0) != edit
                    {
                        return Err(fail(
                            ErrorKind::StaleIdentity,
                            "installation result no longer matches its committed Agent/plan",
                        ));
                    }
                    let id = outcome.definition.id;
                    self.put_agent(outcome.definition).await?;
                    self.sync_catalog_configured();
                    if !self.frame.shutting_down {
                        if let Err(error) = self.discover_auth(id) {
                            self.record_error(error);
                        }
                    }
                    self.publish();
                    Ok(id)
                }
                .await;
                self.reply(reply, result);
            }
            Completion::Configured {
                id,
                explicit,
                edit,
                result,
                reply,
            } => {
                let result = async {
                    if *self.agent_edits.get(&id).unwrap_or(&0) != edit {
                        return Err(fail(
                            ErrorKind::StaleIdentity,
                            "a newer manual configuration superseded this result",
                        ));
                    }
                    let validation = result?;
                    let mut definition = validation.definition;
                    // Validation canonicalizes the launch. Reuse its persisted identity even
                    // when the user entered an equivalent relative/case-normalized path.
                    let id = if explicit {
                        id
                    } else {
                        self.frame
                            .workbench
                            .agents
                            .iter()
                            .find(|a| a.launch.is_some() && a.launch == definition.launch)
                            .map(|a| a.id)
                            .unwrap_or(id)
                    };
                    definition.id = id;
                    if let Ok(current) = self.agent(id) {
                        definition.source = current.source;
                        definition.distributions = current.distributions;
                    }
                    self.put_agent(definition).await?;
                    self.sync_catalog_configured();
                    if !self.frame.shutting_down {
                        if let Err(error) = self.discover_auth(id) {
                            self.record_error(error);
                        }
                    }
                    self.publish();
                    Ok(id)
                }
                .await;
                self.reply(reply, result);
            }
        }
    }
    async fn put_agent(&mut self, definition: AgentDefinition) -> Result<(), BusinessError> {
        let id = definition.id;
        let changed_launch = self
            .agent(id)
            .ok()
            .is_none_or(|previous| previous.launch != definition.launch);
        let update = self
            .storage
            .execute(CommandEnvelope::new(
                CommandId::new(),
                now(),
                Scope::default(),
                Command::PutAgent { agent: definition },
            ))
            .await?;
        let rejected = update.decisions.iter().find_map(|d| {
            if let ApplyDisposition::Rejected(error) = &d.disposition {
                Some(error.clone())
            } else {
                None
            }
        });
        self.committed(update).await?;
        if let Some(error) = rejected {
            return Err(error);
        }
        if changed_launch {
            *self.agent_edits.entry(id).or_default() += 1;
            self.frame
                .authentication
                .retain(|view| view.scope.agent_id() != id || view.scope.session_id().is_some());
            self.auth_operations
                .retain(|scope, _| scope.agent_id() != id || scope.session_id().is_some());
        }
        Ok(())
    }
    fn sync_catalog_configured(&mut self) {
        if let Some(catalog) = &mut self.frame.catalog {
            catalog.local.configured = self.frame.workbench.agents.clone();
        }
    }
}
fn async_install_check(
    actor: &Actor,
    id: InstallPlanId,
) -> Result<(InstallPlan, u64), BusinessError> {
    actor.accept()?;
    if !actor
        .frame
        .prepared_installations
        .iter()
        .any(|p| p.id == id)
    {
        return Err(fail(
            ErrorKind::StaleIdentity,
            "installation plan is absent or already consumed",
        ));
    }
    let (plan, edit) = actor
        .plans
        .get(&id)
        .cloned()
        .ok_or_else(|| fail(ErrorKind::StaleIdentity, "installation plan expired"))?;
    if *actor.agent_edits.get(&plan.agent_id).unwrap_or(&0) != edit {
        return Err(fail(
            ErrorKind::StaleIdentity,
            "prepare a new plan after changing this Agent",
        ));
    }
    if actor.frame.running_installations.iter().any(|other| {
        actor
            .plans
            .get(other)
            .is_some_and(|(p, _)| p.agent_id == plan.agent_id)
    }) {
        return Err(fail(ErrorKind::Busy, "installation is already running"));
    }
    Ok((plan, edit))
}
async fn adopt(
    catalog: &AgentCatalog,
    base: &[AgentDefinition],
    local: LocalDiscovery,
) -> Result<(LocalDiscovery, Vec<AgentDefinition>), BusinessError> {
    let mut adopted = Vec::new();
    for candidate in &local.candidates {
        if candidate.compatibility != RuntimeCompatibility::SupportedCodex {
            continue;
        }
        let existing=base.iter().find(|a|a.launch.as_ref()==Some(&candidate.launch)||candidate.registry_id.as_ref().is_some_and(|key|matches!(&a.source,AgentSource::Registry{registry_id} if registry_id==key)));
        if existing.is_some_and(|agent| agent.launch.is_some()) {
            continue;
        }
        let mut definition = catalog
            .adopt_candidate(
                &candidate.key,
                existing.map(|agent| agent.id).unwrap_or_default(),
            )
            .await
            .map_err(error)?;
        if let Some(registry_id) = &candidate.registry_id {
            definition.source = AgentSource::Registry {
                registry_id: registry_id.clone(),
            };
        }
        adopted.push(definition);
    }
    Ok((local, adopted))
}
fn same_identity(left: &AgentDefinition, right: &AgentDefinition) -> bool {
    left.id == right.id
        || (left.launch.is_some() && left.launch == right.launch)
        || matches!((&left.source,&right.source),(AgentSource::Registry{registry_id:a},AgentSource::Registry{registry_id:b}) if a==b)
}
