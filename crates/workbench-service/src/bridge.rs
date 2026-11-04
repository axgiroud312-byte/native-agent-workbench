use crate::actor::{Message, Reply};
use agent_runtime::{AgentResolver, AgentRuntime, AttachmentResolver};
use futures::{StreamExt, stream::FuturesUnordered};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use workbench_core::*;

#[derive(Clone)]
pub(crate) struct EventSink(pub mpsc::Sender<EventEnvelope>);
impl RuntimeEventSink for EventSink {
    fn emit(&self, event: EventEnvelope) -> PortFuture<'_, ()> {
        Box::pin(async move { self.0.send(event).await.map_err(|_| crate::api::closed()) })
    }
}
#[derive(Clone)]
pub(crate) struct Bridge(pub mpsc::Sender<Message>);
impl Bridge {
    fn query<T: Send + 'static>(
        &self,
        build: impl FnOnce(Reply<T>) -> Message + Send + 'static,
    ) -> futures::future::BoxFuture<'static, Result<T, BusinessError>> {
        let sender = self.0.clone();
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            sender
                .send(build(reply))
                .await
                .map_err(|_| crate::api::closed())?;
            response.await.map_err(|_| crate::api::closed())?
        })
    }
}
impl AgentResolver for Bridge {
    fn resolve(
        &self,
        id: AgentId,
    ) -> futures::future::BoxFuture<'_, Result<AgentDefinition, BusinessError>> {
        self.query(move |reply| Message::ResolveAgent { id, reply })
    }
}
impl AttachmentResolver for Bridge {
    fn resolve(
        &self,
        session: SessionId,
        attachment: AttachmentId,
    ) -> futures::future::BoxFuture<'_, Result<(Attachment, Vec<u8>), BusinessError>> {
        self.query(move |reply| Message::ResolveAttachment {
            session,
            attachment,
            reply,
        })
    }
}
impl AuthenticationNoticeSink for Bridge {
    fn emit_authentication(&self, notice: AuthenticationNotice) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.0
                .send(Message::AuthenticationNotice(notice))
                .await
                .map_err(|_| crate::api::closed())
        })
    }
}

/// FIFO admission happens before polling any long authentication futures. Interactions and
/// cancel therefore continue while Discover/Authenticate wait on their original responder.
pub(crate) fn authentication_dispatcher(
    runtime: AgentRuntime,
    sink: Bridge,
    mut requests: mpsc::Receiver<AuthenticationRequest>,
) {
    tokio::task::spawn_local(async move {
        let mut pending = FuturesUnordered::new();
        loop {
            tokio::select! {
                biased;
                request=requests.recv()=>match request {
                    Some(request)=>pending.push(runtime.execute_authentication(request,&sink)),
                    None=>break,
                },
                _=pending.next(),if !pending.is_empty()=>{},
            }
        }
        while pending.next().await.is_some() {}
        let _ = sink.0.send(Message::AuthenticationStopped).await;
    });
}

pub(crate) fn runtime(options: mpsc::Sender<Message>) -> Result<AgentRuntime, BusinessError> {
    let resolver = Arc::new(Bridge(options));
    AgentRuntime::new(
        agent_runtime::RuntimeOptions::new()
            .with_agents(resolver.clone())
            .with_attachments(resolver),
    )
}
