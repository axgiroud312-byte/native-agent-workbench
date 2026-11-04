use crate::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{fs, io::AsyncReadExt, process::Command};
use workbench_core::*;

pub(crate) async fn discover(
    root: &Path,
    configured: Vec<AgentDefinition>,
) -> Result<LocalDiscovery, CatalogError> {
    let tools = tools().await;
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(node) = &tools.node {
        for directory in path_directories() {
            let script =
                directory.join("node_modules/@agentclientprotocol/codex-acp/dist/index.js");
            if script.is_file() {
                candidate(&node.executable, &script, &mut seen, &mut candidates).await;
            }
        }
        let parent = root.join("agents/codex-acp");
        if let Ok(mut entries) = fs::read_dir(parent).await {
            let mut count = 0;
            while let Ok(Some(entry)) = entries.next_entry().await {
                if count >= 64 {
                    break;
                }
                count += 1;
                let script = entry
                    .path()
                    .join("node_modules/@agentclientprotocol/codex-acp/dist/index.js");
                if script.is_file() {
                    candidate(&node.executable, &script, &mut seen, &mut candidates).await;
                }
            }
        }
    }
    let mut validated = Vec::new();
    for mut definition in configured {
        if let Some(launch) = &definition.launch {
            let result = inspect(launch).await;
            definition.discovery = match result {
                Ok((version, _, RuntimeCompatibility::SupportedCodex)) => {
                    AgentDiscovery::Installed {
                        executable: launch.executable.clone(),
                        version,
                        distribution_id: None,
                    }
                }
                Ok((_, _, RuntimeCompatibility::Incompatible { reason })) => {
                    AgentDiscovery::Incompatible { reason }
                }
                Err(error) => AgentDiscovery::Failed {
                    reason: error.message,
                },
            };
        }
        // Existing ID, source, launch and distribution are never replaced by remote data.
        validated.push(definition);
    }
    Ok(LocalDiscovery {
        tools,
        configured: validated,
        candidates,
    })
}
async fn candidate(
    node: &Path,
    script: &Path,
    seen: &mut BTreeSet<String>,
    output: &mut Vec<LocalCandidate>,
) {
    let launch = AgentLaunchConfiguration::new(node.into(), vec![script.to_string_lossy().into()]);
    let Ok((version, native, compatibility)) = inspect(&launch).await else {
        return;
    };
    let key = key(&launch);
    if !seen.insert(key.clone()) {
        return;
    }
    output.push(LocalCandidate {
        key,
        name: "Codex".into(),
        registry_id: Some(CODEX_REGISTRY_ID.into()),
        launch,
        adapter_version: version,
        native_agent: native,
        compatibility,
    });
}
pub(crate) async fn manual(
    id: AgentId,
    name: &str,
    input: ManualLaunch,
) -> Result<ManualValidation, CatalogError> {
    if name.trim().is_empty() {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "Agent name is required",
            false,
        ));
    }
    if input.args.iter().any(|arg| {
        let value = arg.to_ascii_lowercase();
        [
            "--api-key",
            "--token",
            "--password",
            "api_key=",
            "auth_token=",
            "access_token=",
        ]
        .iter()
        .any(|key| value.contains(key))
            || value.starts_with("sk-")
    }) {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "credentials cannot be saved in launch arguments; use official authentication",
            false,
        ));
    }
    let executable = absolute_file(&input.executable).await?;
    let launch = if is_script_wrapper(&executable) {
        if !input.args.is_empty() {
            return Err(failure(
                CatalogErrorKind::InvalidLaunch,
                "wrapper arguments cannot be discarded; select native Node and the adapter JS explicitly",
                false,
            ));
        }
        wrapper_launch(&executable).await.ok_or_else(|| {
            failure(
                CatalogErrorKind::InvalidLaunch,
                "script wrappers are not native executables; select node.exe and the adapter JS",
                false,
            )
        })?
    } else {
        let mut args = input.args;
        if args.len() == 1 {
            let arg = PathBuf::from(&args[0]);
            if arg.is_absolute() && arg.is_file() {
                args[0] = fs::canonicalize(arg)
                    .await
                    .map_err(|_| {
                        failure(
                            CatalogErrorKind::InvalidLaunch,
                            "adapter entry cannot be resolved",
                            false,
                        )
                    })?
                    .to_string_lossy()
                    .into();
            }
        }
        AgentLaunchConfiguration::new(executable, args)
    };
    let (version, native, compatibility) = inspect(&launch).await?;
    let discovery = match &compatibility {
        RuntimeCompatibility::SupportedCodex => AgentDiscovery::Installed {
            executable: launch.executable.clone(),
            version,
            distribution_id: None,
        },
        RuntimeCompatibility::Incompatible { reason } => AgentDiscovery::Incompatible {
            reason: reason.clone(),
        },
    };
    let mut definition = AgentDefinition::new(id, name.trim(), AgentSource::LocalConfiguration);
    definition.launch = Some(launch);
    definition.discovery = discovery.clone();
    Ok(ManualValidation {
        definition,
        compatibility,
        native_agent: native,
        discovery,
    })
}
pub(crate) async fn inspect(
    launch: &AgentLaunchConfiguration,
) -> Result<(Option<String>, Option<NativeTool>, RuntimeCompatibility), CatalogError> {
    native_file(&launch.executable).await?;
    if launch
        .executable
        .file_stem()
        .is_none_or(|name| !name.eq_ignore_ascii_case("node"))
        || launch.args.len() != 1
    {
        return Ok((None,None,RuntimeCompatibility::Incompatible {reason:"current Runtime requires native Node plus the Codex ACP 2.5.0 adapter entry; a plain Codex CLI is not an ACP server".into()}));
    }
    let script = absolute_file(Path::new(&launch.args[0])).await?;
    let Some(package) = script.parent().and_then(Path::parent) else {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "adapter package root is missing",
            false,
        ));
    };
    let manifest = manifest(&package.join("package.json")).await?;
    let version = manifest["version"].as_str().map(str::to_owned);
    if manifest["name"] != "@agentclientprotocol/codex-acp"
        || script
            != fs::canonicalize(package.join("dist/index.js"))
                .await
                .unwrap_or_default()
        || version.as_deref() != Some(CODEX_ADAPTER_VERSION)
    {
        return Ok((version,None,RuntimeCompatibility::Incompatible {reason:"this entry is not the verified Codex ACP 2.5.0 package; other Runtime mappings are not implemented".into()}));
    }
    let native = match resolve_codex(package).await {
        Ok(native) => native,
        Err(error) => {
            return Ok((
                version,
                None,
                RuntimeCompatibility::Incompatible {
                    reason: error.message,
                },
            ));
        }
    };
    let compatibility = if native.version == CODEX_NATIVE_VERSION {
        RuntimeCompatibility::SupportedCodex
    } else {
        RuntimeCompatibility::Incompatible {
            reason: format!(
                "this adapter resolves Codex {}; the verified combination requires 0.166.0",
                native.version
            ),
        }
    };
    Ok((version, Some(native), compatibility))
}
pub(crate) async fn tools() -> ToolchainDiscovery {
    let mut tools = ToolchainDiscovery::default();
    if let Some(node) = find_native("node") {
        match version(&node, &["--version".into()]).await {
            Ok(version) => {
                tools.node = Some(NativeTool {
                    executable: node,
                    args: Vec::new(),
                    version,
                    evidence: VersionEvidence::ExecutedVersion,
                })
            }
            Err(error) => tools.issues.push(error.message),
        }
    } else {
        tools
            .issues
            .push("native Node was not found on PATH".into());
    }
    if let Some(node) = &tools.node {
        let mut candidates = Vec::new();
        if let Some(parent) = node.executable.parent() {
            candidates.push(parent.join("node_modules/npm/bin/npm-cli.js"));
        }
        for directory in path_directories() {
            candidates.push(directory.join("node_modules/npm/bin/npm-cli.js"));
        }
        for script in candidates {
            if !script.is_file() {
                continue;
            }
            let Ok(package) = manifest(
                &script
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("package.json"),
            )
            .await
            else {
                continue;
            };
            if package["name"] != "npm" {
                continue;
            }
            let args = vec![script.to_string_lossy().into(), "--version".into()];
            if let Ok(version) = version(&node.executable, &args).await {
                tools.npm = Some(NativeTool {
                    executable: node.executable.clone(),
                    args: vec![script.to_string_lossy().into()],
                    version,
                    evidence: VersionEvidence::ExecutedVersion,
                });
                break;
            }
        }
    }
    if tools.npm.is_none() {
        tools
            .issues
            .push("native Node/npm CLI entry was not found; no wrapper will be executed".into());
    }
    if let Some(exe) = find_native("codex") {
        if let Ok(version) = version(&exe, &["--version".into()]).await {
            tools.codex = Some(NativeTool {
                executable: exe,
                args: Vec::new(),
                version,
                evidence: VersionEvidence::ExecutedVersion,
            });
        }
    }
    if tools.codex.is_none() {
        for directory in path_directories() {
            if !directory.join("codex.ps1").is_file()
                && !directory.join("codex.cmd").is_file()
                && !directory
                    .join("node_modules/@openai/codex/package.json")
                    .is_file()
            {
                continue;
            }
            if let Ok(native) =
                resolve_codex_package(&directory.join("node_modules/@openai/codex")).await
            {
                if let Ok(version) = version(&native.executable, &["--version".into()]).await {
                    tools.codex = Some(NativeTool {
                        version,
                        evidence: VersionEvidence::ExecutedVersion,
                        ..native
                    });
                    break;
                }
            }
        }
    }
    tools
}
pub(crate) async fn resolve_codex(start: &Path) -> Result<NativeTool, CatalogError> {
    for directory in start.ancestors() {
        let package = directory.join("node_modules/@openai/codex");
        if package.join("package.json").is_file() {
            return resolve_codex_package(&package).await;
        }
    }
    Err(failure(
        CatalogErrorKind::InvalidLaunch,
        "the adapter has no matching local @openai/codex dependency; global CODEX_PATH is not used",
        false,
    ))
}
async fn resolve_codex_package(package: &Path) -> Result<NativeTool, CatalogError> {
    let metadata = manifest(&package.join("package.json")).await?;
    if metadata["name"] != "@openai/codex" {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "Codex package identity is invalid",
            false,
        ));
    }
    let version = metadata["version"]
        .as_str()
        .ok_or_else(|| {
            failure(
                CatalogErrorKind::InvalidLaunch,
                "Codex package version is absent",
                false,
            )
        })?
        .to_owned();
    let (native, triple) = match std::env::consts::ARCH {
        "x86_64" => ("@openai/codex-win32-x64", "x86_64-pc-windows-msvc"),
        "aarch64" => ("@openai/codex-win32-arm64", "aarch64-pc-windows-msvc"),
        _ => {
            return Err(failure(
                CatalogErrorKind::Unsupported,
                "Windows architecture has no validated Codex package",
                false,
            ));
        }
    };
    for ancestor in package.ancestors() {
        let dependency = ancestor.join("node_modules").join(native);
        let exe = dependency.join("vendor").join(triple).join("bin/codex.exe");
        if exe.is_file() {
            let actual = manifest(&dependency.join("package.json")).await?;
            if actual["version"].as_str()
                != Some(&format!(
                    "{version}-win32-{}",
                    if std::env::consts::ARCH == "aarch64" {
                        "arm64"
                    } else {
                        "x64"
                    }
                ))
            {
                return Err(failure(
                    CatalogErrorKind::InvalidLaunch,
                    "Codex wrapper/native package versions do not match",
                    false,
                ));
            }
            native_file(&exe).await?;
            return Ok(NativeTool {
                executable: exe,
                args: Vec::new(),
                version,
                evidence: VersionEvidence::PackageManifest,
            });
        }
    }
    let exe = package.join("vendor").join(triple).join("bin/codex.exe");
    native_file(&exe).await?;
    Ok(NativeTool {
        executable: exe,
        args: Vec::new(),
        version,
        evidence: VersionEvidence::PackageManifest,
    })
}
async fn wrapper_launch(wrapper: &Path) -> Option<AgentLaunchConfiguration> {
    if wrapper.file_stem()?.to_str()? != "codex-acp" {
        return None;
    }
    let script = wrapper
        .parent()?
        .join("node_modules/@agentclientprotocol/codex-acp/dist/index.js");
    if !script.is_file() {
        return None;
    }
    let node = find_native("node")?;
    Some(AgentLaunchConfiguration::new(
        node,
        vec![
            fs::canonicalize(script)
                .await
                .ok()?
                .to_string_lossy()
                .into(),
        ],
    ))
}
fn find_native(name: &str) -> Option<PathBuf> {
    path_directories()
        .into_iter()
        .map(|path| {
            path.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            })
        })
        .find(|path| path.is_file())
}
fn path_directories() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|path| path.is_absolute())
                .collect()
        })
        .unwrap_or_default()
}
fn is_script_wrapper(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["ps1", "cmd", "bat"]
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
    })
}
async fn absolute_file(path: &Path) -> Result<PathBuf, CatalogError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "select an existing absolute executable or adapter file path",
            false,
        ));
    }
    fs::canonicalize(path).await.map_err(|_| {
        failure(
            CatalogErrorKind::InvalidLaunch,
            "the selected file cannot be resolved",
            false,
        )
    })
}
pub(crate) async fn native_file(path: &Path) -> Result<(), CatalogError> {
    if !path.is_absolute() || !path.is_file() || is_script_wrapper(path) {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "a native executable is required",
            false,
        ));
    }
    #[cfg(windows)]
    {
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            return Err(failure(
                CatalogErrorKind::InvalidLaunch,
                "Windows launch requires an .exe",
                false,
            ));
        }
        let mut file = fs::File::open(path).await.map_err(|_| {
            failure(
                CatalogErrorKind::InvalidLaunch,
                "native executable is unreadable",
                false,
            )
        })?;
        let mut header = [0; 2];
        file.read_exact(&mut header).await.map_err(|_| {
            failure(
                CatalogErrorKind::InvalidLaunch,
                "native executable header is unreadable",
                false,
            )
        })?;
        if header != *b"MZ" {
            return Err(failure(
                CatalogErrorKind::InvalidLaunch,
                "selected .exe has no Windows executable header",
                false,
            ));
        }
    }
    Ok(())
}
pub(crate) async fn manifest(path: &Path) -> Result<Value, CatalogError> {
    let bytes = fs::read(path).await.map_err(|_| {
        failure(
            CatalogErrorKind::InvalidLaunch,
            "expected package manifest is missing",
            false,
        )
    })?;
    if bytes.len() > 512 * 1024 {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "package manifest is too large",
            false,
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        failure(
            CatalogErrorKind::InvalidLaunch,
            "package manifest is invalid",
            false,
        )
    })
}
pub(crate) async fn version(executable: &Path, args: &[String]) -> Result<String, CatalogError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    let output = tokio::time::timeout(Duration::from_secs(8), command.output())
        .await
        .map_err(|_| {
            failure(
                CatalogErrorKind::Timeout,
                "local tool version check timed out",
                true,
            )
        })?
        .map_err(|_| {
            failure(
                CatalogErrorKind::MissingTool,
                "local tool could not be executed for version detection",
                false,
            )
        })?;
    if !output.status.success() || output.stdout.len() > 16384 {
        return Err(failure(
            CatalogErrorKind::InvalidLaunch,
            "local tool version check failed",
            false,
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|part| {
            part.trim_start_matches('v')
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_digit())
                && part.contains('.')
        })
        .map(|part| part.trim_start_matches('v').to_owned())
        .ok_or_else(|| {
            failure(
                CatalogErrorKind::InvalidLaunch,
                "local tool did not return a recognizable version",
                false,
            )
        })
}
fn key(launch: &AgentLaunchConfiguration) -> String {
    let mut digest = Sha256::new();
    digest.update(
        launch
            .executable
            .to_string_lossy()
            .to_lowercase()
            .as_bytes(),
    );
    for arg in &launch.args {
        digest.update([0]);
        digest.update(arg.to_lowercase().as_bytes());
    }
    format!("local:{:x}", digest.finalize())
}
