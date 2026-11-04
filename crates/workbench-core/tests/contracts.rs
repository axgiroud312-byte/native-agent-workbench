use workbench_core::examples::{ContractExample, confirmed_example, interleaved_example};
use workbench_core::*;

#[derive(Clone)]
struct Scenario {
    state: WorkbenchState,
    records: Vec<JournalRecord>,
    fixture: ContractExample,
}
impl Scenario {
    fn new() -> Self {
        let fixture = interleaved_example();
        let mut state = WorkbenchState::new();
        for record in &fixture.records {
            let t = reduce(&state, record, ApplyMode::Live).unwrap();
            assert_eq!(t.disposition(), &ApplyDisposition::Applied);
            state = t.into_parts().0;
        }
        Self {
            state,
            records: fixture.records.clone(),
            fixture,
        }
    }
    fn apply(&mut self, entry: JournalEntry) -> Transition {
        let record = JournalRecord::new(self.state.sequence() + 1, entry);
        let transition = reduce(&self.state, &record, ApplyMode::Live).unwrap();
        self.state = transition.state().clone();
        self.records.push(record);
        transition
    }
    fn command(&mut self, scope: Scope, command: Command) -> Transition {
        self.apply(JournalEntry::Command(CommandEnvelope::new(
            CommandId::new(),
            Timestamp::new(self.state.sequence() as i64),
            scope,
            command,
        )))
    }
    fn event(&mut self, scope: Scope, event: Event) -> Transition {
        self.apply(JournalEntry::Event(EventEnvelope::new(
            EventId::new(),
            Timestamp::new(self.state.sequence() as i64),
            scope,
            EventSource::new(
                EventSourceId::new(),
                EventOrigin::Runtime {
                    adapter: "fixture".into(),
                    version: "1".into(),
                },
            ),
            event,
        )))
    }
    fn scope(&self, ix: usize) -> Scope {
        Scope::session(self.fixture.sessions[ix], ConnectionEpoch::new(1))
            .with_run(self.fixture.runs[ix])
    }
    fn receipt(&mut self, ix: usize, outcome: AttemptOutcome) -> Transition {
        self.event(
            self.scope(ix)
                .with_request(self.fixture.transport_requests[ix]),
            Event::Attempt {
                attempt_id: self.fixture.attempts[ix],
                outcome,
            },
        )
    }
    fn snapshot(&self, ix: usize) -> SessionSnapshot {
        self.state
            .session_snapshot(self.fixture.sessions[ix])
            .unwrap()
    }
    fn steer(&mut self) -> (SubmissionId, AttemptId, Scope) {
        let scope = self.scope(1).with_request(RequestId::new());
        let attempt = AttemptId::new();
        let mut draft = ComposerDraft::new(self.fixture.sessions[1], Timestamp::new(2000));
        draft.content.push(InputPart::Text {
            text: "途中补充".into(),
        });
        let expected_revision = self.snapshot(1).draft.revision;
        draft.revision = expected_revision + 1;
        self.command(
            Scope::session(self.fixture.sessions[1], ConnectionEpoch::new(1)),
            Command::SaveDraft {
                draft: draft.clone(),
                expected_revision,
            },
        );
        assert!(self.snapshot(1).can_steer);
        let submission = InputSubmission::from_draft(
            SubmissionId::new(),
            &draft,
            SubmissionIntent::SteerRun {
                run_id: self.fixture.runs[1],
            },
            Timestamp::new(2000),
        );
        let id = submission.id;
        assert_eq!(
            self.command(
                scope,
                Command::Submit {
                    submission,
                    attempt_id: attempt
                }
            )
            .disposition(),
            &ApplyDisposition::Applied
        );
        (id, attempt, scope)
    }
}

fn rejected(t: &Transition) {
    assert!(matches!(t.disposition(), ApplyDisposition::Rejected(_)));
    assert!(t.effects().is_empty());
}

#[test]
fn shared_interleaved_example_replays_without_effects_or_raw_ui_payloads() {
    let mut s = Scenario::new();
    let encoded = serde_json::to_string(&Versioned::new(s.fixture.clone())).unwrap();
    let decoded: Versioned<ContractExample> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.record, s.fixture);
    let replayed = replay(WorkbenchState::new(), &decoded.record.records).unwrap();
    assert_eq!(replayed, s.state);
    for record in &s.records {
        let t = reduce(&s.state, record, ApplyMode::Replay).unwrap();
        assert_eq!(t.disposition(), &ApplyDisposition::Duplicate);
        assert!(t.effects().is_empty());
    }
    let a = s.snapshot(0);
    let b = s.snapshot(1);
    assert!(
        matches!(&a.messages[0].blocks[0].content, ContentPresentation::Text { text, .. } if text == "甲继续")
    );
    assert!(
        matches!(&b.messages[0].blocks[0].content, ContentPresentation::Text { text, .. } if text == "乙继续")
    );
    assert!(matches!(a.tools[0].state, ToolState::Failed { .. }));
    assert_eq!(a.permissions.len(), 2);
    assert_eq!(a.questions.len(), 1);
    assert_ne!(
        a.messages[0].context.session_id,
        b.messages[0].context.session_id
    );
    let scope = s.scope(0);
    s.event(
        scope,
        Event::Unsupported {
            type_name: "unknown-form".into(),
            diagnostic: serde_json::json!({"private_diagnostic": "not-for-ui"}),
        },
    );
    assert!(
        !serde_json::to_string(&s.snapshot(0))
            .unwrap()
            .contains("not-for-ui")
    );
    let record = s.records.last().unwrap().clone();
    assert!(
        reduce(
            &s.state,
            &JournalRecord::new(s.state.sequence() + 2, record.entry),
            ApplyMode::Live
        )
        .is_err()
    );
}

