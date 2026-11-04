//! Windows-only local process regression probe. No Agent, network or credentials.
use std::{io::Write, process::Stdio, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("--leaf") => {
            println!("LEAF_READY:{}", std::process::id());
            std::io::stdout().flush()?;
            // Retain inherited stdout so EOF also independently witnesses leaf exit.
            std::thread::sleep(Duration::from_secs(60));
            Ok(())
        }
        Some("--parent") => {
            let child = std::process::Command::new(std::env::current_exe()?)
                .arg("--leaf")
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::null())
                .spawn()?;
            println!("DESCENDANT:{}", child.id());
            std::io::stdout().flush()?;
            // std::Child drop leaves the descendant alive when this parent exits.
            Ok(())
        }
        _ => {
            #[cfg(windows)]
            {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(probe())
            }
            #[cfg(not(windows))]
            {
                Err("this probe requires Windows Job Objects".into())
            }
        }
    }
}

#[cfg(windows)]
async fn probe() -> Result<(), Box<dyn std::error::Error>> {
    use process_wrap::tokio::{CommandWrap, CreationFlags, JobObject, KillOnDrop};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let mut command = CommandWrap::with_new(std::env::current_exe()?, |command| {
        command
            .arg("--parent")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
    });
    let mut flags = CreationFlags(Default::default());
    flags.0.0 = 0x08000000;
    command.wrap(KillOnDrop).wrap(flags).wrap(JobObject);
    let mut child = command.spawn()?;
    let parent_pid = child.id().ok_or("parent PID missing")?;
    let mut pipe = BufReader::new(child.stdout().take().ok_or("helper stdout missing")?);
    let mut descendant = None;
    let mut ready = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while descendant.is_none() || !ready {
            let mut line = String::new();
            if pipe.read_line(&mut line).await? == 0 {
                return Err(std::io::Error::other("helper exited before leaf was ready"));
            }
            if let Some(pid) = line.trim().strip_prefix("DESCENDANT:") {
                descendant = Some(pid.parse::<u32>().map_err(std::io::Error::other)?);
            }
            ready |= line.starts_with("LEAF_READY:");
        }
        loop {
            // Root parent status only, beneath the Job wrapper. Its exit is not Job completion.
            if let Some(status) = child.inner_mut().try_wait()? {
                if !status.success() {
                    return Err(std::io::Error::other("helper parent failed"));
                }
                return Ok::<_, std::io::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    if child.try_wait()?.is_some() {
        return Err("try_wait returned while descendant was alive".into());
    }
    if tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .is_ok()
    {
        return Err("Job wait returned early after only its parent exited".into());
    }
    child.start_kill()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while child.try_wait()?.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, std::io::Error>(())
    })
    .await??;
    // try_wait consumed ACTIVE_PROCESS_ZERO; wait must retain that fact and not hang.
    let status = tokio::time::timeout(Duration::from_secs(1), child.wait()).await??;
    if !status.success() {
        return Err("original parent exit status changed during tree cleanup".into());
    }
    let mut tail = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), pipe.read_to_end(&mut tail)).await??;
    println!(
        "parent={parent_pid}; descendant={}; parent_exited=true; early_job_wait=false; cancelled_wait_resumed=true; try_wait_empty_retained=true; descendant_pipe_eof=true",
        descendant.unwrap()
    );
    Ok(())
}
