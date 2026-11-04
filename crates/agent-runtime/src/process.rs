use crate::{RuntimeOptions, error};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, task::JoinHandle};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use workbench_core::*;

pub(crate) struct OwnedProcess {
    child: Box<dyn ChildWrapper>,
    stderr: JoinHandle<()>,
}
impl OwnedProcess {
    pub fn spawn(
        launch: &AgentLaunchConfiguration,
        cwd: &Path,
        options: &RuntimeOptions,
    ) -> Result<
        (
            Self,
            agent_client_protocol::ByteStreams<
                impl futures::AsyncWrite + Send + 'static,
                impl futures::AsyncRead + Send + 'static,
            >,
        ),
        BusinessError,
    > {
        if launch.executable.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("ps1")
                || extension.eq_ignore_ascii_case("cmd")
                || extension.eq_ignore_ascii_case("bat")
        }) {
            return Err(error(
                ErrorKind::InvalidInput,
                "launch must point at a native executable; pass adapter JS as a node argument",
                RetryAdvice::AfterCorrection,
            ));
        }
        let mut command = CommandWrap::with_new(&launch.executable, |cmd| {
            cmd.args(&launch.args)
                .current_dir(cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(path) = &options.codex_executable {
                cmd.env("CODEX_PATH", path);
            }
        });
        command.wrap(KillOnDrop);
        #[cfg(windows)]
        {
            let mut flags = process_wrap::tokio::CreationFlags(Default::default());
            flags.0.0 = 0x08000000;
            command.wrap(flags).wrap(process_wrap::tokio::JobObject);
        }
        #[cfg(unix)]
        {
            command.wrap(process_wrap::tokio::ProcessGroup::leader());
        }
        let mut child = command.spawn().map_err(|_| {
            error(
                ErrorKind::ResourceUnavailable,
                "could not start the configured ACP adapter",
                RetryAdvice::AfterCorrection,
            )
        })?;
        let stdin = child.stdin().take().ok_or_else(|| {
            error(
                ErrorKind::Transport,
                "missing protocol stdin",
                RetryAdvice::Never,
            )
        })?;
        let stdout = child.stdout().take().ok_or_else(|| {
            error(
                ErrorKind::Transport,
                "missing protocol stdout",
                RetryAdvice::Never,
            )
        })?;
        let mut stderr = child.stderr().take().ok_or_else(|| {
            error(
                ErrorKind::Transport,
                "missing diagnostic stderr",
                RetryAdvice::Never,
            )
        })?;
        // Drain diagnostics independently. Raw output can contain secrets and is never persisted.
        let stderr = tokio::spawn(async move {
            let mut buffer = [0u8; 8192];
            while stderr.read(&mut buffer).await.unwrap_or(0) > 0 {}
        });
        Ok((
            Self { child, stderr },
            agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat()),
        ))
    }
    pub async fn close(mut self) -> Result<(), BusinessError> {
        // Protocol EOF gets a short grace period. Then terminate the owned job/group
        // before its FIRST wait: process-wrap caches the parent exit status during wait,
        // so cancelling a wait and waiting again cannot prove descendant exit.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let termination = self.child.start_kill();
        let exited = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .map_err(|_| {
                error(
                    ErrorKind::Transport,
                    "owned process tree did not exit",
                    RetryAdvice::ReconcileFirst,
                )
            })?
            .map_err(|_| {
                error(
                    ErrorKind::Transport,
                    "could not verify owned process exit",
                    RetryAdvice::ReconcileFirst,
                )
            })?;
        // The Job Object's wait includes its descendants. Exit status is not run success.
        let _ = (termination, exited);
        self.stderr.abort();
        Ok(())
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.stderr.abort();
    }
}
