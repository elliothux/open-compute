//! production storage APIs, without SQL-authored step outcomes.

use super::*;
use open_compute_core::WorkflowOperationId;

const NULL_VALUE: &str = "T0NEVgECAA==";
const TRUE_VALUE: &str = "T0NEVgECAw==";
const ONE_VALUE: &str = "T0NEVgECBD/wAAAAAAAA";
const TWO_VALUE: &str = "T0NEVgECBEAAAAAAAAAA";
const SEVEN_VALUE: &str = "T0NEVgECBEAcAAAAAAAA";
const EIGHT_VALUE: &str = "T0NEVgECBEAgAAAAAAAA";
const FORTY_TWO_VALUE: &str = "T0NEVgECBEBFAAAAAAAA";
const ARRAY_VALUE: &str = "T0NEVgECEAAAAAIAAAAABD/wAAAAAAAABEAAAAAAAAAA";
const DECISION_VALUE: &str = "T0NEVgECEQAAAAEAAAAIZGVjaXNpb24D";

fn do_step(ordinal: u32, config: Value) -> WorkflowStepDescriptor {
    descriptor(ordinal, WorkflowStepKind::Do, config)
}
fn attempt(ordinal: u32, grant: WorkflowStepGrant) -> WorkflowStepAttempt {
    let WorkflowStepGrant::Run {
        step_token,
        attempt,
        ..
    } = grant
    else {
        panic!("expected callback grant")
    };
    WorkflowStepAttempt {
        ordinal,
        attempt,
        step_token,
    }
}
fn claim(
    store: &SchedulerStore,
    fence: &WorkflowFence,
    step: &WorkflowStepDescriptor,
    now: i64,
    limits: &WorkflowsConfig,
) -> WorkflowStepAttempt {
    attempt(
        step.ordinal,
        store
            .claim_workflow_batch(
                fence,
                std::slice::from_ref(step),
                limits.dispatch_timeout_ms,
                now,
                limits,
            )
            .unwrap()
            .remove(0),
    )
}

fn wait_result(
    store: &SchedulerStore,
    fence: &WorkflowFence,
    step: &WorkflowStepDescriptor,
    now: i64,
    limits: &WorkflowsConfig,
) -> WorkflowStepResult {
    store
        .claim_workflow_batch(
            fence,
            std::slice::from_ref(step),
            limits.dispatch_timeout_ms,
            now,
            limits,
        )
        .unwrap();
    store
        .workflow_step_result(fence, step.ordinal, now)
        .unwrap()
}

