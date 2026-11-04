use crate::{
    Facts, RuntimeOptions, Work,
    authentication::{AuthIncoming, AuthWork, AuthenticationSession},
    command, error,
    interaction::Responders,
    mapping::Mapper,
    process::OwnedProcess,
};
use agent_client_protocol::{self as acp, UntypedMessage, v1};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tokio::sync::{mpsc, watch};
use workbench_core::*;

enum Incoming {
    Notification(String, Value),
    Request(String, Value, v1::Responder),
    Prompt(
        TransportAttempt,
        Result<Value, acp::Error>,
        tokio::sync::oneshot::Sender<Result<(), BusinessError>>,
    ),
    Prepared(
        TransportAttempt,
        InputSubmission,
        Result<Vec<Value>, BusinessError>,
        tokio::sync::oneshot::Sender<Result<(), BusinessError>>,
    ),
    Config(ConfigRequest, Result<Value, acp::Error>),
    Steering(
        TransportAttempt,
        InputSubmission,
        Result<Value, acp::Error>,
        tokio::sync::oneshot::Sender<Result<(), BusinessError>>,
    ),
}
struct ConfigRequest {
    scope: Scope,
    cause: Option<CommandId>,
    steps: VecDeque<(&'static str, String)>,
    done: tokio::sync::oneshot::Sender<Result<(), BusinessError>>,
}
type Prepared = (
    TransportAttempt,
    InputSubmission,
    Result<Vec<Value>, BusinessError>,
    tokio::sync::oneshot::Sender<Result<(), BusinessError>>,
);
fn sdk(error: acp::Error) -> BusinessError {
    sdk_error(&error)
}
fn sdk_error(error: &acp::Error) -> BusinessError {
    if error.code == acp::ErrorCode::AUTH_REQUIRED {
        return crate::error(
            ErrorKind::Authentication,
            "Agent requires official sign-in; authenticate and reconnect",
            RetryAdvice::AfterCorrection,
        );
    }
    if error.code == acp::ErrorCode::METHOD_NOT_FOUND {
        return crate::error(
            ErrorKind::Unsupported,
            "Agent does not support the requested ACP method",
            RetryAdvice::AfterCorrection,
        );
    }
    crate::error(
        ErrorKind::Protocol,
        format!("ACP request failed (code {:?})", error.code),
        RetryAdvice::ReconcileFirst,
    )
}
fn to_sdk(error: BusinessError) -> acp::Error {
    acp::Error::internal_error().data(error.message)
}
fn rpc(
    cx: &v1::ConnectionTo<acp::Agent>,
    method: &str,
    params: impl serde::Serialize,
) -> Result<v1::SentRequest<Value>, acp::Error> {
    Ok(cx.send_request(UntypedMessage::new(method, params)?))
}

pub(super) async fn run(
    work: Work,
    mut commands: mpsc::Receiver<Work>,
    mut auth_commands: mpsc::Receiver<AuthWork>,
    mut stop: watch::Receiver<bool>,
    options: RuntimeOptions,
) {
    let facts = Facts::new(work.effect.scope, work.facts);
    let RuntimeOperation::Connect {
        project,
        agent,
        external,
        resume,
    } = work.effect.operation
    else {
        return;
    };
    let auth_scope = AuthenticationScope::session(
        agent.id,
        work.effect.scope.session_id.unwrap(),
        work.effect.scope.epoch.unwrap(),
    );
    let Some(launch) = agent.launch else {
        let error = error(
            ErrorKind::NotFound,
            "Agent has no configured executable",
            RetryAdvice::AfterCorrection,
        );
        facts
            .final_event(Event::Error {
                error: error.clone(),
            })
            .await;
        facts
            .final_event(Event::ConnectionChanged {
                phase: ConnectionPhase::Failed {
                    reason: error.message.clone(),
                },
            })
            .await;
        facts.final_event(Event::ResourcesReleased).await;
        let _ = work.done.send(Err(error));
        return;
    };
    let process = OwnedProcess::spawn(&launch, &project.directory, &options);
    let (process, transport) = match process {
        Ok(value) => value,
        Err(error) => {
            facts
                .final_event(Event::Error {
                    error: error.clone(),
                })
                .await;
            facts
                .final_event(Event::ConnectionChanged {
                    phase: ConnectionPhase::Failed {
                        reason: error.message.clone(),
                    },
                })
                .await;
            facts.final_event(Event::ResourcesReleased).await;
            let _ = work.done.send(Err(error));
            return;
        }
    };
    let (incoming_tx, mut incoming) = mpsc::channel(options.event_capacity);
    let dispatch_tx = incoming_tx.clone();
    let (auth_tx, mut auth_rx) = mpsc::channel(64);
    let foreground_facts = facts.clone();
    let builder = acp::Client.v1().on_receive_dispatch(
        async move |dispatch: v1::Dispatch, _cx| {
            let item = match dispatch {
                acp::Dispatch::Notification(message) => {
                    Incoming::Notification(message.method().into(), message.params().clone())
                }
                acp::Dispatch::Request(message, responder) => {
                    Incoming::Request(message.method().into(), message.params().clone(), responder)
                }
                other => {
                    return Ok(acp::Handled::No {
                        message: other,
                        retry: false,
                    });
                }
            };
            dispatch_tx.try_send(item).map_err(|_| {
                acp::Error::internal_error().data("bounded Runtime incoming queue exhausted")
            })?;
            Ok(acp::Handled::Yes)
        },
        acp::on_receive_dispatch!(),
    );
    let result=builder.connect_with(transport,async move |cx:v1::ConnectionTo<acp::Agent>| {
        let mut mapper=Mapper::new(foreground_facts);
        let mut authentication=None;
        let setup=async {
            let init=rpc(&cx,"initialize",json!({"protocolVersion":1,"clientInfo":{"name":"native-agent-workbench","version":"0.1.0"},"clientCapabilities":{"elicitation":{"form":{}}}}))?.block_task().await?;
            if init["agentInfo"]["version"]!="2.5.0"||init["agentInfo"]["name"]!="@agentclientprotocol/codex-acp" {return Err(acp::Error::invalid_params().data("this adapter implementation requires verified codex-acp 2.5.0"));}
            mapper.negotiated(&init);
            authentication=Some(AuthenticationSession::new(auth_scope,&init)?);
            let (method,params)=if resume {
                let binding=external.as_ref().ok_or_else(||acp::Error::invalid_params().data("missing registered external session"))?;
                mapper.external=Some(binding.id.as_str().into());
                if mapper.capabilities.session_resume==CapabilitySupport::Supported {
                    ("session/resume",json!({"sessionId":binding.id.as_str(),"cwd":project.directory,"mcpServers":[]}))
                } else if mapper.capabilities.history_load==CapabilitySupport::Supported {
                    mapper.replay=true;
                    ("session/load",json!({"sessionId":binding.id.as_str(),"cwd":project.directory,"mcpServers":[]}))
                } else {return Err(acp::Error::invalid_params().data("Agent cannot resume or load the registered session"));}
            } else {("session/new",json!({"cwd":project.directory,"mcpServers":[]}))};
            let session=rpc(&cx,method,params)?.block_task().await?;
            let id=if resume {mapper.external.clone().unwrap()}else{session["sessionId"].as_str().ok_or_else(||acp::Error::internal_error().data("session/new returned no identity"))?.into()};
            mapper.external=Some(id.clone());
            mapper.facts.emit(mapper.session_scope(),Event::SessionRegistered {binding:ExternalSessionBinding::new(ExternalSessionId::new(id),crate::now()),resumed:resume},command(&work.effect.cause)).map_err(to_sdk)?;
            mapper.configuration(&session["configOptions"]).map_err(to_sdk)?;
            // Drain updates received during setup before a new live prompt is possible.
            while let Ok(item)=incoming.try_recv() {match item {Incoming::Notification(method,params) if method=="session/update"=>mapper.notification(params).map_err(to_sdk)?,Incoming::Request(_,_,responder)=>{let _=responder.respond_with_error(acp::Error::request_cancelled());},_=>{}}}
            mapper.replay=false;
            if !resume {
                if let Some(full)=mapper.capabilities.permission_modes.iter().find(|p|p.mode==AccessMode::FullAccess).map(|p|p.option.id.as_str().to_owned()) {
                    let configured=rpc(&cx,"session/set_config_option",json!({"sessionId":mapper.external,"configId":"mode","value":full}))?.block_task().await?;
                    mapper.configuration(&configured["configOptions"]).map_err(to_sdk)?;
                } else {mapper.facts.emit(mapper.session_scope(),Event::Error {error:error(ErrorKind::Unsupported,"Agent did not offer the requested initial full-access mode",RetryAdvice::AfterCorrection)},None).map_err(to_sdk)?;}
            }
            mapper.facts.emit(mapper.session_scope(),Event::ConnectionChanged {phase:ConnectionPhase::Ready},None).map_err(to_sdk)?;
            Ok(())
        };
        let mut setup=Box::pin(tokio::time::timeout(options.setup_timeout,setup));
        loop {tokio::select! {
            result=&mut setup=>{result.map_err(|_|acp::Error::internal_error().data("ACP setup timed out"))??;break;}
            _=stop.changed()=>return Ok(()),
            work=commands.recv()=>{let Some(work)=work else {return Ok(())};
                if matches!(work.effect.operation,RuntimeOperation::ReleaseSession) {let _=work.done.send(Ok(()));return Ok(());}
                crate::reject(work,error(ErrorKind::NotReady,"connection setup is not complete",RetryAdvice::AfterReconnect));
            }
        }}
        drop(setup);
        let mut authentication=authentication.ok_or_else(||acp::Error::internal_error().data("authentication metadata missing after initialize"))?;
        authentication.verified_login();let mut auth_open=true;
        let mut evidence:BTreeMap<AttemptId,ConsumptionEvidence>=BTreeMap::new();
        let mut cancelled:BTreeSet<RunId>=BTreeSet::new();
        let mut sent=BTreeSet::new();
        // Transport preparation preserves committed BEGIN order; this is not the user queue.
        let mut preparation_order=VecDeque::new();let mut prepared=BTreeMap::new();
        let mut preparation_cancel:BTreeMap<AttemptId,(RunId,tokio::sync::oneshot::Sender<()>)>=BTreeMap::new();
        let mut responders=Responders::default();
        let mut withdrawal=tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            tokio::select! {
                _=stop.changed()=>break,
                _=withdrawal.tick()=>{responders.withdrawn(&mapper).map_err(to_sdk)?;authentication.tick().map_err(to_sdk)?;}
                item=auth_rx.recv()=>{if let Some(item)=item {authentication.incoming(item).map_err(to_sdk)?;}}
                auth=auth_commands.recv(),if auth_open=>{
                    match auth {Some(work)=>authentication.action(work,&cx,&auth_tx,mapper.run_scope().is_some()).await.map_err(to_sdk)?,None=>auth_open=false}
                }
                item=incoming.recv()=>{
                    match item {
                        Some(Incoming::Notification(method,params)) if method=="session/update"=>mapper.notification(params).map_err(to_sdk)?,
                        Some(Incoming::Notification(method,params)) if method=="elicitation/complete"=>authentication.incoming(AuthIncoming::Notification(method,params)).map_err(to_sdk)?,
                        Some(Incoming::Notification(method,params))=>mapper.unsupported(&method,&params).map_err(to_sdk)?,
                        Some(Incoming::Request(method,params,responder)) if method=="elicitation/create"&&params.get("requestId").is_some()=>authentication.incoming(AuthIncoming::Request(method,params,responder)).map_err(to_sdk)?,
                        Some(Incoming::Request(method,params,responder))=>responders.receive(&mut mapper,&method,params,responder).map_err(to_sdk)?,
                        Some(Incoming::Prepared(attempt,submission,content,done))=>{
                            preparation_cancel.remove(&attempt.id);prepared.insert(attempt.id,(attempt,submission,content,done));
                            dispatch_ready(&cx,&mut mapper,&incoming_tx,&mut preparation_order,&mut prepared,&cancelled,&mut sent)?;
                        }
                        Some(Incoming::Config(request,response))=>{
                            match response {
                                Ok(response)=>{
                                    let revision=mapper.configuration(&response["configOptions"]).map_err(to_sdk)?;
                                    if request.steps.is_empty() {
                                        mapper.facts.emit(request.scope,Event::ConfigurationChangeSettled {reported_revision:revision},request.cause).map_err(to_sdk)?;
                                        let _=request.done.send(Ok(()));
                                    } else {start_config(&cx,&mapper,&incoming_tx,request)?;}
                                }
                                Err(_)=>{
                                    mapper.facts.emit(request.scope,Event::ConfigurationRejected {reason:"Agent rejected the requested configuration".into()},request.cause).map_err(to_sdk)?;
                                    let _=request.done.send(Ok(()));
                                }
                            }
                        }
                        Some(Incoming::Steering(attempt,submission,response,done))=>{
                            let scope=mapper.session_scope().with_run(attempt.context.run_id).with_request(attempt.request_id);
                            let receipt=match response {
                                Ok(response)=>match response["outcome"].as_str() {
                                    Some("injected")=>SteeringReceipt::Injected,
                                    Some("promptRequired") if response["reason"]=="noRunningTurn"=>SteeringReceipt::PromptRequired,
                                    Some("startedNewTurn")=>SteeringReceipt::StartedNewTurn {local_run_id:RunId::new()},
                                    _=>SteeringReceipt::Failed {reason:"steering did not return verified consumption evidence".into()},
                                },
                                Err(_)=>SteeringReceipt::Failed {reason:"steering RPC failed; consumption is unknown".into()},
                            };
                            if matches!(receipt,SteeringReceipt::StartedNewTurn {..}) {mapper.complete_messages().map_err(to_sdk)?;}
                            mapper.facts.emit(scope,Event::Attempt {attempt_id:attempt.id,outcome:AttemptOutcome::Steering {receipt:receipt.clone()}},Some(attempt.command_id)).map_err(to_sdk)?;
                            match receipt {
                                SteeringReceipt::Injected=>mapper.steered_user_message(&submission,attempt.context.run_id).map_err(to_sdk)?,
                                SteeringReceipt::StartedNewTurn {local_run_id}=>{mapper.start_adapter_turn(attempt,submission,local_run_id).map_err(to_sdk)?;}
                                _=>{}
                            }
                            let _=done.send(Ok(()));
                        }
                        Some(Incoming::Prompt(attempt,response,done))=>{
                            let scope=mapper.session_scope().with_run(attempt.context.run_id).with_request(attempt.request_id);
                            let outcome=match response {
                                Ok(response)=>{
                                    let end=run_end(&response);
                                    if mapper.active.as_ref().is_some_and(|a|a.id==attempt.id) {
                                        mapper.accepted().map_err(to_sdk)?;mapper.complete_messages().map_err(to_sdk)?;
                                    }
                                    evidence.insert(attempt.id,ConsumptionEvidence::Consumed {run_id:attempt.context.run_id,end:Some(end.clone())});
                                    AttemptOutcome::PromptReturned {end}
                                }
                                Err(_)=>AttemptOutcome::Unknown {reason:"prompt returned an error without reliable consumption evidence".into()},
                            };
                            mapper.facts.emit(scope,Event::Attempt {attempt_id:attempt.id,outcome},Some(attempt.command_id)).map_err(to_sdk)?;
                            mapper.finish_attempt(attempt.id);
                            let _=done.send(Ok(()));
                        }
                        None=>break,
                    }
                }
                work=commands.recv()=>{
                    let Some(work)=work else {break};
                    let scope=work.effect.scope;
                    let cause=command(&work.effect.cause);
                    let external=mapper.external.as_deref().unwrap().to_owned();
                    match work.effect.operation {
                        RuntimeOperation::ReleaseSession=>{responders.cancel_all();let _=cx.send_notification(UntypedMessage::new("session/cancel",json!({"sessionId":external}))?);let _=work.done.send(Ok(()));break;}
                        RuntimeOperation::Submit {attempt,submission}=>{
                            if authentication.is_in_progress() {
                                let reason=error(ErrorKind::Authentication,"official authentication must finish before dispatch",RetryAdvice::AfterCorrection);
                                mapper.facts.emit(scope,Event::Attempt {attempt_id:attempt.id,outcome:AttemptOutcome::NotSent {reason:reason.message.clone()}},cause).map_err(to_sdk)?;
                                let _=work.done.send(Err(reason));continue;
                            }
                            if matches!(attempt.method,TransportMethod::Prompt) {mapper.begin(attempt.clone(),submission.clone());}
                            preparation_order.push_back(attempt.id);
                            if !submission.content.iter().any(|part|matches!(part,InputPart::Attachment {..})) {
                                // No asynchronous I/O in this path: start the request before
                                // receiving the following committed Cancel or steering effect.
                                let content=prompt_content(&submission,&options,&project.directory).await;
                                prepared.insert(attempt.id,(attempt,submission,content,work.done));
                                dispatch_ready(&cx,&mut mapper,&incoming_tx,&mut preparation_order,&mut prepared,&cancelled,&mut sent)?;
                            } else {
                                let (cancel_tx,cancel_rx)=tokio::sync::oneshot::channel();preparation_cancel.insert(attempt.id,(attempt.context.run_id,cancel_tx));
                                let tx=incoming_tx.clone();let options=options.clone();let cwd=project.directory.clone();
                                cx.spawn(async move {
                                    let content=tokio::select! {
                                        content=prompt_content(&submission,&options,&cwd)=>content,
                                        _=cancel_rx=>Err(error(ErrorKind::Rejected,"attachment preparation was stopped before dispatch",RetryAdvice::Never)),
                                    };
                                    tx.send(Incoming::Prepared(attempt,submission,content,work.done)).await.map_err(|_|acp::Error::internal_error().data("input preparation owner was closed"))?;Ok(())
                                })?;
                            }
                        }
                        RuntimeOperation::Cancel=>{
                            if let Some(run)=scope.run_id {cancelled.insert(run);}
                            let reads=preparation_cancel.iter().filter(|(_,entry)|Some(entry.0)==scope.run_id).map(|(id,_)|*id).collect::<Vec<_>>();
                            for id in reads {if let Some((_,cancel))=preparation_cancel.remove(&id){let _=cancel.send(());}}
                            let preparing=mapper.active.as_ref().is_some_and(|attempt|!sent.contains(&attempt.id));
                            let result=if preparing {Ok(())}else{cx.send_notification(UntypedMessage::new("session/cancel",json!({"sessionId":external}))?).map_err(sdk)};
                            let _=work.done.send(result);
                        }
                        RuntimeOperation::CancelRequests {ids}=>{responders.cancel(&ids);let _=work.done.send(Ok(()));}
                        RuntimeOperation::RespondPermission {context,response,attempt_id}=>{let result=responders.permission(&mapper,&context,response,attempt_id);let _=work.done.send(result);}
                        RuntimeOperation::RespondQuestion {context,response,attempt_id}=>{let result=responders.question(&mapper,&context,response,attempt_id);let _=work.done.send(result);}
                        RuntimeOperation::ReconcileSubmission {submission_id,attempt}=>{
                            mapper.facts.emit(scope,Event::SubmissionReconciled {submission_id,attempt_id:attempt.id,attempt_epoch:attempt.epoch,
                                evidence:evidence.get(&attempt.id).cloned().unwrap_or(ConsumptionEvidence::Unknown {reason:"no authoritative evidence for the original attempt".into()})},cause).map_err(to_sdk)?;
                            let _=work.done.send(Ok(()));
                        }
                        RuntimeOperation::Configure {pending}=>{
                            if authentication.is_in_progress() {
                                mapper.facts.emit(scope,Event::ConfigurationRejected {reason:"official authentication must finish before configuration".into()},cause).map_err(to_sdk)?;
                                let _=work.done.send(Ok(()));continue;
                            }
                            match config_steps(&mapper,&pending) {
                                Ok(steps)=>{let request=ConfigRequest {scope,cause,steps,done:work.done};start_config(&cx,&mapper,&incoming_tx,request)?;}
                                Err(_)=>{mapper.facts.emit(scope,Event::ConfigurationRejected {reason:"requested option is no longer available".into()},cause).map_err(to_sdk)?;let _=work.done.send(Ok(()));}
                            }
                        }
                        _=>{let _=work.done.send(Err(error(ErrorKind::Unsupported,"operation is not available on this connection",RetryAdvice::Never)));}
                    }
                }
            }
        }
        responders.cancel_all();
        authentication.close();
        Ok(())
    }).await;
    // Cleanup must not wait behind a stalled app event sink.
    let cleanup = process.close().await;
    if let Err(failure) = &result {
        facts
            .final_event(Event::Error {
                error: sdk_error(failure),
            })
            .await;
        facts
            .final_event(Event::ConnectionChanged {
                phase: ConnectionPhase::Disconnected {
                    reason: "ACP transport or protocol failed".into(),
                },
            })
            .await;
    }
    if cleanup.is_ok() {
        facts.final_event(Event::ResourcesReleased).await;
    } else if let Err(error) = &cleanup {
        facts
            .final_event(Event::Error {
                error: error.clone(),
            })
            .await;
    }
    let _ = work.done.send(result.map_err(sdk).and(cleanup));
}