#[test]
fn steering_receipts_prevent_duplicate_delivery_and_wait_for_the_old_prompt() {
    // A late consumed-new-turn receipt after Stop must transfer the stop intent, not restart work.
    let mut cancelled = Scenario::new();
    let (_, late_attempt, late_scope) = cancelled.steer();
    cancelled.command(
        cancelled.scope(1).with_request(RequestId::new()),
        Command::Stop,
    );
    cancelled.receipt(
        1,
        AttemptOutcome::PromptReturned {
            end: RunEnd::Cancelled,
        },
    );
    let discovered_run = RunId::new();
    let stop = cancelled.event(
        late_scope,
        Event::Attempt {
            attempt_id: late_attempt,
            outcome: AttemptOutcome::Steering {
                receipt: SteeringReceipt::StartedNewTurn {
                    local_run_id: discovered_run,
                },
            },
        },
    );
    assert!(
        stop.effects()
            .iter()
            .any(|e| matches!(e.operation, RuntimeOperation::Cancel)
                && e.scope.run_id == Some(discovered_run))
    );
    assert!(matches!(
        cancelled
            .snapshot(1)
            .runs
            .iter()
            .find(|r| r.context.run_id == discovered_run)
            .unwrap()
            .state,
        ExecutionState::Cancelling { .. }
    ));
    let mut s = Scenario::new();
    let (submission, attempt, scope) = s.steer();
    assert!(!s.snapshot(1).can_steer);
    let second = InputSubmission::from_draft(
        SubmissionId::new(),
        &s.snapshot(1).draft,
        SubmissionIntent::SteerRun {
            run_id: s.fixture.runs[1],
        },
        Timestamp::new(2001),
    );
    rejected(&s.command(
        scope.with_request(RequestId::new()),
        Command::Submit {
            submission: second,
            attempt_id: AttemptId::new(),
        },
    ));
    s.event(
        scope,
        Event::Attempt {
            attempt_id: attempt,
            outcome: AttemptOutcome::Steering {
                receipt: SteeringReceipt::PromptRequired,
            },
        },
    );
    let run = RunId::new();
    let fallback = |id| Command::FallbackToPrompt {
        submission_id: submission,
        attempt_id: id,
        run_id: run,
    };
    rejected(
        &s.command(
            Scope::session(s.fixture.sessions[1], ConnectionEpoch::new(1))
                .with_run(run)
                .with_request(RequestId::new()),
            fallback(AttemptId::new()),
        ),
    );
    s.receipt(
        1,
        AttemptOutcome::PromptReturned {
            end: RunEnd::Completed {
                result: RunResult::EndOfTurn,
            },
        },
    );
    let t = s.command(
        Scope::session(s.fixture.sessions[1], ConnectionEpoch::new(1))
            .with_run(run)
            .with_request(RequestId::new()),
        fallback(AttemptId::new()),
    );
    assert_eq!(t.disposition(), &ApplyDisposition::Applied);
    assert_eq!(t.effects().len(), 1);
    assert_eq!(
        s.snapshot(1)
            .attempts
            .iter()
            .filter(|a| a.submission_id == submission)
            .count(),
        2
    );
    rejected(&s.command(scope, fallback(AttemptId::new())));

    let mut injected = Scenario::new();
    let (id, attempt, scope) = injected.steer();
    injected.event(
        scope,
        Event::Attempt {
            attempt_id: attempt,
            outcome: AttemptOutcome::Steering {
                receipt: SteeringReceipt::Injected,
            },
        },
    );
    assert!(
        injected
            .snapshot(1)
            .submissions
            .iter()
            .any(|s| s.id == id && matches!(s.state, SubmissionState::Accepted { .. }))
    );
    rejected(&injected.command(
        scope,
        Command::FallbackToPrompt {
            submission_id: id,
            attempt_id: AttemptId::new(),
            run_id: RunId::new(),
        },
    ));

    let mut started = Scenario::new();
    let (id, attempt, scope) = started.steer();
    let new_run = RunId::new();
    started.event(
        scope,
        Event::Attempt {
            attempt_id: attempt,
            outcome: AttemptOutcome::Steering {
                receipt: SteeringReceipt::StartedNewTurn {
                    local_run_id: new_run,
                },
            },
        },
    );
    started.receipt(
        1,
        AttemptOutcome::PromptReturned {
            end: RunEnd::Completed {
                result: RunResult::EndOfTurn,
            },
        },
    );
    assert_eq!(started.snapshot(1).record.current_run_id, Some(new_run));
    assert!(matches!(
        started
            .snapshot(1)
            .runs
            .iter()
            .find(|r| r.context.run_id == new_run)
            .unwrap()
            .state,
        ExecutionState::Running
    ));
    rejected(&started.command(
        scope,
        Command::FallbackToPrompt {
            submission_id: id,
            attempt_id: AttemptId::new(),
            run_id: RunId::new(),
        },
    ));

    let mut unknown = Scenario::new();
    let (id, attempt, scope) = unknown.steer();
    unknown.event(
        scope,
        Event::Attempt {
            attempt_id: attempt,
            outcome: AttemptOutcome::Unknown {
                reason: "timeout".into(),
            },
        },
    );
    assert!(
        unknown
            .snapshot(1)
            .submissions
            .iter()
            .any(|s| s.id == id && matches!(s.state, SubmissionState::OutcomeUnknown { .. }))
    );
    assert!(matches!(
        unknown.snapshot(1).queue.dispatch,
        QueueDispatchState::Paused { .. }
    ));
    rejected(&unknown.command(
        scope,
        Command::FallbackToPrompt {
            submission_id: id,
            attempt_id: AttemptId::new(),
            run_id: RunId::new(),
        },
    ));
}

