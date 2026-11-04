use std::path::{Path, PathBuf};
use workbench_core::RequestId;

pub(crate) struct TestDirectory(PathBuf);
impl TestDirectory {
    pub(crate) fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("workbench-catalog-test-{}", RequestId::new()));
        std::fs::create_dir(&root).unwrap();
        Self(std::fs::canonicalize(root).unwrap())
    }
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        // Remove only this test's own, still identically resolved directory.
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        if self.0.starts_with(&base)
            && self.0.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("workbench-catalog-test-")
            })
            && std::fs::canonicalize(&self.0).ok().as_ref() == Some(&self.0)
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::test]
async fn install_failure_is_retryable_and_concurrent_confirmation_is_rejected() {
    use crate::install::{Installer, PlanRecord};
    use crate::*;
    use futures::future::BoxFuture;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Notify;
    use workbench_core::AgentId;

    struct FailingInstaller {
        calls: AtomicUsize,
        entered: Notify,
        release: Notify,
    }
    impl Installer for FailingInstaller {
        fn run(&self, _: InstallCommand) -> BoxFuture<'_, Result<(), CatalogError>> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                self.release.notified().await;
                Err(if call >= 2 {
                    failure(
                        CatalogErrorKind::CleanupUnconfirmed,
                        "fake cleanup unconfirmed",
                        false,
                    )
                } else {
                    failure(
                        CatalogErrorKind::Timeout,
                        "fake installer released after timeout",
                        true,
                    )
                })
            })
        }
    }
    let root = TestDirectory::new();
    let installer = Arc::new(FailingInstaller {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut catalog = AgentCatalog::new(root.path().into()).unwrap();
    Arc::get_mut(&mut catalog.inner).unwrap().installer = installer.clone();
    let target = root.path().join("agents/codex-acp/2.5.0-codex-0.166.0");
    let plan = InstallPlan {
        id: InstallPlanId(RequestId::new()),
        agent_id: AgentId::new(),
        registry_id: CODEX_REGISTRY_ID.into(),
        source_url: OFFICIAL_REGISTRY_URL.into(),
        adapter_version: CODEX_ADAPTER_VERSION.into(),
        native_agent_version: CODEX_NATIVE_VERSION.into(),
        target_directory: target.clone(),
        command: InstallCommand {
            program: PathBuf::from("unused-native-installer"),
            args: Vec::new(),
            working_directory: target.clone(),
        },
        prerequisites: Vec::new(),
        created_at: now(),
        native_agent_source_url: "https://registry.npmjs.org/@openai/codex/0.166.0".into(),
    };
    catalog.inner.state.lock().unwrap().plans.insert(
        plan.id,
        PlanRecord {
            plan: plan.clone(),
            state: InstallState::Prepared,
            result: None,
        },
    );
    let first = {
        let catalog = catalog.clone();
        let id = plan.id;
        tokio::spawn(async move { catalog.confirm_install(id).await })
    };
    installer.entered.notified().await;
    assert_eq!(catalog.plan_state(plan.id), Some(InstallState::Running));
    assert_eq!(
        catalog.confirm_install(plan.id).await.unwrap_err().kind,
        CatalogErrorKind::InstallInProgress
    );
    assert_eq!(installer.calls.load(Ordering::SeqCst), 1);
    installer.release.notify_one();
    assert_eq!(
        first.await.unwrap().unwrap_err().kind,
        CatalogErrorKind::Timeout
    );
    assert!(
        matches!(catalog.plan_state(plan.id), Some(InstallState::Failed { error }) if error.retryable)
    );
    assert!(!target.join(".install.lock").exists());
    let retry = {
        let catalog = catalog.clone();
        let id = plan.id;
        tokio::spawn(async move { catalog.confirm_install(id).await })
    };
    installer.entered.notified().await;
    installer.release.notify_one();
    assert_eq!(
        retry.await.unwrap().unwrap_err().kind,
        CatalogErrorKind::Timeout
    );
    assert_eq!(installer.calls.load(Ordering::SeqCst), 2);
    let unresolved = {
        let catalog = catalog.clone();
        let id = plan.id;
        tokio::spawn(async move { catalog.confirm_install(id).await })
    };
    installer.entered.notified().await;
    installer.release.notify_one();
    assert_eq!(
        unresolved.await.unwrap().unwrap_err().kind,
        CatalogErrorKind::CleanupUnconfirmed
    );
    assert!(target.join(".install.lock").is_file());
    assert_eq!(
        catalog.confirm_install(plan.id).await.unwrap_err().kind,
        CatalogErrorKind::CleanupUnconfirmed
    );
    assert_eq!(installer.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        catalog
            .confirm_install(InstallPlanId(RequestId::new()))
            .await
            .unwrap_err()
            .kind,
        CatalogErrorKind::InvalidPlan
    );
}
