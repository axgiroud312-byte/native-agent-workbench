//! Read-only discovery/Registry/plan probe. Does not confirm or run an installation.
use agent_catalog::*;
use std::path::PathBuf;
use workbench_core::AgentId;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(
        args.next()
            .ok_or("provide an absolute application-data root")?,
    );
    let catalog = AgentCatalog::new(root)?;
    let mut configured = Vec::new();
    if let Some(node) = args.next() {
        let script = args
            .next()
            .ok_or("provide an adapter JS after native Node")?;
        let manual = catalog
            .validate_manual(
                AgentId::new(),
                "Explicit local Codex",
                ManualLaunch::new(node.into(), vec![script.to_string_lossy().into()]),
            )
            .await?;
        println!(
            "manual: {:?}; native evidence: {:?}",
            manual.compatibility,
            manual
                .native_agent
                .as_ref()
                .map(|tool| (&tool.version, &tool.evidence))
        );
        configured.push(manual.definition);
    }
    let originals = configured.clone();
    let snapshot = catalog
        .refresh(configured, RefreshPolicy::Revalidate)
        .await?;
    println!(
        "registry: {:?}; format: {:?}; entries: {}; fetched: {:?}; expires: {:?}",
        snapshot.registry.state,
        snapshot.registry.format_version,
        snapshot.available.len(),
        snapshot.registry.fetched_at,
        snapshot.registry.expires_at
    );
    for (name, tool) in [
        ("Node", &snapshot.local.tools.node),
        ("npm", &snapshot.local.tools.npm),
        ("Codex", &snapshot.local.tools.codex),
    ] {
        println!(
            "{name}: {:?}",
            tool.as_ref().map(|tool| (&tool.version, &tool.evidence))
        );
    }
    for original in originals {
        let retained = snapshot
            .local
            .configured
            .iter()
            .find(|current| current.id == original.id)
            .ok_or("configured ID lost")?;
        if retained.launch != original.launch
            || retained.source != original.source
            || retained.distributions != original.distributions
        {
            return Err("configured launch/source changed during refresh".into());
        }
    }
    println!(
        "configured preserved: {}; discovered candidates: {}",
        snapshot.local.configured.len(),
        snapshot.local.candidates.len()
    );
    if let Some(codex) = snapshot
        .available
        .iter()
        .find(|agent| agent.registry_id == CODEX_REGISTRY_ID)
    {
        println!(
            "Codex Registry version: {}; installation: {:?}",
            codex.version, codex.installation
        );
        if codex.installation == Installability::Supported {
            let plan = catalog
                .prepare_install(CODEX_REGISTRY_ID, AgentId::new())
                .await?;
            println!(
                "prepared {:?}; versions: {}/{}; source: {}; target: {}; command: {} {:?}",
                plan.id,
                plan.adapter_version,
                plan.native_agent_version,
                plan.source_url,
                plan.target_directory.display(),
                plan.command.program.display(),
                plan.command.args
            );
            println!(
                "state: {:?}; confirmed/executed: false",
                catalog.plan_state(plan.id)
            );
            if plan.target_directory.exists() {
                println!("target already exists; prepare did not write it");
            }
        }
    }
    Ok(())
}