#[test]
fn cancelled_requests_stay_invalid_and_manual_send_does_not_resume_the_queue() {
    let mut s = Scenario::new();
    let permission_scope = s.scope(0).with_request(s.fixture.permissions[0]);
    let response_id = ResponseAttemptId::new();
    s.command(
        permission_scope,
        Command::RespondPermission {
            response: PermissionResponse::Selected {
                option_id: AgentOptionId::new("allow-once"),
            },
            attempt_id: response_id,
        },
    );
    s.event(
        permission_scope,
        Event::PermissionDelivery {
            attempt_id: response_id,
            delivery: ResponseDelivery::Submitted,
        },
    );
    assert!(matches!(
        s.snapshot(0)
            .permissions
            .iter()
            .find(|p| p.request.context.id == s.fixture.permissions[0])
            .unwrap()
            .request
            .status,
        RequestStatus::ResponseSubmitted { .. }
    ));
    let question_scope = s.scope(0).with_request(s.fixture.question);
    let late_id = ResponseAttemptId::new();
    s.command(
        question_scope,
        Command::RespondQuestion {
            response: QuestionResponse::Cancelled,
            attempt_id: late_id,
        },
    );
    s.command(s.scope(0).with_request(RequestId::new()), Command::Stop);
    s.event(
        question_scope,
        Event::QuestionDelivery {
            attempt_id: late_id,
            delivery: ResponseDelivery::Submitted,
        },
    );
    assert!(matches!(
        s.snapshot(0).questions[0].status,
        RequestStatus::Invalidated { .. }
    ));
    assert!(matches!(
        s.snapshot(0).tools[0].state,
        ToolState::Failed { .. }
    ));
    rejected(&s.command(
        question_scope,
        Command::RespondQuestion {
            response: QuestionResponse::Cancelled,
            attempt_id: ResponseAttemptId::new(),
        },
    ));
    s.receipt(
        0,
        AttemptOutcome::PromptReturned {
            end: RunEnd::Cancelled,
        },
    );
    assert!(!s.snapshot(0).can_dispatch_queue);
    // An empty paused queue must not block the user's explicit new work after confirmed stop.
    s.command(s.scope(1).with_request(RequestId::new()), Command::Stop);
    s.receipt(
        1,
        AttemptOutcome::PromptReturned {
            end: RunEnd::Cancelled,
        },
    );
    let new_run = RunId::new();
    let mut draft = ComposerDraft::new(s.fixture.sessions[1], Timestamp::new(3000));
    draft.content.push(InputPart::Text {
        text: "停止后再发".into(),
    });
    let submission = InputSubmission::from_draft(
        SubmissionId::new(),
        &draft,
        SubmissionIntent::StartRun { run_id: new_run },
        Timestamp::new(3000),
    );
    let t = s.command(
        Scope::session(s.fixture.sessions[1], ConnectionEpoch::new(1))
            .with_run(new_run)
            .with_request(RequestId::new()),
        Command::Submit {
            submission,
            attempt_id: AttemptId::new(),
        },
    );
    assert_eq!(t.disposition(), &ApplyDisposition::Applied);
    assert!(matches!(
        s.snapshot(1).queue.dispatch,
        QueueDispatchState::Paused { .. }
    ));
}

