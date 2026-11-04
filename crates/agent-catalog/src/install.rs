use crate::*;
use futures::future::BoxFuture;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{fs, io::AsyncReadExt};
use workbench_core::*;

const OWNER_FILE: &str = ".workbench-agent-install.json";
const TIMEOUT: Duration = Duration::from_secs(600);
pub(crate) struct PlanRecord {
    pub plan: InstallPlan,
    pub state: InstallState,
    pub result: Option<AgentDefinition>,
}
pub(crate) trait Installer: Send + Sync {
    fn run(&self, command: InstallCommand) -> BoxFuture<'_, Result<(), CatalogError>>;
}
pub(crate) struct NpmInstaller;
impl Installer for NpmInstaller {
    fn run(&self, command: InstallCommand) -> BoxFuture<'_, Result<(), CatalogError>> {
        Box::pin(async move {
            let mut wrapped = CommandWrap::with_new(&command.program, |process| {
                process
                    .args(&command.args)
                    .current_dir(&command.working_directory)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
            });
            wrapped.wrap(KillOnDrop);
            #[cfg(windows)]
            {
                let mut flags = process_wrap::tokio::CreationFlags(Default::default());
                flags.0.0 = 0x08000000;
                wrapped.wrap(flags).wrap(process_wrap::tokio::JobObject);
            }
            #[cfg(unix)]
            {
                wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
            }
            let mut child = wrapped.spawn().map_err(|_| {
                failure(
                    CatalogErrorKind::InstallFailed,
                    "native npm process could not start",
                    true,
                )
            })?;
            let Some(mut stderr) = child.stderr().take() else {
                return Err(if bounded_cleanup(child.as_mut()).await {
                    failure(
                        CatalogErrorKind::InstallFailed,
                        "npm diagnostic stream unavailable; owned processes released",
                        true,
                    )
                } else {
                    unconfirmed("npm diagnostic stream unavailable")
                });
            };
            let mut diagnostics = tokio::spawn(async move {
                let mut retained = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let n = stderr.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    if retained.len() < 32768 {
                        retained.extend_from_slice(&buffer[..n.min(32768 - retained.len())]);
                    }
                }
                retained
            });
            // The local process-wrap patch retains parent status separately from Job-empty
            // proof, so a cancelled wait can safely resume after termination.
            let status = match tokio::time::timeout(TIMEOUT, child.wait()).await {
                Ok(Ok(status)) => status,
                result => {
                    let timed_out = result.is_err();
                    let cleaned = bounded_cleanup(child.as_mut()).await;
                    diagnostics.abort();
                    let reason = if timed_out {
                        "installation timed out"
                    } else {
                        "installer wait failed"
                    };
                    return Err(if cleaned {
                        failure(
                            if timed_out {
                                CatalogErrorKind::Timeout
                            } else {
                                CatalogErrorKind::InstallFailed
                            },
                            format!(
                                "{reason}; owned processes released; retry or configure manually"
                            ),
                            true,
                        )
                    } else {
                        unconfirmed(reason)
                    });
                }
            };
            let bytes = match tokio::time::timeout(Duration::from_secs(2), &mut diagnostics).await {
                Ok(Ok(bytes)) => bytes,
                _ => {
                    diagnostics.abort();
                    Vec::new()
                }
            };
            if !status.success() {
                let text = String::from_utf8_lossy(&bytes);
                let code = text.lines().find_map(|line| {
                    line.split_once("error code ").and_then(|(_, code)| {
                        let code = code.trim();
                        if !code.is_empty()
                            && code.len() <= 40
                            && code
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        {
                            Some(code.to_owned())
                        } else {
                            None
                        }
                    })
                });
                return Err(failure(
                    CatalogErrorKind::InstallFailed,
                    format!(
                        "npm installation failed{}",
                        code.map(|code| format!(" ({code})")).unwrap_or_default()
                    ),
                    true,
                ));
            }
            Ok(())
        })
    }
}
async fn bounded_cleanup(child: &mut dyn ChildWrapper) -> bool {
    let termination = child.start_kill();
    let wait = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    match (termination, wait) {
        // A proved empty Job suffices even if termination raced its natural exit.
        (_, Ok(Ok(_))) => true,
        _ => false,
    }
}
fn unconfirmed(reason: &str) -> CatalogError {
    failure(
        CatalogErrorKind::CleanupUnconfirmed,
        format!(
            "{reason}; termination and bounded cleanup attempted, owned-tree release remains unconfirmed; this installation target stays locked"
        ),
        false,
    )
}
pub(crate) async fn prepare(
    catalog: &AgentCatalog,
    id: &str,
    agent_id: AgentId,
) -> Result<InstallPlan, CatalogError> {
    let registry = catalog
        .inner
        .state
        .lock()
        .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?
        .registry
        .clone()
        .ok_or_else(|| {
            failure(
                CatalogErrorKind::NotFound,
                "refresh the official Registry before preparing installation",
                true,
            )
        })?;
    let entry = registry
        .agents
        .iter()
        .find(|agent| agent.registry_id == id)
        .ok_or_else(|| {
            failure(
                CatalogErrorKind::NotFound,
                "Agent is absent from the loaded Registry",
                true,
            )
        })?;
    if entry.installation != Installability::Supported {
        return Err(failure(
            CatalogErrorKind::Unsupported,
            "this Registry distribution is not supported by the current Runtime",
            false,
        ));
    }
    let tools = discovery::tools().await;
    let node = tools.node.ok_or_else(|| {
        failure(
            CatalogErrorKind::MissingTool,
            "install or manually select native Node before installing the adapter",
            false,
        )
    })?;
    let npm = tools.npm.ok_or_else(|| {
        failure(
            CatalogErrorKind::MissingTool,
            "a verified native Node/npm CLI entry is required",
            false,
        )
    })?;
    let major = node
        .version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    if major < 20 {
        return Err(failure(
            CatalogErrorKind::MissingTool,
            "this adapter's dependencies require Node 20 or newer; the verified environment uses Node 24.14.0",
            false,
        ));
    }
    let target = catalog
        .inner
        .root
        .join("agents/codex-acp/2.5.0-codex-0.166.0");
    validate_existing_target(&catalog.inner.root, &target).await?;
    let mut args = npm.args.clone();
    args.extend([
        "install".into(),
        "--prefix".into(),
        target.to_string_lossy().into(),
        "--save-exact".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--registry=https://registry.npmjs.org/".into(),
        format!("--userconfig={}", target.join(".npmrc").to_string_lossy()),
        format!(
            "--globalconfig={}",
            target.join(".npmrc-global").to_string_lossy()
        ),
        "@agentclientprotocol/codex-acp@2.5.0".into(),
        "@openai/codex@0.166.0".into(),
    ]);
    let plan=InstallPlan {id:InstallPlanId(RequestId::new()),agent_id,registry_id:id.into(),source_url:registry.evidence.source_url,
        adapter_version:CODEX_ADAPTER_VERSION.into(),native_agent_version:CODEX_NATIVE_VERSION.into(),target_directory:target.clone(),command:InstallCommand {program:npm.executable,args,working_directory:target},
        prerequisites:vec![InstallPrerequisite {name:"Node".into(),satisfied:true,detail:format!("native Node {} detected",node.version)},InstallPrerequisite {name:"npm".into(),satisfied:true,detail:format!("npm {} through verified npm-cli.js",npm.version)},InstallPrerequisite {name:"isolated exact versions".into(),satisfied:true,detail:"writes an application-owned manifest pinning adapter 2.5.0 and Codex 0.166.0; public npm source; no global installation".into()},InstallPrerequisite {name:"bounded network".into(),satisfied:true,detail:"10 minute total timeout; 60 second requests; no automatic fetch retries".into()}],created_at:now(),native_agent_source_url:"https://registry.npmjs.org/@openai/codex/0.166.0".into()};
    catalog
        .inner
        .state
        .lock()
        .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?
        .plans
        .insert(
            plan.id,
            PlanRecord {
                plan: plan.clone(),
                state: InstallState::Prepared,
                result: None,
            },
        );
    Ok(plan)
}
pub(crate) async fn confirm(
    catalog: &AgentCatalog,
    id: InstallPlanId,
) -> Result<InstallOutcome, CatalogError> {
    let plan = {
        let mut state = catalog
            .inner
            .state
            .lock()
            .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?;
        let record = state.plans.get(&id).ok_or_else(|| {
            failure(
                CatalogErrorKind::InvalidPlan,
                "review a current installation plan before confirming",
                false,
            )
        })?;
        if state.plans.values().any(|candidate| {
            candidate.plan.target_directory == record.plan.target_directory
                && matches!(&candidate.state, InstallState::Failed { error } if error.kind == CatalogErrorKind::CleanupUnconfirmed)
        }) {
            return Err(unconfirmed("a previous installation has unresolved owned resources"));
        }
        if let Some(definition) = &record.result {
            return Ok(InstallOutcome {
                plan_id: id,
                definition: definition.clone(),
                reused_completed_result: true,
            });
        }
        if state.plans.values().any(|candidate| {
            candidate.plan.target_directory == record.plan.target_directory
                && candidate.state == InstallState::Running
        }) {
            return Err(failure(
                CatalogErrorKind::InstallInProgress,
                "installation is already in progress for this target",
                true,
            ));
        }
        let plan = record.plan.clone();
        state.plans.get_mut(&id).unwrap().state = InstallState::Running;
        plan
    };
    let result = perform(catalog, &plan).await;
    let mut state = catalog
        .inner
        .state
        .lock()
        .map_err(|_| failure(CatalogErrorKind::Io, "catalog state unavailable", true))?;
    let record = state.plans.get_mut(&id).ok_or_else(|| {
        failure(
            CatalogErrorKind::InvalidPlan,
            "installation plan expired",
            false,
        )
    })?;
    match result {
        Ok(definition) => {
            record.state = InstallState::Succeeded;
            record.result = Some(definition.clone());
            Ok(InstallOutcome {
                plan_id: id,
                definition,
                reused_completed_result: false,
            })
        }
        Err(error) => {
            record.state = InstallState::Failed {
                error: error.clone(),
            };
            Err(error)
        }
    }
}
async fn perform(
    catalog: &AgentCatalog,
    plan: &InstallPlan,
) -> Result<AgentDefinition, CatalogError> {
    let expected = catalog
        .inner
        .root
        .join("agents/codex-acp/2.5.0-codex-0.166.0");
    if plan.target_directory != expected
        || plan.adapter_version != CODEX_ADAPTER_VERSION
        || plan.native_agent_version != CODEX_NATIVE_VERSION
        || plan.source_url != OFFICIAL_REGISTRY_URL
    {
        return Err(failure(
            CatalogErrorKind::InvalidPlan,
            "installation plan no longer matches the reviewed source/target",
            false,
        ));
    }
    validate_existing_target(&catalog.inner.root, &expected).await?;
    ensure_target(&catalog.inner.root).await?;
    let lock = expected.join(".install.lock");
    let mut guard = InstallLock::take(lock, id_string(plan.id)).await?;
    let result = async {
    let owner = json!({"application":"native-agent-workbench","registry_id":CODEX_REGISTRY_ID,"adapter_version":CODEX_ADAPTER_VERSION,"native_agent_version":CODEX_NATIVE_VERSION});
    let manifest = json!({"private":true,"dependencies":{"@agentclientprotocol/codex-acp":"2.5.0","@openai/codex":"0.166.0"},"overrides":{"@agentclientprotocol/codex-acp":{"@openai/codex":"0.166.0"}}});
    fs::write(
        expected.join(OWNER_FILE),
        serde_json::to_vec(&owner).unwrap(),
    )
    .await
    .map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "could not record installation ownership",
            true,
        )
    })?;
    fs::write(
        expected.join("package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .await
    .map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "could not write pinned isolated package manifest",
            true,
        )
    })?;
    fs::write(expected.join(".npmrc"),"registry=https://registry.npmjs.org/\n@openai:registry=https://registry.npmjs.org/\n@agentclientprotocol:registry=https://registry.npmjs.org/\nfetch-retries=0\nfetch-timeout=60000\n").await.map_err(|_|failure(CatalogErrorKind::Io,"could not write isolated npm source configuration",true))?;
    fs::write(
        expected.join(".npmrc-global"),
        "# isolated application install; no credentials\n",
    )
    .await
    .map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "could not write isolated npm configuration",
            true,
        )
    })?;
    let result = catalog.inner.installer.run(plan.command.clone()).await;
    result?;
    let script = expected.join("node_modules/@agentclientprotocol/codex-acp/dist/index.js");
    let validated = discovery::manual(
        plan.agent_id,
        "Codex",
        ManualLaunch::new(
            plan.command.program.clone(),
            vec![script.to_string_lossy().into()],
        ),
    )
    .await?;
    if validated.compatibility != RuntimeCompatibility::SupportedCodex {
        return Err(failure(
            CatalogErrorKind::InstallFailed,
            "installed package does not resolve the pinned adapter/native combination",
            true,
        ));
    }
    // Version only, not an Agent session/handshake. Service still owns connection acceptance.
    let adapter_version = discovery::version(
        &plan.command.program,
        &[script.to_string_lossy().into(), "--version".into()],
    )
    .await?;
    let engine = validated.native_agent.as_ref().ok_or_else(|| {
        failure(
            CatalogErrorKind::InstallFailed,
            "installed native Codex is missing",
            true,
        )
    })?;
    let engine_version = discovery::version(&engine.executable, &["--version".into()]).await?;
    if adapter_version != CODEX_ADAPTER_VERSION || engine_version != CODEX_NATIVE_VERSION {
        return Err(failure(
            CatalogErrorKind::InstallFailed,
            "installed executable versions do not match the confirmed plan",
            true,
        ));
    }
    let mut definition = validated.definition;
    definition.source = AgentSource::Registry {
        registry_id: plan.registry_id.clone(),
    };
    definition.distributions = vec![AgentDistribution::new(
        "npx",
        CODEX_ADAPTER_VERSION,
        OFFICIAL_REGISTRY_URL,
    )];
    Ok(definition)
    }.await;
    if !matches!(&result, Err(error) if error.kind == CatalogErrorKind::CleanupUnconfirmed) {
        guard.release();
    }
    result
}
fn id_string(id: InstallPlanId) -> String {
    id.0.to_string()
}
async fn validate_existing_target(root: &Path, target: &Path) -> Result<(), CatalogError> {
    if !target.exists() {
        return Ok(());
    }
    containment(root, target).await?;
    if !target.is_dir() {
        return Err(failure(
            CatalogErrorKind::InvalidPlan,
            "installation target is not a directory",
            false,
        ));
    }
    if let Ok(bytes) = fs::read(target.join(OWNER_FILE)).await {
        if let Ok(owner) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if owner["application"] == "native-agent-workbench"
                && owner["registry_id"] == CODEX_REGISTRY_ID
                && owner["adapter_version"] == CODEX_ADAPTER_VERSION
                && owner["native_agent_version"] == CODEX_NATIVE_VERSION
            {
                return Ok(());
            }
        }
    }
    let mut entries = fs::read_dir(target).await.map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "installation target cannot be inspected",
            false,
        )
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|_| {
            failure(
                CatalogErrorKind::Io,
                "installation target cannot be inspected",
                false,
            )
        })?
        .is_some()
    {
        return Err(failure(
            CatalogErrorKind::InvalidPlan,
            "non-empty target is not an owned compatible installation; inspect or choose a different application root",
            false,
        ));
    }
    Ok(())
}
async fn containment(root: &Path, target: &Path) -> Result<(), CatalogError> {
    let root = fs::canonicalize(root).await.map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "application data root cannot be resolved",
            false,
        )
    })?;
    let target = fs::canonicalize(target).await.map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "installation target cannot be resolved",
            false,
        )
    })?;
    if !target.starts_with(root) {
        return Err(failure(
            CatalogErrorKind::InvalidPlan,
            "installation target resolves outside application data",
            false,
        ));
    }
    Ok(())
}
async fn ensure_target(root: &Path) -> Result<(), CatalogError> {
    fs::create_dir_all(root).await.map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "could not create application data root",
            true,
        )
    })?;
    let canonical = fs::canonicalize(root).await.map_err(|_| {
        failure(
            CatalogErrorKind::Io,
            "application data root cannot be resolved",
            false,
        )
    })?;
    let mut parent = root.to_owned();
    for component in ["agents", "codex-acp", "2.5.0-codex-0.166.0"] {
        parent.push(component);
        if !parent.exists() {
            fs::create_dir(&parent).await.map_err(|_| {
                failure(
                    CatalogErrorKind::Io,
                    "could not create owned installation directory",
                    true,
                )
            })?;
        }
        let resolved = fs::canonicalize(&parent).await.map_err(|_| {
            failure(
                CatalogErrorKind::Io,
                "installation directory cannot be resolved",
                false,
            )
        })?;
        if !resolved.starts_with(&canonical) || !resolved.is_dir() {
            return Err(failure(
                CatalogErrorKind::InvalidPlan,
                "installation directory escapes application data or is not a directory",
                false,
            ));
        }
    }
    Ok(())
}
struct InstallLock {
    path: PathBuf,
    nonce: String,
    retain: bool,
}
impl InstallLock {
    async fn take(path: PathBuf, nonce: String) -> Result<Self, CatalogError> {
        use tokio::io::AsyncWriteExt;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await
            .map_err(|_| {
                failure(
                    CatalogErrorKind::InstallInProgress,
                    "another or unconfirmed previous installation owns this target",
                    true,
                )
            })?;
        if file.write_all(nonce.as_bytes()).await.is_err() {
            drop(file);
            let _ = fs::remove_file(&path).await;
            return Err(failure(
                CatalogErrorKind::Io,
                "could not record installation lock",
                true,
            ));
        }
        Ok(Self {
            path,
            nonce,
            retain: true,
        })
    }
    fn release(&mut self) {
        self.retain = false;
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        if !self.retain && std::fs::read_to_string(&self.path).ok().as_deref() == Some(&self.nonce)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