fn config_steps(
    mapper: &Mapper,
    pending: &PendingConfiguration,
) -> Result<VecDeque<(&'static str, String)>, acp::Error> {
    let permission = match &pending.requested.permission {
        PermissionSelection::AgentOption { id } => id.as_str().to_owned(),
        PermissionSelection::DefaultFullAccess => mapper
            .capabilities
            .permission_modes
            .iter()
            .find(|p| p.mode == AccessMode::FullAccess)
            .map(|p| p.option.id.as_str().to_owned())
            .ok_or_else(|| acp::Error::invalid_params().data("full access was not offered"))?,
    };
    let mut requested = Vec::new();
    if let Some(id) = &pending.requested.model {
        requested.push(("model", id.as_str().to_owned()));
    }
    if let Some(id) = &pending.requested.reasoning_effort {
        requested.push(("reasoning_effort", id.as_str().to_owned()));
    }
    requested.push(("mode", permission));
    Ok(requested.into())
}
fn dispatch_ready(
    cx: &v1::ConnectionTo<acp::Agent>,
    mapper: &mut Mapper,
    tx: &mpsc::Sender<Incoming>,
    order: &mut VecDeque<AttemptId>,
    ready: &mut BTreeMap<AttemptId, Prepared>,
    cancelled: &BTreeSet<RunId>,
    sent: &mut BTreeSet<AttemptId>,
) -> Result<(), acp::Error> {
    while let Some(id) = order.front() {
        let Some((attempt, submission, content, done)) = ready.remove(id) else {
            break;
        };
        order.pop_front();
        let scope = mapper
            .session_scope()
            .with_run(attempt.context.run_id)
            .with_request(attempt.request_id);
        let content = if cancelled.contains(&attempt.context.run_id) {
            Err(error(
                ErrorKind::Rejected,
                "input was stopped before dispatch",
                RetryAdvice::Never,
            ))
        } else {
            content
        };
        match content {
            Err(error) => {
                mapper
                    .facts
                    .emit(
                        scope,
                        Event::Attempt {
                            attempt_id: attempt.id,
                            outcome: AttemptOutcome::NotSent {
                                reason: error.message.clone(),
                            },
                        },
                        Some(attempt.command_id),
                    )
                    .map_err(to_sdk)?;
                mapper.finish_attempt(attempt.id);
                let _ = done.send(Err(error));
            }
            Ok(content) => {
                let tx = tx.clone();
                let pending = attempt.clone();
                if matches!(attempt.method, TransportMethod::Prompt) {
                    sent.insert(attempt.id);
                    rpc(
                        cx,
                        "session/prompt",
                        json!({"sessionId":mapper.external,"prompt":content}),
                    )?
                    .on_receiving_result(async move |response| {
                        tx.try_send(Incoming::Prompt(pending, response, done))
                            .map_err(|_| {
                                acp::Error::internal_error()
                                    .data("prompt completion queue exhausted")
                            })?;
                        Ok(())
                    })?;
                } else {
                    rpc(cx,"_session/steering",json!({"sessionId":mapper.external,"prompt":content,"_meta":{"steering":{"idleBehavior":"promptRequired"}}}))?.on_receiving_result(async move |response| {tx.try_send(Incoming::Steering(pending,submission,response,done)).map_err(|_|acp::Error::internal_error().data("steering completion queue exhausted"))?;Ok(())})?;
                }
            }
        }
    }
    Ok(())
}
fn start_config(
    cx: &v1::ConnectionTo<acp::Agent>,
    mapper: &Mapper,
    incoming: &mpsc::Sender<Incoming>,
    mut request: ConfigRequest,
) -> Result<(), acp::Error> {
    let (id, value) = request
        .steps
        .pop_front()
        .ok_or_else(|| acp::Error::invalid_params().data("empty config change"))?;
    if id == "reasoning_effort"
        && !mapper.capabilities.models.iter().any(|model| {
            mapper.effective.model.as_ref() == Some(&model.option.id)
                && model
                    .reasoning_efforts
                    .iter()
                    .any(|option| option.id.as_str() == value)
        })
    {
        mapper
            .facts
            .emit(
                request.scope,
                Event::ConfigurationRejected {
                    reason: "requested effort is unavailable for the newly confirmed model".into(),
                },
                request.cause,
            )
            .map_err(to_sdk)?;
        let _ = request.done.send(Ok(()));
        return Ok(());
    }
    let tx = incoming.clone();
    rpc(
        cx,
        "session/set_config_option",
        json!({"sessionId":mapper.external,"configId":id,"value":value}),
    )?
    .on_receiving_result(async move |result| {
        tx.try_send(Incoming::Config(request, result))
            .map_err(|_| acp::Error::internal_error().data("config completion queue exhausted"))?;
        Ok(())
    })
}
fn run_end(response: &Value) -> RunEnd {
    match response["stopReason"].as_str() {
        Some("end_turn") => RunEnd::Completed {
            result: RunResult::EndOfTurn,
        },
        Some("cancelled") => RunEnd::Cancelled,
        Some("max_tokens" | "max_turn_requests") => RunEnd::Completed {
            result: RunResult::LimitReached {
                reason: response["stopReason"].as_str().unwrap().into(),
            },
        },
        Some("refusal") => RunEnd::Failed {
            reason: "Agent refused this request".into(),
        },
        _ => RunEnd::Failed {
            reason: "Agent reported an unsupported end reason".into(),
        },
    }
}
async fn prompt_content(
    submission: &InputSubmission,
    options: &RuntimeOptions,
    cwd: &std::path::Path,
) -> Result<Vec<Value>, BusinessError> {
    use base64::Engine;
    let mut content = Vec::new();
    for part in &submission.content {
        match part {
            InputPart::Text { text } => content.push(json!({"type":"text","text":text})),
            InputPart::AgentCommand {
                command_id,
                arguments,
            } => content.push(
                json!({"type":"text","text":format!("/{} {}",command_id.as_str(),arguments)}),
            ),
            InputPart::FileReference { reference } => {
                let path = if reference.path.is_absolute() {
                    reference.path.clone()
                } else {
                    cwd.join(&reference.path)
                };
                content.push(json!({"type":"resource_link","uri":file_uri(&path),"name":path.file_name().unwrap_or_default().to_string_lossy()}));
                if let Some(lines) = reference.lines {
                    content.push(json!({"type":"text","text":format!("Selected source lines {} through {} in the referenced file.",lines.start(),lines.end())}));
                }
            }
            InputPart::Attachment { attachment_id } => {
                let resolver = options.attachments.as_ref().ok_or_else(|| {
                    error(
                        ErrorKind::Unsupported,
                        "application attachment resolver is not connected",
                        RetryAdvice::AfterCorrection,
                    )
                })?;
                let (metadata, bytes) = resolver
                    .resolve(submission.session_id, *attachment_id)
                    .await?;
                if metadata.id != *attachment_id || metadata.session_id != submission.session_id {
                    return Err(error(
                        ErrorKind::StaleIdentity,
                        "attachment owner does not match submission",
                        RetryAdvice::Never,
                    ));
                }
                if !matches!(metadata.kind, AttachmentKind::Image { .. }) {
                    return Err(error(
                        ErrorKind::Unsupported,
                        "embedded file attachment format is not supported",
                        RetryAdvice::AfterCorrection,
                    ));
                }
                content.push(json!({"type":"image","mimeType":metadata.media_type,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
            }
        }
    }
    Ok(content)
}
fn file_uri(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from(if path.starts_with("//") {
        "file:"
    } else if path.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            uri.push(byte as char)
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}
