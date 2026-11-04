//! Transient official authentication. No URL, responder or credential enters a journal.
use crate::{RuntimeOptions, error, process::OwnedProcess};
use agent_client_protocol::{self as acp, v1};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
use workbench_core::*;

pub(crate) struct AuthWork {
    pub request: AuthenticationRequest,
    pub notices: mpsc::Sender<AuthenticationNotice>,
    pub done: oneshot::Sender<Result<(), BusinessError>>,
}
pub(crate) enum AuthIncoming {
    Request(String, Value, v1::Responder),
    Notification(String, Value),
    Finished(RequestId, Result<(), ()>),
}
struct Active {
    id: RequestId,
    method: AuthenticationMethodId,
    wire_id: Value,
    cancellation: acp::RequestCancellationHandle,
    notices: mpsc::Sender<AuthenticationNotice>,
    done: oneshot::Sender<Result<(), BusinessError>>,
    started: Instant,
    cancelled: bool,
}
struct Interaction {
    url: Option<String>,
    elicitation_id: Option<String>,
    responder: v1::Responder,
}
pub(crate) struct AuthenticationSession {
    scope: AuthenticationScope,
    methods: Vec<AuthenticationMethod>,
    verified_login: bool,
    active: Option<Active>,
    interactions: BTreeMap<AuthenticationInteractionId, Interaction>,
}
impl AuthenticationSession {
    pub fn new(scope: AuthenticationScope, init: &Value) -> Result<Self, acp::Error> {
        let typed: v1::schema::InitializeResponse = serde_json::from_value(init.clone())?;
        let methods = typed
            .auth_methods
            .into_iter()
            .map(|method| {
                let (id, name, description, kind) = match method {
                    v1::schema::AuthMethod::Agent(method) => (
                        method.id.to_string(),
                        method.name,
                        method.description,
                        AuthenticationMethodKind::AgentManaged,
                    ),
                    v1::schema::AuthMethod::Terminal(method) => (
                        method.id.to_string(),
                        method.name,
                        method.description,
                        AuthenticationMethodKind::Terminal,
                    ),
                    _ => {
                        return AuthenticationMethod::new(
                            AuthenticationMethodId::new("unsupported"),
                            "Unsupported authentication method",
                            AuthenticationMethodKind::Unsupported,
                            AuthenticationMethodAvailability::Unavailable {
                                reason: AuthenticationUnavailableReason::UnsupportedMethodKind,
                            },
                        );
                    }
                };
                let availability = if kind != AuthenticationMethodKind::AgentManaged {
                    AuthenticationMethodAvailability::Unavailable {
                        reason: AuthenticationUnavailableReason::UnsupportedMethodKind,
                    }
                } else if id == "api-key" {
                    AuthenticationMethodAvailability::Unavailable {
                        reason: AuthenticationUnavailableReason::SecretEntryUnsupported,
                    }
                } else {
                    AuthenticationMethodAvailability::Available
                };
                let mut public = AuthenticationMethod::new(
                    AuthenticationMethodId::new(id),
                    name,
                    kind,
                    availability,
                );
                public.description = description;
                public
            })
            .collect();
        Ok(Self {
            scope,
            methods,
            verified_login: false,
            active: None,
            interactions: BTreeMap::new(),
        })
    }
    pub fn verified_login(&mut self) {
        self.verified_login = true;
    }
    pub fn is_in_progress(&self) -> bool {
        self.active.is_some()
    }
    fn notice(&self, work: &AuthWork, update: AuthenticationUpdate) -> Result<(), BusinessError> {
        work.notices
            .try_send(AuthenticationNotice::new(self.scope, update).with_request(work.request.id))
            .map_err(|_| {
                error(
                    ErrorKind::Exhausted,
                    "authentication notice receiver is full or closed",
                    RetryAdvice::AfterCorrection,
                )
            })
    }
    fn active_notice(&self, update: AuthenticationUpdate) -> Result<(), BusinessError> {
        let Some(active) = &self.active else {
            return Ok(());
        };
        active
            .notices
            .try_send(AuthenticationNotice::new(self.scope, update).with_request(active.id))
            .map_err(|_| {
                error(
                    ErrorKind::Exhausted,
                    "authentication notice receiver is full or closed",
                    RetryAdvice::AfterCorrection,
                )
            })
    }
    pub async fn action(
        &mut self,
        work: AuthWork,
        cx: &v1::ConnectionTo<acp::Agent>,
        incoming: &mpsc::Sender<AuthIncoming>,
        busy: bool,
    ) -> Result<(), BusinessError> {
        if work.request.scope != self.scope {
            fail(work, AuthenticationFailure::StaleScope);
            return Ok(());
        }
        match work.request.action.clone() {
            AuthenticationAction::Discover => {
                self.notice(
                    &work,
                    AuthenticationUpdate::Methods {
                        methods: self.methods.clone(),
                    },
                )?;
                self.notice(
                    &work,
                    AuthenticationUpdate::Status {
                        status: if self.verified_login {
                            AuthenticationStatus::Authenticated
                        } else {
                            AuthenticationStatus::Unknown
                        },
                    },
                )?;
                let _ = work.done.send(Ok(()));
            }
            AuthenticationAction::Authenticate { method_id } => {
                let Some(method) = self.methods.iter().find(|method| method.id == method_id) else {
                    fail(work, AuthenticationFailure::MethodNotOffered);
                    return Ok(());
                };
                if method.availability != AuthenticationMethodAvailability::Available
                    || busy
                    || self.active.is_some()
                {
                    fail(work, AuthenticationFailure::MethodUnavailable);
                    return Ok(());
                }
                self.notice(
                    &work,
                    AuthenticationUpdate::Status {
                        status: AuthenticationStatus::InProgress {
                            method_id: method_id.clone(),
                        },
                    },
                )?;
                let sent = cx.send_request(v1::schema::AuthenticateRequest::new(
                    v1::schema::AuthMethodId::new(method_id.as_str()),
                ));
                let wire_id = serde_json::to_value(sent.raw_id()).map_err(|_| {
                    error(
                        ErrorKind::Protocol,
                        "could not retain authentication request identity",
                        RetryAdvice::Never,
                    )
                })?;
                let cancellation = sent.cancellation_handle();
                let id = work.request.id;
                let tx = incoming.clone();
                self.active = Some(Active {
                    id,
                    method: method_id,
                    wire_id,
                    cancellation,
                    notices: work.notices,
                    done: work.done,
                    started: Instant::now(),
                    cancelled: false,
                });
                sent.on_receiving_result(async move |result| {
                    tx.try_send(AuthIncoming::Finished(
                        id,
                        result.map(|_| ()).map_err(|_| ()),
                    ))
                    .map_err(|_| {
                        acp::Error::internal_error()
                            .data("authentication completion receiver closed")
                    })?;
                    Ok(())
                })
                .map_err(|_| {
                    error(
                        ErrorKind::Transport,
                        "could not begin official authentication",
                        RetryAdvice::AfterReconnect,
                    )
                })?;
            }
            AuthenticationAction::Cancel { request_id } => {
                if !self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.id == request_id)
                {
                    fail(work, AuthenticationFailure::InteractionExpired);
                    return Ok(());
                }
                if let Some(active) = &mut self.active {
                    active.cancelled = true;
                    let _ = active.cancellation.cancel();
                }
                self.close_interactions(AuthenticationInteractionEnd::Cancelled)?;
                self.active_notice(AuthenticationUpdate::Status {
                    status: AuthenticationStatus::Cancelled,
                })?;
                let _ = work.done.send(Ok(()));
            }
            AuthenticationAction::Interact {
                interaction_id,
                action,
            } => {
                if !self.interactions.contains_key(&interaction_id) {
                    fail(work, AuthenticationFailure::InteractionExpired);
                    return Ok(());
                }
                if action == AuthenticationInteractionAction::OpenOfficialFlow {
                    let Some(url) = self.interactions[&interaction_id].url.clone() else {
                        fail(work, AuthenticationFailure::MethodUnavailable);
                        return Ok(());
                    };
                    if !url.starts_with("https://") {
                        fail(work, AuthenticationFailure::OfficialFlowFailed);
                        return Ok(());
                    }
                    // The OS launcher runs off the SDK/UI thread; its URL is never logged.
                    let opened = tokio::task::spawn_blocking(move || open::that(url)).await;
                    if !matches!(opened, Ok(Ok(()))) {
                        fail(work, AuthenticationFailure::OfficialFlowFailed);
                        return Ok(());
                    }
                }
                let Some(interaction) = self.interactions.remove(&interaction_id) else {
                    fail(work, AuthenticationFailure::InteractionExpired);
                    return Ok(());
                };
                if interaction.responder.cancellation().is_cancelled() {
                    self.active_notice(AuthenticationUpdate::InteractionClosed {
                        interaction_id,
                        reason: AuthenticationInteractionEnd::Expired,
                    })?;
                    fail(work, AuthenticationFailure::InteractionExpired);
                    return Ok(());
                }
                let (action, reason) = match action {
                    AuthenticationInteractionAction::OpenOfficialFlow => {
                        ("accept", AuthenticationInteractionEnd::Completed)
                    }
                    AuthenticationInteractionAction::Decline => {
                        ("decline", AuthenticationInteractionEnd::Declined)
                    }
                    AuthenticationInteractionAction::Cancel => {
                        ("cancel", AuthenticationInteractionEnd::Cancelled)
                    }
                };
                let delivered = interaction
                    .responder
                    .respond(json!({"action":action}))
                    .is_ok();
                self.active_notice(AuthenticationUpdate::InteractionClosed {
                    interaction_id,
                    reason,
                })?;
                if delivered {
                    let _ = work.done.send(Ok(()));
                } else {
                    fail(work, AuthenticationFailure::Transport);
                }
            }
        }
        Ok(())
    }
    pub fn incoming(&mut self, item: AuthIncoming) -> Result<(), BusinessError> {
        match item {
            AuthIncoming::Request(method, params, responder) => {
                if method != "elicitation/create" {
                    let _ = responder.respond_with_error(acp::Error::method_not_found());
                    return Ok(());
                }
                let Some(active) = &self.active else {
                    let _ = responder.respond_with_error(acp::Error::request_cancelled());
                    return Ok(());
                };
                if params["requestId"] != active.wire_id || active.cancelled {
                    let _ = responder.respond_with_error(acp::Error::request_cancelled());
                    return Ok(());
                }
                let official = params["mode"] == "url";
                let id = AuthenticationInteractionId::new();
                let url = if official {
                    params["url"].as_str().map(str::to_owned)
                } else {
                    None
                };
                let mut public = AuthenticationInteraction::new(
                    id,
                    if official {
                        AuthenticationInteractionKind::OfficialUrl
                    } else {
                        AuthenticationInteractionKind::UnsupportedSecret
                    },
                );
                public.method_id = Some(active.method.clone());
                self.interactions.insert(
                    id,
                    Interaction {
                        url,
                        elicitation_id: params["elicitationId"].as_str().map(str::to_owned),
                        responder,
                    },
                );
                self.active_notice(AuthenticationUpdate::InteractionRequired {
                    interaction: public,
                })?;
            }
            AuthIncoming::Notification(method, params) if method == "elicitation/complete" => {
                let ids = self
                    .interactions
                    .iter()
                    .filter(|(_, interaction)| {
                        interaction.elicitation_id.as_deref() == params["elicitationId"].as_str()
                    })
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();
                for id in ids {
                    if let Some(interaction) = self.interactions.remove(&id) {
                        let _ = interaction.responder.respond(json!({"action":"cancel"}));
                        self.active_notice(AuthenticationUpdate::InteractionClosed {
                            interaction_id: id,
                            reason: AuthenticationInteractionEnd::Completed,
                        })?;
                    }
                }
            }
            AuthIncoming::Notification(_, _) => {}
            AuthIncoming::Finished(id, result) => {
                if !self.active.as_ref().is_some_and(|active| active.id == id) {
                    return Ok(());
                }
                self.close_interactions(AuthenticationInteractionEnd::Completed)?;
                let active = self.active.take().unwrap();
                let status = if active.cancelled {
                    AuthenticationStatus::Cancelled
                } else if result.is_ok() {
                    self.verified_login = true;
                    AuthenticationStatus::Authenticated
                } else {
                    AuthenticationStatus::Failed {
                        failure: AuthenticationFailure::ProviderRejected,
                    }
                };
                let _ = active.notices.try_send(
                    AuthenticationNotice::new(self.scope, AuthenticationUpdate::Status { status })
                        .with_request(id),
                );
                let _ = active.done.send(if result.is_ok() || active.cancelled {
                    Ok(())
                } else {
                    Err(error(
                        ErrorKind::Authentication,
                        "official authentication did not complete",
                        RetryAdvice::AfterCorrection,
                    ))
                });
            }
        }
        Ok(())
    }
    pub fn tick(&mut self) -> Result<(), BusinessError> {
        let ids = self
            .interactions
            .iter()
            .filter(|(_, interaction)| interaction.responder.cancellation().is_cancelled())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(interaction) = self.interactions.remove(&id) {
                self.active_notice(AuthenticationUpdate::InteractionClosed {
                    interaction_id: id,
                    reason: AuthenticationInteractionEnd::Expired,
                })?;
                let _ = interaction.responder.respond(json!({"action":"cancel"}));
            }
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.started.elapsed() > Duration::from_secs(300))
        {
            self.close_interactions(AuthenticationInteractionEnd::Expired)?;
            if let Some(active) = self.active.take() {
                let _ = active.cancellation.cancel();
                let _ = active.notices.try_send(
                    AuthenticationNotice::new(
                        self.scope,
                        AuthenticationUpdate::Status {
                            status: AuthenticationStatus::Failed {
                                failure: AuthenticationFailure::Timeout,
                            },
                        },
                    )
                    .with_request(active.id),
                );
                let _ = active.done.send(Err(error(
                    ErrorKind::Authentication,
                    "official authentication timed out",
                    RetryAdvice::AfterCorrection,
                )));
            }
        }
        Ok(())
    }
    fn close_interactions(
        &mut self,
        reason: AuthenticationInteractionEnd,
    ) -> Result<(), BusinessError> {
        let interactions = std::mem::take(&mut self.interactions);
        for (id, interaction) in interactions {
            self.active_notice(AuthenticationUpdate::InteractionClosed {
                interaction_id: id,
                reason,
            })?;
            let _ = interaction.responder.respond(json!({"action":"cancel"}));
        }
        Ok(())
    }
    pub fn close(&mut self) {
        let _ = self.close_interactions(AuthenticationInteractionEnd::ConnectionClosed);
        if let Some(active) = self.active.take() {
            let _ = active.cancellation.cancel();
            let _ = active.notices.try_send(
                AuthenticationNotice::new(
                    self.scope,
                    AuthenticationUpdate::Status {
                        status: AuthenticationStatus::Failed {
                            failure: AuthenticationFailure::NoLiveConnection,
                        },
                    },
                )
                .with_request(active.id),
            );
            let _ = active.done.send(Err(error(
                ErrorKind::Authentication,
                "authentication connection closed",
                RetryAdvice::AfterReconnect,
            )));
        }
    }
}
impl Drop for AuthenticationSession {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) fn fail(work: AuthWork, failure: AuthenticationFailure) {
    let _ = work.notices.try_send(
        AuthenticationNotice::new(
            work.request.scope,
            AuthenticationUpdate::Status {
                status: AuthenticationStatus::Failed { failure },
            },
        )
        .with_request(work.request.id),
    );
    let _ = work.done.send(Err(error(
        ErrorKind::Authentication,
        "authentication operation is unavailable on this connection",
        RetryAdvice::AfterCorrection,
    )));
}

