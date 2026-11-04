use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct Sink(RefCell<Vec<EventEnvelope>>);
impl RuntimeEventSink for Sink {
    fn emit(&self, event: EventEnvelope) -> PortFuture<'_, ()> {
        self.0.borrow_mut().push(event);
        Box::pin(async { Ok(()) })
    }
}
fn admission(capacity: usize) -> (AgentRuntime, mpsc::Receiver<Work>) {
    let (commands, rx) = mpsc::channel(capacity);
    let (shutdown, _) = watch::channel(false);
    let (_, stopped) = watch::channel(None);
    let (authentication, _) = mpsc::channel(1);
    (
        AgentRuntime {
            service: Arc::new(Service {
                commands,
                authentication,
                shutdown,
                stopped,
                capacity: 16,
            }),
        },
        rx,
    )
}
fn submit() -> RuntimeEffect {
    submitted().1
}
fn submitted() -> (WorkbenchState, RuntimeEffect) {
    let mut state = WorkbenchState::new();
    for record in examples::interleaved_example().records {
        let transition = reduce(&state, &record, ApplyMode::Live).unwrap();
        if let Some(effect) = transition
            .effects()
            .iter()
            .find(|effect| matches!(effect.operation, RuntimeOperation::Submit { .. }))
        {
            return (transition.state().clone(), effect.clone());
        }
        state = transition.state().clone();
    }
    panic!("shared contract example must contain a submission");
}
#[test]
fn mapped_tools_diffs_and_following_text_form_a_valid_shared_object_graph() {
    let (mut state, effect) = submitted();
    let session = effect.scope.session_id.unwrap();
    let (tx, mut events) = mpsc::channel(64);
    let facts = Facts::new(Scope::session(session, effect.scope.epoch.unwrap()), tx);
    let mut mapper = mapping::Mapper::new(facts);
    mapper.external = Some("remote-session".into());
    let RuntimeOperation::Submit {
        attempt,
        submission,
    } = effect.operation
    else {
        unreachable!()
    };
    mapper.begin(attempt, submission);
    mapper.notification(serde_json::json!({"sessionId":"remote-session","update":{"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"edit file","status":"in_progress","content":[{"type":"diff","path":"note.txt","oldText":"old\n","newText":"new\n"}]}})).unwrap();
    mapper.notification(serde_json::json!({"sessionId":"remote-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"after tool"}}})).unwrap();
    while let Ok(event) = events.try_recv() {
        let transition = reduce(
            &state,
            &JournalRecord::new(state.sequence() + 1, JournalEntry::Event(event)),
            ApplyMode::Live,
        )
        .unwrap();
        assert!(matches!(
            transition.disposition(),
            ApplyDisposition::Applied
        ));
        state = transition.state().clone();
    }
    let snapshot = state.session_snapshot(session).unwrap();
    assert_eq!(snapshot.changes.len(), 1);
    assert!(
        matches!(&snapshot.changes[0].diff,FileDiff::Unified {text} if text.contains("@@ -1,1 +1,1 @@"))
    );
    let blocks = snapshot
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Agent)
        .flat_map(|message| &message.blocks)
        .collect::<Vec<_>>();
    let tool = blocks
        .iter()
        .position(|block| matches!(block.content, ContentPresentation::ToolCall { .. }))
        .unwrap();
    let text=blocks.iter().position(|block|matches!(&block.content,ContentPresentation::Text {text,..} if text=="after tool")).unwrap();
    assert!(tool < text);
}
#[test]
fn execute_admits_in_call_order_before_either_future_is_polled() {
    let (runtime, mut queue) = admission(4);
    let sink = Sink::default();
    let first = submit();
    let first_id = first.cause.clone();
    let mut second = first.clone();
    second.cause = EffectCause::Command {
        id: CommandId::new(),
    };
    second.operation = RuntimeOperation::Cancel;
    let second_id = second.cause.clone();
    let first_future = runtime.execute(first, &sink);
    let second_future = runtime.execute(second, &sink);
    assert_eq!(queue.try_recv().unwrap().effect.cause, first_id);
    assert_eq!(queue.try_recv().unwrap().effect.cause, second_id);
    drop((first_future, second_future));
}
#[tokio::test]
async fn rejected_admission_is_not_sent_with_original_attempt_and_scope() {
    for closed in [false, true] {
        let (runtime, mut queue) = admission(1);
        let occupied = runtime.enqueue(submit());
        if closed {
            queue.close();
        }
        let effect = submit();
        let scope = effect.scope;
        let RuntimeOperation::Submit { attempt, .. } = &effect.operation else {
            unreachable!()
        };
        let id = attempt.id;
        let ticket = runtime.enqueue(effect);
        assert!(!ticket.is_enqueued());
        let sink = Sink::default();
        assert!(ticket.forward(&sink).await.is_err());
        assert!(sink.0.borrow().iter().any(|e|e.scope==scope&&matches!(e.event,Event::Attempt {attempt_id,outcome:AttemptOutcome::NotSent {..}} if attempt_id==id)));
        drop(occupied);
    }
}
#[tokio::test]
async fn losing_an_admitted_owner_produces_unknown_instead_of_silent_awaiting() {
    let (runtime, mut queue) = admission(1);
    let effect = submit();
    let scope = effect.scope;
    let ticket = runtime.enqueue(effect);
    drop(queue.recv().await.unwrap());
    let sink = Sink::default();
    assert!(ticket.forward(&sink).await.is_err());
    assert!(sink.0.borrow().iter().any(|e| e.scope == scope
        && matches!(
            e.event,
            Event::Attempt {
                outcome: AttemptOutcome::Unknown { .. },
                ..
            }
        )));
}