#[test]
fn current_pause_drains_grants_and_replays_completed_steps_without_extending_deadlines() {
    let (temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 0, &limits)
        .unwrap();
    assert!(
        store
            .claim_workflow(&identity, 0, &limits)
            .unwrap()
            .is_none()
    );
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 0, &limits)
        .unwrap();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, 1, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 1, &limits)
        .unwrap()
        .unwrap();
    let first = do_step(0, json!({"timeout":100}));
    let grant = claim(&store, &run.fence, &first, 1, &limits);
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 2, &limits)
        .unwrap();
    assert!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .pause_requested
    );
    assert_eq!(
        store
            .modify_workflow(&identity, WorkflowInstanceAction::Resume, 3, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
    assert_eq!(
        store.yield_workflow(&run.fence, 3).unwrap_err().code(),
        ErrorCode::WorkflowInstanceBusy
    );
    let next = do_step(1, json!({"timeout":100}));
    assert!(matches!(
        store
            .claim_workflow_batch(&run.fence, &[next], 300000, 3, &limits)
            .unwrap()[0],
        WorkflowStepGrant::Suspended
    ));
    store
        .settle_workflow_step(
            &run.fence,
            &grant,
            WorkflowStepOutcome::Success(ONE_VALUE),
            4,
            &limits,
        )
        .unwrap();
    // Pause committed before terminal wins even when it arrived after the last grant.
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: ONE_VALUE.into(),
                    final_ordinal: 1
                },
                5,
                &limits
            )
            .unwrap(),
        WorkflowState::Paused
    );
    assert!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .output_json
            .is_none()
    );
    drop(store);
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 6).unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, 6, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 6, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_batch(&run.fence, &[first], 300000, 6, &limits)
            .unwrap()[0],
        WorkflowStepGrant::Complete { .. }
    ));
    let mut wait = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approval","timeout":10}),
    );
    wait.name_count = 1;
    wait_result(&store, &run.fence, &wait, 6, &limits);
    store.yield_workflow(&run.fence, 6).unwrap();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 7, &limits)
        .unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "approval",
            TRUE_VALUE,
            15,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Paused
    );
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, 20, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 20, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        wait_result(&store, &run.fence, &wait, 20, &limits),
        WorkflowStepResult::Event { .. }
    ));
    store
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: TRUE_VALUE.into(),
                final_ordinal: 2,
            },
            21,
            &limits,
        )
        .unwrap();
    for action in [
        WorkflowInstanceAction::Pause,
        WorkflowInstanceAction::Resume,
        WorkflowInstanceAction::Terminate,
    ] {
        assert_eq!(
            store
                .modify_workflow(&identity, action, 22, &limits)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceStateConflict
        );
    }
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn current_pause_survives_expired_run_recovery_and_terminate_rejects_late_commits() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let step = do_step(0, json!({"timeout":100,"retries":{"limit":0,"delay":0}}));
    let grant = claim(&store, &run.fence, &step, 1, &limits);
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 2, &limits)
        .unwrap();
    let after = i64::try_from(limits.lease_ms).unwrap() + 2;
    store.recover_workflows(after, &limits, 10).unwrap();
    assert_eq!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Paused
    );
    store.maintain_workflow_due(after, &limits, 10).unwrap();
    assert_eq!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Paused
    );
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, after, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, after, &limits)
        .unwrap()
        .unwrap();
    assert!(
        matches!(store.workflow_step_result(&run.fence,0,after).unwrap(),WorkflowStepResult::Failed{code} if code=="WORKFLOW_STEP_TIMEOUT")
    );
    let next = do_step(1, json!({"timeout":100}));
    let next_grant = claim(&store, &run.fence, &next, after, &limits);
    store
        .modify_workflow(
            &identity,
            WorkflowInstanceAction::Terminate,
            after + 1,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .settle_workflow_step(
                &run.fence,
                &next_grant,
                WorkflowStepOutcome::Success(ONE_VALUE),
                after + 2,
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert_eq!(
        store
            .settle_workflow_step(
                &run.fence,
                &grant,
                WorkflowStepOutcome::Success(ONE_VALUE),
                after + 2,
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert!(
        store
            .claim_workflow(&identity, after + 2, &limits)
            .unwrap()
            .is_none()
    );
    let record = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, WorkflowState::Terminated);
    assert!(record.run_token.is_none());
    assert!(record.durable.next_wake_at_ms.is_none());
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn current_production_do_sleep_event_resume_and_terminal_are_durable() {
    let (temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let action = do_step(0, json!({"timeout":100}));
    let grant = claim(&store, &run.fence, &action, 1, &limits);
    assert!(matches!(
        store
            .settle_workflow_step(
                &run.fence,
                &grant,
                WorkflowStepOutcome::Success(SEVEN_VALUE),
                2,
                &limits
            )
            .unwrap(),
        WorkflowStepResult::Complete { .. }
    ));
    let mut sleep = descriptor(1, WorkflowStepKind::Sleep, json!({"duration":10}));
    sleep.name_count = 1;
    assert!(matches!(
        wait_result(&store, &run.fence, &sleep, 3, &limits),
        WorkflowStepResult::Suspended
    ));
    assert_eq!(
        store.yield_workflow(&run.fence, 3).unwrap(),
        WorkflowState::Waiting
    );
    assert_eq!(store.maintain_workflow_due(12, &limits, 10).unwrap(), 0);
    assert_eq!(store.maintain_workflow_due(13, &limits, 10).unwrap(), 1);
    drop(store);
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 13).unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
    let run = store
        .claim_workflow(&identity, 13, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_batch(
                &run.fence,
                std::slice::from_ref(&action),
                300000,
                13,
                &limits
            )
            .unwrap()[0],
        WorkflowStepGrant::Complete { .. }
    ));
    assert!(matches!(
        wait_result(&store, &run.fence, &sleep, 13, &limits),
        WorkflowStepResult::Complete {
            output_base64: None
        }
    ));
    let mut wait = descriptor(
        2,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approval","timeout":100}),
    );
    wait.name_count = 1;
    assert!(matches!(
        wait_result(&store, &run.fence, &wait, 13, &limits),
        WorkflowStepResult::Suspended
    ));
    store.yield_workflow(&run.fence, 13).unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "approval",
            DECISION_VALUE,
            14,
            &limits,
        )
        .unwrap();
    let run = store
        .claim_workflow(&identity, 14, &limits)
        .unwrap()
        .unwrap();
    let result = wait_result(&store, &run.fence, &wait, 14, &limits);
    let WorkflowStepResult::Event {
        event_type,
        payload_base64,
        timestamp_ms,
    } = result
    else {
        panic!("event result")
    };
    assert_eq!(
        (event_type.as_str(), payload_base64.as_str(), timestamp_ms),
        ("approval", DECISION_VALUE, 14)
    );
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: TRUE_VALUE.into(),
                    final_ordinal: 3
                },
                15,
                &limits
            )
            .unwrap(),
        WorkflowState::Complete
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
    let metadata = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap()
        .durable;
    assert_eq!(
        (
            metadata.registered_step_count,
            metadata.settled_step_count,
            metadata.event_count
        ),
        (3, 3, 0)
    );
    assert_eq!(metadata.expires_at_ms, Some(3_600_015));
    assert_eq!(
        store
            .send_workflow_event(
                &identity,
                WorkflowOperationId::generate(),
                "approval",
                NULL_VALUE,
                16,
                &limits,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
}

#[test]
fn current_retry_is_claimed_only_when_due_and_settled_failures_can_be_caught() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let action = do_step(0, json!({"timeout":100,"retries":{"limit":1,"delay":10}}));
    let first = claim(&store, &run.fence, &action, 1, &limits);
    assert_eq!(first.attempt, 1);
    assert!(matches!(
        store
            .settle_workflow_step(
                &run.fence,
                &first,
                WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),
                2,
                &limits
            )
            .unwrap(),
        WorkflowStepResult::Suspended
    ));
    store.yield_workflow(&run.fence, 2).unwrap();
    assert_eq!(store.maintain_workflow_due(11, &limits, 10).unwrap(), 0);
    assert_eq!(store.maintain_workflow_due(12, &limits, 10).unwrap(), 1);
    let run = store
        .claim_workflow(&identity, 12, &limits)
        .unwrap()
        .unwrap();
    let second = claim(&store, &run.fence, &action, 12, &limits);
    assert_eq!(second.attempt, 2);
    assert!(
        matches!(store.settle_workflow_step(&run.fence,&second,WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),13,&limits).unwrap(),WorkflowStepResult::Failed {code} if code=="WORKFLOW_STEP_RETRIES_EXHAUSTED")
    );
    let fallback = do_step(1, json!({"timeout":100}));
    let token = claim(&store, &run.fence, &fallback, 14, &limits);
    store
        .settle_workflow_step(
            &run.fence,
            &token,
            WorkflowStepOutcome::Success(FORTY_TWO_VALUE),
            15,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: FORTY_TWO_VALUE.into(),
                    final_ordinal: 2
                },
                16,
                &limits
            )
            .unwrap(),
        WorkflowState::Complete
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn current_unknown_recovery_does_not_extend_attempt_and_late_success_loses_to_deadline() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig {
        lease_ms: 100,
        heartbeat_ms: 20,
        recovery_backoff_ms: 10,
        ..WorkflowsConfig::default()
    };
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let action = do_step(0, json!({"timeout":200,"retries":{"limit":0,"delay":0}}));
    let old = claim(&store, &run.fence, &action, 1, &limits);
    assert_eq!(store.recover_workflows(101, &limits, 10).unwrap(), 1);
    let next = store
        .claim_workflow(&identity, 111, &limits)
        .unwrap()
        .unwrap();
    let grants = store
        .claim_workflow_batch(
            &next.fence,
            std::slice::from_ref(&action),
            300000,
            111,
            &limits,
        )
        .unwrap();
    assert!(matches!(
        &grants[0],
        WorkflowStepGrant::Run {
            attempt: 1,
            remaining_ms: 90,
            ..
        }
    ));
    let new = attempt(0, grants.into_iter().next().unwrap());
    assert_ne!(new.step_token, old.step_token);
    assert_eq!(
        store
            .settle_workflow_step(
                &next.fence,
                &old,
                WorkflowStepOutcome::Success(ONE_VALUE),
                112,
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepStale
    );
    assert!(
        matches!(store.settle_workflow_step(&next.fence,&new,WorkflowStepOutcome::Success(TWO_VALUE),201,&limits).unwrap(),WorkflowStepResult::Failed {code} if code=="WORKFLOW_STEP_TIMEOUT")
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn current_batch_commits_independently_then_yields_after_siblings_drain() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let batch: Vec<_> = (0..2)
        .map(|ordinal| {
            let mut step = do_step(
                ordinal,
                json!({"timeout":100,"retries":{"limit":1,"delay":10}}),
            );
            step.batch_first_ordinal = 0;
            step.batch_size = 2;
            step.dependencies.clear();
            step
        })
        .collect();
    let mut grants = store
        .claim_workflow_batch(&run.fence, &batch, 300000, 1, &limits)
        .unwrap()
        .into_iter();
    let first = attempt(0, grants.next().unwrap());
    let second = attempt(1, grants.next().unwrap());
    assert_ne!(first.step_token, second.step_token);
    store
        .settle_workflow_step(
            &run.fence,
            &second,
            WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),
            2,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store.yield_workflow(&run.fence, 2).unwrap_err().code(),
        ErrorCode::WorkflowInstanceBusy
    );
    store
        .settle_workflow_step(
            &run.fence,
            &first,
            WorkflowStepOutcome::Success(ONE_VALUE),
            3,
            &limits,
        )
        .unwrap();
    store.yield_workflow(&run.fence, 3).unwrap();
    store.maintain_workflow_due(12, &limits, 10).unwrap();
    let run = store
        .claim_workflow(&identity, 12, &limits)
        .unwrap()
        .unwrap();
    let mut grants = store
        .claim_workflow_batch(&run.fence, &batch, 300000, 12, &limits)
        .unwrap()
        .into_iter();
    assert!(matches!(
        grants.next().unwrap(),
        WorkflowStepGrant::Complete { .. }
    ));
    let last = attempt(1, grants.next().unwrap());
    assert_eq!(last.attempt, 2);
    store
        .settle_workflow_step(
            &run.fence,
            &last,
            WorkflowStepOutcome::Success(TWO_VALUE),
            13,
            &limits,
        )
        .unwrap();
    store
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: ARRAY_VALUE.into(),
                final_ordinal: 2,
            },
            14,
            &limits,
        )
        .unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn mixed_graph_group_persists_explicit_dependencies_and_replays_all_step_kinds() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let first = do_step(0, json!({"timeout":100}));
    let first_attempt = claim(&store, &run.fence, &first, 1, &limits);
    store
        .settle_workflow_step(
            &run.fence,
            &first_attempt,
            WorkflowStepOutcome::Success(ONE_VALUE),
            2,
            &limits,
        )
        .unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "ready",
            TRUE_VALUE,
            2,
            &limits,
        )
        .unwrap();
    let mut sleep = descriptor(1, WorkflowStepKind::Sleep, json!({"duration":0}));
    let mut event = descriptor(
        2,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ready","timeout":100}),
    );
    let mut action = do_step(3, json!({"timeout":100}));
    sleep.name_count = 1;
    event.name_count = 1;
    action.name_count = 2;
    for item in [&mut sleep, &mut event, &mut action] {
        item.dependencies = vec![0];
        item.batch_first_ordinal = 1;
        item.batch_size = 3;
    }
    let grants = store
        .claim_workflow_batch(&run.fence, &[sleep, event, action], 300_000, 3, &limits)
        .unwrap();
    assert!(matches!(grants[0], WorkflowStepGrant::Complete { .. }));
    assert!(matches!(grants[1], WorkflowStepGrant::Complete { .. }));
    let last = attempt(3, grants.into_iter().nth(2).unwrap());
    assert!(matches!(
        store.workflow_step_result(&run.fence, 2, 3).unwrap(),
        WorkflowStepResult::Event {
            ref event_type,
            ref payload_base64,
            timestamp_ms: 2,
        } if event_type == "ready" && payload_base64 == TRUE_VALUE
    ));
    store
        .settle_workflow_step(
            &run.fence,
            &last,
            WorkflowStepOutcome::Success(TWO_VALUE),
            4,
            &limits,
        )
        .unwrap();
    store
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: ARRAY_VALUE.into(),
                final_ordinal: 4,
            },
            5,
            &limits,
        )
        .unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn current_buffering_timeout_boundary_and_budget_yield_do_not_consume_callbacks() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "ok",
            SEVEN_VALUE,
            0,
            &limits,
        )
        .unwrap();
    let run = store
        .claim_workflow(&identity, 1, &limits)
        .unwrap()
        .unwrap();
    let wait = descriptor(
        0,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ok","timeout":0}),
    );
    assert!(matches!(
        wait_result(&store, &run.fence, &wait, 1, &limits),
        WorkflowStepResult::Event { .. }
    ));
    let mut late = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ok","timeout":1}),
    );
    late.name_count = 2;
    wait_result(&store, &run.fence, &late, 1, &limits);
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "ok",
            EIGHT_VALUE,
            2,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store.yield_workflow(&run.fence, 2).unwrap(),
        WorkflowState::Queued
    );
    let run = store
        .claim_workflow(&identity, 2, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        wait_result(&store, &run.fence, &late, 2, &limits),
        WorkflowStepResult::Failed { code } if code == "WORKFLOW_EVENT_TIMEOUT"
    ));
    let mut action = do_step(2, json!({"timeout":100}));
    action.name_count = 1;
    assert!(matches!(
        store
            .claim_workflow_batch(&run.fence, std::slice::from_ref(&action), 100, 2, &limits)
            .unwrap()[0],
        WorkflowStepGrant::Suspended
    ));
    store.yield_workflow(&run.fence, 2).unwrap();
    let run = store
        .claim_workflow(&identity, 3, &limits)
        .unwrap()
        .unwrap();
    assert_eq!(claim(&store, &run.fence, &action, 3, &limits).attempt, 1);
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn rollback_replays_completed_handlers_recovers_inflight_work_and_terminates() {
    let (temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let mut completed = do_step(0, json!({"timeout":100}));
    completed.rollback_config = Some(
        open_compute_core::workflow::WorkflowStepConfig::resolve(
            &json!({"timeout":240000,"retries":{"limit":1,"delay":0}}),
        )
        .unwrap(),
    );
    let completed_attempt = claim(&store, &run.fence, &completed, 0, &limits);
    store
        .settle_workflow_step(
            &run.fence,
            &completed_attempt,
            WorkflowStepOutcome::Success(SEVEN_VALUE),
            1,
            &limits,
        )
        .unwrap();
    let unfinished = do_step(1, json!({"timeout":240000}));
    let stale_attempt = claim(&store, &run.fence, &unfinished, 1, &limits);

    store
        .request_workflow_rollback(&identity, 2, &limits)
        .unwrap();
    assert_eq!(
        store
            .settle_workflow_step(
                &run.fence,
                &stale_attempt,
                WorkflowStepOutcome::Success(EIGHT_VALUE),
                2,
                &limits,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let rollback_run = store
        .claim_workflow(&identity, 2, &limits)
        .unwrap()
        .unwrap();
    assert!(rollback_run.rollback);
    assert!(matches!(
        store
            .claim_workflow_batch(
                &rollback_run.fence,
                std::slice::from_ref(&completed),
                limits.dispatch_timeout_ms,
                2,
                &limits,
            )
            .unwrap()[0],
        WorkflowStepGrant::Complete {
            attempt: Some(1),
            config: Some(_)
        }
    ));
    assert!(matches!(
        store
            .claim_workflow_batch(
                &rollback_run.fence,
                std::slice::from_ref(&unfinished),
                limits.dispatch_timeout_ms,
                2,
                &limits,
            )
            .unwrap()[0],
        WorkflowStepGrant::RollbackBoundary {
            rollback_ordinal: 2
        }
    ));
    let mut rollback = do_step(2, json!({"timeout":240000,"retries":{"limit":1,"delay":0}}));
    rollback.name = "rollback:0".into();
    rollback.name_count = 1;
    rollback.dependencies.clear();
    rollback.rollback_step = true;
    let first_rollback_attempt = claim(&store, &rollback_run.fence, &rollback, 2, &limits);
    assert_eq!(first_rollback_attempt.attempt, 1);

    drop(store);
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 3).unwrap();
    let recovered_at = 2 + i64::try_from(limits.lease_ms).unwrap();
    assert_eq!(
        store.recover_workflows(recovered_at, &limits, 1).unwrap(),
        1
    );
    let ready_at = recovered_at + i64::try_from(limits.recovery_backoff_ms).unwrap();
    let recovered = store
        .claim_workflow(&identity, ready_at, &limits)
        .unwrap()
        .unwrap();
    assert!(recovered.rollback);
    assert!(matches!(
        store
            .claim_workflow_batch(
                &recovered.fence,
                std::slice::from_ref(&completed),
                limits.dispatch_timeout_ms,
                ready_at,
                &limits,
            )
            .unwrap()[0],
        WorkflowStepGrant::Complete { .. }
    ));
    assert!(matches!(
        store
            .claim_workflow_batch(
                &recovered.fence,
                std::slice::from_ref(&unfinished),
                limits.dispatch_timeout_ms,
                ready_at,
                &limits,
            )
            .unwrap()[0],
        WorkflowStepGrant::RollbackBoundary {
            rollback_ordinal: 2
        }
    ));
    let recovered_attempt = claim(&store, &recovered.fence, &rollback, ready_at, &limits);
    assert_eq!(recovered_attempt.attempt, 1);
    store
        .settle_workflow_step(
            &recovered.fence,
            &recovered_attempt,
            WorkflowStepOutcome::Success(NULL_VALUE),
            ready_at + 1,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .finish_workflow(
                &recovered.fence,
                &WorkflowCompletion::Terminated { final_ordinal: 3 },
                ready_at + 2,
                &limits,
            )
            .unwrap(),
        WorkflowState::Terminated
    );
    let record = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert!(!record.durable.rollback_requested);
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn dynamic_retry_delay_is_durable_and_resolved_under_the_exact_attempt() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let step = do_step(
        0,
        json!({"timeout":5,"retries":{"limit":1,"delay":{"dynamic":true},"backoff":"linear"}}),
    );
    let first = claim(&store, &run.fence, &step, 1, &limits);
    assert_eq!(
        format!("{:?}", WorkflowStepOutcome::Success(ONE_VALUE)),
        "Success([REDACTED])"
    );
    assert!(matches!(
        store
            .settle_workflow_step(
                &run.fence,
                &first,
                WorkflowStepOutcome::Success(ONE_VALUE),
                6,
                &limits,
            )
            .unwrap(),
        WorkflowStepResult::ResolveDelay { attempt: 1, ref code, .. }
            if code == "WORKFLOW_STEP_TIMEOUT"
    ));
    assert!(matches!(
        store.workflow_step_result(&run.fence, 0, 6).unwrap(),
        WorkflowStepResult::ResolveDelay { attempt: 1, .. }
    ));
    assert!(matches!(
        store
            .claim_workflow_batch(
                &run.fence,
                std::slice::from_ref(&step),
                limits.dispatch_timeout_ms,
                6,
                &limits,
            )
            .unwrap()[0],
        WorkflowStepGrant::ResolveDelay { attempt: 1, .. }
    ));
    assert_eq!(
        store
            .resolve_workflow_delay(
                &run.fence,
                0,
                1,
                WorkflowDelayResolution {
                    failure_code: ErrorCode::WorkflowExecutionFailed,
                    resolved_delay_ms: Some(0),
                },
                6,
                &limits,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepStale
    );
    assert!(matches!(
        store
            .resolve_workflow_delay(
                &run.fence,
                0,
                1,
                WorkflowDelayResolution {
                    failure_code: ErrorCode::WorkflowStepTimeout,
                    resolved_delay_ms: Some(0),
                },
                6,
                &limits,
            )
            .unwrap(),
        WorkflowStepResult::Suspended
    ));
    store.yield_workflow(&run.fence, 6).unwrap();
    store.maintain_workflow_due(6, &limits, 10).unwrap();
    let retry = store
        .claim_workflow(&identity, 6, &limits)
        .unwrap()
        .unwrap();
    let second = claim(&store, &retry.fence, &step, 6, &limits);
    assert_eq!(second.attempt, 2);
    assert!(matches!(
        store
            .settle_workflow_step(
                &retry.fence,
                &second,
                WorkflowStepOutcome::FailureWithDelay(
                    ErrorCode::WorkflowExecutionFailed,
                    1,
                ),
                7,
                &limits,
            )
            .unwrap(),
        WorkflowStepResult::Failed { ref code }
            if code == "WORKFLOW_STEP_RETRIES_EXHAUSTED"
    ));
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn rejected_dynamic_delay_and_batch_shapes_fail_closed_before_new_work() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .claim_workflow_batch(&run.fence, &[], limits.dispatch_timeout_ms, 0, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepLimitExceeded
    );
    let mut oversized = (0..17)
        .map(|ordinal| {
            let mut step = do_step(ordinal, json!({"timeout":5}));
            step.batch_first_ordinal = 0;
            step.batch_size = 17;
            step
        })
        .collect::<Vec<_>>();
    assert_eq!(
        store
            .claim_workflow_batch(
                &run.fence,
                &oversized,
                limits.dispatch_timeout_ms,
                0,
                &limits,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepLimitExceeded
    );
    oversized.truncate(2);
    oversized[0].batch_size = 2;
    oversized[1].batch_size = 2;
    oversized[1].ordinal = 3;
    assert_eq!(
        store
            .claim_workflow_batch(
                &run.fence,
                &oversized,
                limits.dispatch_timeout_ms,
                0,
                &limits,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );

    let dynamic = do_step(
        0,
        json!({"timeout":5,"retries":{"limit":1,"delay":{"dynamic":true}}}),
    );
    let attempt = claim(&store, &run.fence, &dynamic, 0, &limits);
    assert!(matches!(
        store
            .settle_workflow_step(
                &run.fence,
                &attempt,
                WorkflowStepOutcome::Timeout,
                5,
                &limits,
            )
            .unwrap(),
        WorkflowStepResult::ResolveDelay { .. }
    ));
    assert!(matches!(
        store
            .resolve_workflow_delay(
                &run.fence,
                0,
                1,
                WorkflowDelayResolution {
                    failure_code: ErrorCode::WorkflowStepConfigUnsupported,
                    resolved_delay_ms: None,
                },
                5,
                &limits,
            )
            .unwrap(),
        WorkflowStepResult::Failed { ref code }
            if code == "WORKFLOW_STEP_CONFIG_UNSUPPORTED"
    ));
    store.verify_workflow_history(identity.instance_id).unwrap();
}