pub(crate) async fn run(
    first: AuthWork,
    mut commands: mpsc::Receiver<AuthWork>,
    mut stop: watch::Receiver<bool>,
    options: RuntimeOptions,
    cached: Option<AgentDefinition>,
) {
    let scope = first.request.scope;
    let agent = if let Some(resolver) = &options.agents {
        resolver.resolve(scope.agent_id()).await
    } else {
        cached.ok_or_else(|| {
            error(
                ErrorKind::NotFound,
                "Agent launch resolver is not connected",
                RetryAdvice::AfterCorrection,
            )
        })
    };
    let agent = match agent {
        Ok(agent) if agent.id == scope.agent_id() => agent,
        _ => {
            fail(first, AuthenticationFailure::NoLiveConnection);
            return;
        }
    };
    let Some(launch) = agent.launch else {
        fail(first, AuthenticationFailure::NoLiveConnection);
        return;
    };
    let spawned = OwnedProcess::spawn(&launch, &std::env::temp_dir(), &options);
    let (process, transport) = match spawned {
        Ok(pair) => pair,
        Err(_) => {
            fail(first, AuthenticationFailure::Transport);
            return;
        }
    };
    let (tx, mut rx) = mpsc::channel(64);
    let callback_tx = tx.clone();
    let builder = acp::Client.v1().on_receive_dispatch(
        async move |dispatch: v1::Dispatch, _| {
            let item = match dispatch {
                acp::Dispatch::Request(message, responder) => AuthIncoming::Request(
                    message.method().into(),
                    message.params().clone(),
                    responder,
                ),
                acp::Dispatch::Notification(message) => {
                    AuthIncoming::Notification(message.method().into(), message.params().clone())
                }
                other => {
                    return Ok(acp::Handled::No {
                        message: other,
                        retry: false,
                    });
                }
            };
            callback_tx.try_send(item).map_err(|_| {
                acp::Error::internal_error().data("authentication dispatch queue exhausted")
            })?;
            Ok(acp::Handled::Yes)
        },
        acp::on_receive_dispatch!(),
    );
    let result=builder.connect_with(transport,async move |cx:v1::ConnectionTo<acp::Agent>| {
        let init=cx.send_request(acp::UntypedMessage::new("initialize",json!({"protocolVersion":1,"clientInfo":{"name":"native-agent-workbench-auth","version":"0.1.0"},"clientCapabilities":{"elicitation":{"url":{},"form":{}}}}))?).block_task();
        let init=tokio::time::timeout(options.setup_timeout,init).await.map_err(|_|acp::Error::internal_error().data("authentication discovery timed out"))??;
        if init["agentInfo"]["version"]!="2.5.0"||init["agentInfo"]["name"]!="@agentclientprotocol/codex-acp" {return Err(acp::Error::invalid_params().data("unverified authentication adapter version"));}
        let mut session=AuthenticationSession::new(scope,&init)?;
        session.action(first,&cx,&tx,false).await.map_err(|_|acp::Error::internal_error().data("authentication action failed"))?;
        let mut tick=tokio::time::interval(Duration::from_millis(100));
        loop {tokio::select! {
            _=stop.changed()=>break,
            _=tick.tick()=>session.tick().map_err(|_|acp::Error::internal_error().data("authentication expiry failed"))?,
            work=commands.recv()=>{let Some(work)=work else {break};session.action(work,&cx,&tx,false).await.map_err(|_|acp::Error::internal_error().data("authentication action failed"))?;}
            item=rx.recv()=>{let Some(item)=item else {break};session.incoming(item).map_err(|_|acp::Error::internal_error().data("authentication dispatch failed"))?;}
        }}
        session.close();Ok(())
    }).await;
    let _ = result;
    let _ = process.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_exports_no_terminal_environment_or_private_method_metadata() {
        let init = json!({"protocolVersion":1,"agentCapabilities":{},"authMethods":[{"id":"terminal","name":"Official terminal","type":"terminal","args":["--login"],"env":{"PRIVATE_TEST_VALUE":"do-not-export"}},{"id":"chat-gpt","name":"ChatGPT","_meta":{"private_url":"https://example.invalid/private-test"}}]});
        let session = AuthenticationSession::new(
            AuthenticationScope::agent(AgentId::new(), ConnectionEpoch::new(1)),
            &init,
        )
        .unwrap();
        assert_eq!(session.methods[0].kind, AuthenticationMethodKind::Terminal);
        assert!(matches!(
            session.methods[0].availability,
            AuthenticationMethodAvailability::Unavailable {
                reason: AuthenticationUnavailableReason::UnsupportedMethodKind
            }
        ));
        let public = format!("{:?}", session.methods);
        assert!(!public.contains("do-not-export"));
        assert!(!public.contains("private-test"));
    }
}
