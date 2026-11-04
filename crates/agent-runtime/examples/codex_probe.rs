//! Early real-protocol probe; the full shared-port workflow is `runtime_smoke`.
use agent_client_protocol::{ByteStreams, Client, UntypedMessage, v1};
use process_wrap::tokio::{CommandWrap, KillOnDrop};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncReadExt;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut raw_args: Vec<_> = std::env::args_os().skip(1).collect();
    let discovery = raw_args.last().is_some_and(|arg| arg == "--discover-auth");
    if discovery {
        raw_args.pop();
    }
    let args: Vec<_> = raw_args.into_iter().map(PathBuf::from).collect();
    if args.len() != 4 {
        return Err(
            "usage: codex_probe NODE_EXE ADAPTER_JS CODEX_EXE ISOLATED_DIRECTORY [--discover-auth]"
                .into(),
        );
    }
    std::fs::create_dir_all(&args[3])?;
    let mut command = CommandWrap::with_new(&args[0], |command| {
        command
            .arg(&args[1])
            .current_dir(&args[3])
            .env("CODEX_PATH", &args[2])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if discovery {
            command.env_remove("NO_BROWSER");
        } else {
            command.env("NO_BROWSER", "1");
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
    let mut child = command.spawn()?;
    let transport = ByteStreams::new(
        child.stdin().take().ok_or("stdin missing")?.compat_write(),
        child.stdout().take().ok_or("stdout missing")?.compat(),
    );
    let mut stderr = child.stderr().take().ok_or("stderr missing")?;
    // Drain, but never print authentication/configuration material from raw stderr.
    let drain = tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        while stderr.read(&mut buffer).await.unwrap_or(0) > 0 {}
    });
    let deltas = Arc::new(AtomicUsize::new(0));
    let observed = deltas.clone();
    let cwd = args[3].clone();
    let result = Client.v1().on_receive_notification(
        async move |notification: v1::schema::SessionNotification, _| {
            let update = serde_json::to_value(notification)?;
            let update = &update["update"];
            if update["sessionUpdate"] == "agent_message_chunk" {
                observed.fetch_add(1, Ordering::Relaxed);
                if let Some(text) = update["content"]["text"].as_str() { print!("{text}"); }
            }
            Ok(())
        }, agent_client_protocol::on_receive_notification!(),
    ).connect_with(transport, async move |cx: v1::ConnectionTo<agent_client_protocol::Agent>| {
        let init = cx.send_request(UntypedMessage::new("initialize", json!({"protocolVersion":1,"clientInfo":{"name":"workbench-runtime-probe","version":"0.1.0"},"clientCapabilities":{}}))?).block_task().await?;
        println!("initialize: name={} version={}", init["agentInfo"]["name"], init["agentInfo"]["version"]);
        if discovery {
            let methods=init["authMethods"].as_array().into_iter().flatten().map(|m|json!({"id":m["id"],"name":m["name"]})).collect::<Vec<_>>();
            println!("authentication_methods={}",serde_json::to_string(&methods)?);return Ok(());
        }
        let session = cx.send_request(v1::schema::NewSessionRequest::new(cwd)).block_task().await?;
        let response = cx.send_request(v1::schema::PromptRequest::new(session.session_id.clone(), vec![v1::schema::ContentBlock::Text(v1::schema::TextContent::new("Reply with exactly ACP_RUNTIME_READY. Do not use tools."))])).block_task().await?;
        println!("\nprompt stop={:?}", response.stop_reason);
        let request = cx.send_request(v1::schema::PromptRequest::new(session.session_id.clone(), vec![v1::schema::ContentBlock::Text(v1::schema::TextContent::new("Use the shell to run a 30 second sleep, then say done. Do not write any files."))]));
        let prompt = request.block_task();
        tokio::pin!(prompt);
        tokio::select! {
            result = &mut prompt => { return Err(agent_client_protocol::Error::internal_error().data(format!("cancel probe ended too early: {:?}", result.map(|r| r.stop_reason)))); }
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
        }
        cx.send_notification(v1::schema::CancelNotification::new(session.session_id))?;
        let response = tokio::time::timeout(Duration::from_secs(30), prompt).await.map_err(|_| agent_client_protocol::Error::internal_error().data("cancel did not settle in 30 seconds"))??;
        let stop = serde_json::to_value(response.stop_reason)?;
        println!("cancel stop={stop}");
        if stop != Value::String("cancelled".into()) { return Err(agent_client_protocol::Error::internal_error().data("cancel was not confirmed")); }
        Ok(())
    });
    let result = tokio::time::timeout(Duration::from_secs(120), result).await;
    // Connection drops its stdin; the Job Object remains owned until all descendants exit.
    let graceful = matches!(
        tokio::time::timeout(Duration::from_secs(5), child.wait()).await,
        Ok(Ok(_))
    );
    if !graceful {
        child.start_kill()?;
        child.wait().await?;
    }
    drain.abort();
    println!(
        "stream_chunks={} graceful_close={graceful}",
        deltas.load(Ordering::Relaxed)
    );
    result??;
    if !discovery && deltas.load(Ordering::Relaxed) == 0 {
        return Err("no real streaming chunks observed".into());
    }
    Ok(())
}