#[test]
fn configuration_settlement_preserves_run_snapshot_and_unsupported_form_fields() {
    let mut s = Scenario::new();
    let scope = Scope::session(s.fixture.sessions[0], ConnectionEpoch::new(1));
    let first = RequestId::new();
    let second = RequestId::new();
    let mut draft = s.snapshot(0).draft;
    draft.revision += 1;
    draft.content.push(InputPart::Text {
        text: "当前输入".into(),
    });
    s.command(
        scope,
        Command::SaveDraft {
            draft: draft.clone(),
            expected_revision: draft.revision - 1,
        },
    );
    assert!(s.snapshot(0).can_steer);
    let mut requested = RequestedConfiguration::default();
    requested.model = Some(AgentOptionId::new("fixture-model"));
    s.command(
        scope.with_request(first),
        Command::Configure {
            requested: requested.clone(),
            capability_revision: 1,
        },
    );
    assert!(!s.snapshot(0).can_steer);
    let blocked = InputSubmission::from_draft(
        SubmissionId::new(),
        &draft,
        SubmissionIntent::SteerRun {
            run_id: s.fixture.runs[0],
        },
        Timestamp::new(2100),
    );
    rejected(&s.command(
        s.scope(0).with_request(RequestId::new()),
        Command::Submit {
            submission: blocked,
            attempt_id: AttemptId::new(),
        },
    ));
    requested.reasoning_effort = Some(AgentOptionId::new("fixture-effort"));
    s.command(
        scope.with_request(second),
        Command::Configure {
            requested: requested.clone(),
            capability_revision: 1,
        },
    );
    let (caps, effective) = confirmed_example(ConnectionEpoch::new(1), 2);
    s.event(
        scope,
        Event::ConfigurationReported {
            effective,
            capabilities: caps,
        },
    );
    assert_eq!(
        s.snapshot(0)
            .record
            .configuration
            .pending
            .as_ref()
            .unwrap()
            .request_id,
        first
    );
    s.event(
        scope.with_request(first),
        Event::ConfigurationChangeSettled {
            reported_revision: 2,
        },
    );
    assert_eq!(
        s.snapshot(0)
            .record
            .configuration
            .pending
            .as_ref()
            .unwrap()
            .request_id,
        second
    );
    assert_eq!(s.snapshot(0).record.configuration.requested, requested);
    let (caps, mut effective) = confirmed_example(ConnectionEpoch::new(1), 3);
    effective.reasoning_effort = Some(AgentOptionId::new("fixture-effort"));
    s.event(
        scope,
        Event::ConfigurationReported {
            effective,
            capabilities: caps,
        },
    );
    s.event(
        scope.with_request(second),
        Event::ConfigurationChangeSettled {
            reported_revision: 3,
        },
    );
    let (caps, effective) = confirmed_example(ConnectionEpoch::new(1), 1);
    s.event(
        scope,
        Event::ConfigurationReported {
            effective,
            capabilities: caps,
        },
    );
    assert_eq!(
        s.snapshot(0)
            .record
            .configuration
            .effective
            .unwrap()
            .capability_revision,
        3
    );
    assert_eq!(
        s.snapshot(0).runs[0]
            .dispatched_configuration
            .as_ref()
            .unwrap()
            .capability_revision,
        1
    );
    assert!(s.snapshot(0).can_steer);
    let (mut capabilities, effective) = confirmed_example(ConnectionEpoch::new(1), 4);
    capabilities.steering_source = None;
    s.event(
        scope,
        Event::ConfigurationReported {
            effective,
            capabilities,
        },
    );
    assert!(!s.snapshot(0).can_steer);
    let blocked = InputSubmission::from_draft(
        SubmissionId::new(),
        &draft,
        SubmissionIntent::SteerRun {
            run_id: s.fixture.runs[0],
        },
        Timestamp::new(2101),
    );
    rejected(&s.command(
        s.scope(0).with_request(RequestId::new()),
        Command::Submit {
            submission: blocked,
            attempt_id: AttemptId::new(),
        },
    ));
    let (capabilities, effective) = confirmed_example(ConnectionEpoch::new(1), 5);
    s.event(
        scope,
        Event::ConfigurationReported {
            effective,
            capabilities,
        },
    );
    assert!(s.snapshot(0).can_steer);
    let expected_revision = draft.revision;
    draft.revision += 1;
    draft.content.clear();
    s.command(
        scope,
        Command::SaveDraft {
            draft: draft.clone(),
            expected_revision,
        },
    );
    assert!(!s.snapshot(0).can_steer);
    let blocked = InputSubmission::from_draft(
        SubmissionId::new(),
        &draft,
        SubmissionIntent::SteerRun {
            run_id: s.fixture.runs[0],
        },
        Timestamp::new(2102),
    );
    rejected(&s.command(
        s.scope(0).with_request(RequestId::new()),
        Command::Submit {
            submission: blocked,
            attempt_id: AttemptId::new(),
        },
    ));
    let id = RequestId::new();
    let req_scope = s.scope(0).with_request(id);
    let field = QuestionField::new(
        AgentFieldId::new("numeric"),
        "数字约束字段",
        QuestionFieldKind::Unsupported {
            type_name: "integer-with-constraints".into(),
            schema: serde_json::json!({"private_schema": "do-not-expose", "minimum": 10}),
        },
    );
    let request = QuestionRequest::new(
        RequestContext::new(
            id,
            RunContext::new(s.fixture.sessions[0], s.fixture.runs[0]),
            ConnectionEpoch::new(1),
            Timestamp::new(4000),
        ),
        vec![field],
    );
    s.event(req_scope, Event::Question { request });
    rejected(&s.command(
        req_scope,
        Command::RespondQuestion {
            response: QuestionResponse::Answered { answers: vec![] },
            attempt_id: ResponseAttemptId::new(),
        },
    ));
    assert!(
        !serde_json::to_string(&s.snapshot(0))
            .unwrap()
            .contains("do-not-expose")
    );
    assert_eq!(
        s.command(
            req_scope,
            Command::RespondQuestion {
                response: QuestionResponse::Declined,
                attempt_id: ResponseAttemptId::new()
            }
        )
        .disposition(),
        &ApplyDisposition::Applied
    );
}

#[test]
fn restart_and_close_gate_late_events_and_preserve_editing_and_tombstones() {
    // A4-02: a queued prompt becomes unknown, then a NEW connection authoritatively
    // reconciles the ORIGINAL attempt. Resume alone and stale/wrong evidence cannot settle it.
    let mut recovery = Scenario::new();
    recovery.receipt(
        0,
        AttemptOutcome::PromptReturned {
            end: RunEnd::Completed {
                result: RunResult::EndOfTurn,
            },
        },
    );
    let session_id = recovery.fixture.sessions[0];
    let original_epoch = ConnectionEpoch::new(1);
    let queued = recovery.snapshot(0).queue.entries[0].clone();
    let run_id = RunId::new();
    let attempt_id = AttemptId::new();
    let rpc_id = RequestId::new();
    let original_scope = Scope::session(session_id, original_epoch)
        .with_run(run_id)
        .with_request(rpc_id);
    let submission = InputSubmission::from_queued(
        SubmissionId::new(),
        &queued,
        SubmissionIntent::StartRun { run_id },
        Timestamp::new(6000),
    );
    let submission_id = submission.id;
    assert_eq!(
        recovery
            .command(
                original_scope,
                Command::Submit {
                    submission,
                    attempt_id
                }
            )
            .disposition(),
        &ApplyDisposition::Applied
    );
    recovery.event(
        original_scope,
        Event::Attempt {
            attempt_id,
            outcome: AttemptOutcome::Unknown {
                reason: "lost prompt result".into(),
            },
        },
    );
    let mut next_draft = ComposerDraft::new(session_id, Timestamp::new(6001));
    next_draft.content.push(InputPart::Text {
        text: "后续排队内容".into(),
    });
    let next_queued =
        QueuedInput::from_draft(QueuedInputId::new(), &next_draft, Timestamp::new(6001));
    let next_queued_id = next_queued.id;
    recovery.command(
        Scope::session(session_id, original_epoch),
        Command::Enqueue { input: next_queued },
    );
    let query = |request_id, epoch| {
        (
            Scope::session(session_id, epoch)
                .with_run(run_id)
                .with_request(request_id),
            Command::ReconcileSubmission {
                submission_id,
                attempt_id,
                attempt_epoch: original_epoch,
            },
        )
    };
    let proof = |end| Event::SubmissionReconciled {
        submission_id,
        attempt_id,
        attempt_epoch: original_epoch,
        evidence: ConsumptionEvidence::Consumed { run_id, end },
    };
    // Also exercise the still-present original prompt gate without Restart clearing it first.
    let mut same_connection = recovery.clone();
    let (scope, command) = query(RequestId::new(), original_epoch);
    same_connection.command(scope, command);
    let complete = same_connection.event(
        scope,
        proof(Some(RunEnd::Completed {
            result: RunResult::EndOfTurn,
        })),
    );
    assert!(complete.effects().is_empty());
    same_connection.command(
        Scope::session(session_id, original_epoch),
        Command::CancelQueued { id: next_queued_id },
    );
    let manual_run = RunId::new();
    let manual = InputSubmission::from_draft(
        SubmissionId::new(),
        &next_draft,
        SubmissionIntent::StartRun { run_id: manual_run },
        Timestamp::new(6002),
    );
    assert_eq!(
        same_connection
            .command(
                Scope::session(session_id, original_epoch)
                    .with_run(manual_run)
                    .with_request(RequestId::new()),
                Command::Submit {
                    submission: manual,
                    attempt_id: AttemptId::new()
                }
            )
            .disposition(),
        &ApplyDisposition::Applied
    );

    recovery.command(Scope::default(), Command::Restart);
    recovery.command(
        Scope::session(session_id, ConnectionEpoch::new(2)),
        Command::Connect { resume: true },
    );
    let current_epoch = ConnectionEpoch::new(3);
    let current = Scope::session(session_id, current_epoch);
    recovery.event(
        current,
        Event::ConnectionChanged {
            phase: ConnectionPhase::Ready,
        },
    );
    let binding = recovery.snapshot(0).record.external.clone().unwrap();
    recovery.event(
        current,
        Event::SessionRegistered {
            binding,
            resumed: true,
        },
    );
    let (capabilities, effective) = confirmed_example(current_epoch, 1);
    recovery.event(
        current,
        Event::ConfigurationReported {
            effective,
            capabilities,
        },
    );
    assert!(matches!(
        recovery
            .snapshot(0)
            .submissions
            .iter()
            .find(|s| s.id == submission_id)
            .unwrap()
            .state,
        SubmissionState::OutcomeUnknown { .. }
    ));
    rejected(&recovery.command(current, Command::ResumeQueue));
    recovery.event(
        original_scope,
        Event::Attempt {
            attempt_id,
            outcome: AttemptOutcome::PromptReturned {
                end: RunEnd::Completed {
                    result: RunResult::EndOfTurn,
                },
            },
        },
    );
    let (scope, command) = query(RequestId::new(), current_epoch);
    recovery.command(scope, command);
    rejected(&recovery.event(
        scope,
        Event::SubmissionReconciled {
            submission_id,
            attempt_id,
            attempt_epoch: original_epoch,
            evidence: ConsumptionEvidence::Consumed {
                run_id: recovery.fixture.runs[0],
                end: Some(RunEnd::Completed {
                    result: RunResult::EndOfTurn,
                }),
            },
        },
    ));
    let unresolved = recovery.event(
        scope,
        Event::SubmissionReconciled {
            submission_id,
            attempt_id,
            attempt_epoch: original_epoch,
            evidence: ConsumptionEvidence::Unknown {
                reason: "adapter cannot establish consumption".into(),
            },
        },
    );
    assert!(unresolved.effects().is_empty());
    assert!(matches!(
        recovery
            .snapshot(0)
            .attempts
            .iter()
            .find(|a| a.id == attempt_id)
            .unwrap()
            .state,
        AttemptState::Unknown { .. }
    ));
    assert!(matches!(
        recovery.snapshot(0).queue.entries[0].state,
        QueuedInputState::Dispatching { .. }
    ));
    rejected(&recovery.command(current, Command::ResumeQueue));
    let (scope, command) = query(RequestId::new(), current_epoch);
    recovery.command(scope, command);
    let mut not_finished = recovery.clone();
    not_finished.event(scope, proof(None));
    rejected(&not_finished.command(current, Command::ResumeQueue));
    assert!(!not_finished.snapshot(0).can_dispatch_queue);
    assert_eq!(
        not_finished
            .snapshot(0)
            .attempts
            .iter()
            .find(|a| a.id == attempt_id)
            .unwrap()
            .state,
        AttemptState::ConsumedAwaitingEnd
    );
    let (again_scope, again_command) = query(RequestId::new(), current_epoch);
    assert_eq!(
        not_finished
            .command(again_scope, again_command)
            .disposition(),
        &ApplyDisposition::Applied
    );
    // A second app restart must not lose the already-consumed/unfinished reconciliation path.
    not_finished.command(Scope::default(), Command::Restart);
    not_finished.command(
        Scope::session(session_id, ConnectionEpoch::new(4)),
        Command::Connect { resume: true },
    );
    let newest_epoch = ConnectionEpoch::new(5);
    let newest = Scope::session(session_id, newest_epoch);
    not_finished.event(
        newest,
        Event::ConnectionChanged {
            phase: ConnectionPhase::Ready,
        },
    );
    let (capabilities, effective) = confirmed_example(newest_epoch, 1);
    not_finished.event(
        newest,
        Event::ConfigurationReported {
            effective,
            capabilities,
        },
    );
    rejected(&not_finished.command(newest, Command::ResumeQueue));
    let (again_scope, again_command) = query(RequestId::new(), newest_epoch);
    assert_eq!(
        not_finished
            .command(again_scope, again_command)
            .disposition(),
        &ApplyDisposition::Applied
    );
    let finished_later = not_finished.event(
        again_scope,
        proof(Some(RunEnd::Completed {
            result: RunResult::EndOfTurn,
        })),
    );
    assert_eq!(finished_later.disposition(), &ApplyDisposition::Applied);
    assert!(finished_later.effects().is_empty());
    assert_eq!(
        not_finished
            .snapshot(0)
            .attempts
            .iter()
            .find(|a| a.id == attempt_id)
            .unwrap()
            .state,
        AttemptState::Returned
    );
    assert_eq!(
        not_finished
            .command(newest, Command::ResumeQueue)
            .disposition(),
        &ApplyDisposition::Applied
    );
    assert!(not_finished.snapshot(0).can_dispatch_queue);
    let settled = recovery.event(
        scope,
        proof(Some(RunEnd::Completed {
            result: RunResult::EndOfTurn,
        })),
    );
    assert_eq!(settled.disposition(), &ApplyDisposition::Applied);
    assert!(settled.effects().is_empty());
    let snapshot = recovery.snapshot(0);
    assert!(
        matches!(snapshot.submissions.iter().find(|s| s.id == submission_id).unwrap().state, SubmissionState::Accepted { epoch, run_id: owner, .. } if epoch == original_epoch && owner == run_id)
    );
    assert_eq!(
        snapshot
            .attempts
            .iter()
            .find(|a| a.id == attempt_id)
            .unwrap()
            .state,
        AttemptState::Returned
    );
    assert_eq!(
        snapshot.queue.entries[0].state,
        QueuedInputState::Submitted { submission_id }
    );
    assert_eq!(snapshot.queue.entries[1].state, QueuedInputState::Waiting);
    assert!(matches!(
        snapshot.queue.dispatch,
        QueueDispatchState::Paused {
            reason: QueuePauseReason::ApplicationRestart,
            ..
        }
    ));
    assert_eq!(snapshot.record.current_run_id, None);
    assert!(!snapshot.can_dispatch_queue);
    let mut manual_after_restart = recovery.clone();
    manual_after_restart.command(current, Command::CancelQueued { id: next_queued_id });
    let manual_run = RunId::new();
    let manual = InputSubmission::from_draft(
        SubmissionId::new(),
        &next_draft,
        SubmissionIntent::StartRun { run_id: manual_run },
        Timestamp::new(6003),
    );
    assert_eq!(
        manual_after_restart
            .command(
                current.with_run(manual_run).with_request(RequestId::new()),
                Command::Submit {
                    submission: manual,
                    attempt_id: AttemptId::new()
                }
            )
            .disposition(),
        &ApplyDisposition::Applied
    );
    assert!(matches!(
        manual_after_restart.snapshot(0).queue.dispatch,
        QueueDispatchState::Paused { .. }
    ));
    assert_eq!(
        recovery
            .command(current, Command::ResumeQueue)
            .disposition(),
        &ApplyDisposition::Applied
    );
    assert!(recovery.snapshot(0).can_dispatch_queue);
    let next = recovery.snapshot(0).queue.entries[1].clone();
    let next_run = RunId::new();
    let next_submission = InputSubmission::from_queued(
        SubmissionId::new(),
        &next,
        SubmissionIntent::StartRun { run_id: next_run },
        Timestamp::new(6004),
    );
    let next_id = next_submission.id;
    let dispatched = recovery.command(
        current.with_run(next_run).with_request(RequestId::new()),
        Command::Submit {
            submission: next_submission,
            attempt_id: AttemptId::new(),
        },
    );
    assert_eq!(dispatched.disposition(), &ApplyDisposition::Applied);
    assert_eq!(dispatched.effects().len(), 1);
    assert!(
        matches!(&dispatched.effects()[0].operation, RuntimeOperation::Submit { submission, .. } if submission.id == next_id && submission.id != submission_id)
    );
    assert_eq!(
        replay(WorkbenchState::new(), &recovery.records).unwrap(),
        recovery.state
    );

    let mut s = Scenario::new();
    let id = s.fixture.queue_id;
    s.command(
        Scope::session(s.fixture.sessions[0], ConnectionEpoch::new(1)),
        Command::SetQueueEditing { id, editing: true },
    );
    s.command(Scope::default(), Command::Restart);
    let snap = s.snapshot(0);
    assert_eq!(snap.queue.entries[0].state, QueuedInputState::Editing);
    assert!(matches!(
        snap.queue.dispatch,
        QueueDispatchState::Paused {
            reason: QueuePauseReason::ApplicationRestart,
            ..
        }
    ));
    assert!(matches!(
        snap.runs[0].state,
        ExecutionState::Interrupted { .. }
    ));
    s.event(
        s.scope(0),
        Event::TextDelta {
            message_id: s.fixture.messages[0],
            block_id: s.fixture.blocks[0],
            delta: "迟到".into(),
        },
    );
    assert_eq!(s.snapshot(0).messages, snap.messages);
    assert_eq!(replay(WorkbenchState::new(), &s.records).unwrap(), s.state);

    // Persisted CloseSession followed by a crash hands cleanup to the new instance.
    let mut crash = Scenario::new();
    let abandoned_epoch = ConnectionEpoch::new(1);
    let closing_session = crash.fixture.sessions[1];
    let abandoned_scope = Scope::session(closing_session, abandoned_epoch);
    crash.command(abandoned_scope, Command::CloseSession);
    crash.command(Scope::default(), Command::Restart);
    let cleanup_scope = Scope::session(closing_session, ConnectionEpoch::new(2));
    assert!(crash.snapshot(1).closing);
    assert_eq!(
        crash
            .snapshot(1)
            .closing_handoff
            .as_ref()
            .unwrap()
            .abandoned_epoch,
        abandoned_epoch
    );
    crash.event(abandoned_scope, Event::ResourcesReleased);
    crash.event(cleanup_scope, Event::ResourcesReleased);
    rejected(&crash.command(cleanup_scope, Command::Connect { resume: true }));
    rejected(&crash.command(cleanup_scope, Command::DeleteSession));
    let first_cleanup = cleanup_scope.with_request(RequestId::new());
    let start_cleanup = crash.command(
        first_cleanup,
        Command::ReconcileClosingResources { abandoned_epoch },
    );
    assert!(
        matches!(&start_cleanup.effects()[0].operation, RuntimeOperation::ReconcileClosingResources { abandoned_epoch: epoch } if *epoch == abandoned_epoch)
    );
    crash.event(
        first_cleanup,
        Event::ClosingResourcesReconciled {
            abandoned_epoch,
            outcome: ResourceCleanupOutcome::Unconfirmed {
                reason: "ownership not yet verified".into(),
            },
        },
    );
    assert!(crash.snapshot(1).closing);
    rejected(&crash.command(cleanup_scope, Command::Connect { resume: true }));
    let final_cleanup = cleanup_scope.with_request(RequestId::new());
    crash.command(
        final_cleanup,
        Command::ReconcileClosingResources { abandoned_epoch },
    );
    crash.event(
        final_cleanup,
        Event::ClosingResourcesReconciled {
            abandoned_epoch: ConnectionEpoch::new(2),
            outcome: ResourceCleanupOutcome::Released,
        },
    );
    assert!(crash.snapshot(1).closing);
    crash.event(
        final_cleanup,
        Event::ClosingResourcesReconciled {
            abandoned_epoch,
            outcome: ResourceCleanupOutcome::Released,
        },
    );
    assert!(!crash.snapshot(1).closing);
    assert!(crash.snapshot(1).closing_handoff.is_none());
    assert!(matches!(
        crash.snapshot(1).runs[0].state,
        ExecutionState::Interrupted { .. }
    ));
    assert_eq!(
        crash
            .command(cleanup_scope, Command::Connect { resume: true })
            .disposition(),
        &ApplyDisposition::Applied
    );
    let reconnected = Scope::session(closing_session, ConnectionEpoch::new(3));
    crash.event(
        reconnected,
        Event::ConnectionChanged {
            phase: ConnectionPhase::Ready,
        },
    );
    crash.event(
        final_cleanup,
        Event::ClosingResourcesReconciled {
            abandoned_epoch,
            outcome: ResourceCleanupOutcome::Released,
        },
    );
    assert!(matches!(
        crash.snapshot(1).record.connection.phase,
        ConnectionPhase::Ready
    ));
    assert_eq!(
        replay(WorkbenchState::new(), &crash.records).unwrap(),
        crash.state
    );

    let mut close = Scenario::new();
    let scope = Scope::session(close.fixture.sessions[1], ConnectionEpoch::new(1));
    close.command(scope, Command::CloseSession);
    close.event(
        scope,
        Event::ConnectionChanged {
            phase: ConnectionPhase::Ready,
        },
    );
    assert!(close.snapshot(1).closing);
    assert!(!matches!(
        close.snapshot(1).record.connection.phase,
        ConnectionPhase::Ready
    ));
    rejected(&close.command(scope, Command::ResumeQueue));
    rejected(&close.command(
        scope.with_request(RequestId::new()),
        Command::Configure {
            requested: RequestedConfiguration::default(),
            capability_revision: 1,
        },
    ));
    close.event(scope, Event::ResourcesReleased);
    assert_eq!(
        close
            .command(scope, Command::Connect { resume: true })
            .disposition(),
        &ApplyDisposition::Applied
    );
    let new_scope = Scope::session(close.fixture.sessions[1], ConnectionEpoch::new(2));
    close.event(
        new_scope,
        Event::ConnectionChanged {
            phase: ConnectionPhase::Ready,
        },
    );
    close.event(scope, Event::ResourcesReleased);
    assert!(matches!(
        close.snapshot(1).record.connection.phase,
        ConnectionPhase::Ready
    ));
    close.command(new_scope, Command::CloseSession);
    close.event(new_scope, Event::ResourcesReleased);
    close.command(new_scope, Command::DeleteSession);
    assert!(close.state.is_deleted(close.fixture.sessions[1]));
    close.event(
        new_scope,
        Event::ConnectionChanged {
            phase: ConnectionPhase::Ready,
        },
    );
    assert!(
        close
            .state
            .session_snapshot(close.fixture.sessions[1])
            .is_none()
    );
    // Agent history replay without reliable identities is quarantined, never appended/hash-deduped.
    let mut history = Scenario::new();
    let before = history.snapshot(1).messages;
    history.apply(JournalEntry::Event(EventEnvelope::new(
        EventId::new(),
        Timestamp::new(5000),
        history.scope(1),
        EventSource::new(
            EventSourceId::new(),
            EventOrigin::AgentReplay {
                adapter: "fixture".into(),
                version: "1".into(),
            },
        ),
        Event::Unsupported {
            type_name: "history-without-id".into(),
            diagnostic: serde_json::json!({"text":"乙继续"}),
        },
    )));
    assert_eq!(history.snapshot(1).messages, before);
    assert!(matches!(
        history.snapshot(1).record.history,
        HistoryAvailability::Partial { .. }
    ));
}
