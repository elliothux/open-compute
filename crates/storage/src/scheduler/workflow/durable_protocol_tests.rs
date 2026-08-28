//! V2 production storage APIs, without SQL-authored step outcomes.

use super::*;

fn do_step(ordinal: u32, config: Value) -> WorkflowStepDescriptor {
    descriptor(ordinal, WorkflowStepKind::Do, config)
}
fn attempt(ordinal: u32, grant: WorkflowV2StepGrant) -> WorkflowStepAttempt {
    let WorkflowV2StepGrant::Run {
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
            .claim_workflow_batch_v2(
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

#[test]
fn v2_pause_drains_grants_and_replays_completed_steps_without_extending_deadlines() {
    let (temp, store, identity) = setup_v2();
    let limits = WorkflowsConfig::default();
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Pause, 0, &limits)
        .unwrap();
    assert!(
        store
            .claim_workflow(&identity, 0, &limits)
            .unwrap()
            .is_none()
    );
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Pause, 0, &limits)
        .unwrap();
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Resume, 1, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 1, &limits)
        .unwrap()
        .unwrap();
    let first = do_step(0, json!({"timeout":100}));
    let grant = claim(&store, &run.fence, &first, 1, &limits);
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Pause, 2, &limits)
        .unwrap();
    assert!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .unwrap()
            .pause_requested
    );
    assert_eq!(
        store
            .modify_workflow_v2(&identity, WorkflowInstanceAction::Resume, 3, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
    assert_eq!(
        store.yield_workflow_v2(&run.fence, 3).unwrap_err().code(),
        ErrorCode::WorkflowInstanceBusy
    );
    let next = do_step(1, json!({"timeout":100}));
    assert!(matches!(
        store
            .claim_workflow_batch_v2(&run.fence, &[next], 300000, 3, &limits)
            .unwrap()[0],
        WorkflowV2StepGrant::Suspended
    ));
    store
        .settle_workflow_step_v2(
            &run.fence,
            &grant,
            WorkflowStepOutcome::Success("1"),
            4,
            &limits,
        )
        .unwrap();
    // Pause committed before terminal wins even when it arrived after the last grant.
    assert_eq!(
        store
            .finish_workflow_v2(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: "1".into(),
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
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Resume, 6, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 6, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_batch_v2(&run.fence, &[first], 300000, 6, &limits)
            .unwrap()[0],
        WorkflowV2StepGrant::Complete
    ));
    let mut wait = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approval","timeout":10}),
    );
    wait.name_count = 1;
    store
        .register_workflow_wait_v2(&run.fence, &wait, 6, &limits)
        .unwrap();
    store.yield_workflow_v2(&run.fence, 6).unwrap();
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Pause, 7, &limits)
        .unwrap();
    store
        .send_workflow_event_v2(&identity, "approval", "true", 15, &limits)
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
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Resume, 20, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 20, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .register_workflow_wait_v2(&run.fence, &wait, 20, &limits)
            .unwrap(),
        WorkflowV2StepResult::Complete { .. }
    ));
    store
        .finish_workflow_v2(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: "true".into(),
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
                .modify_workflow_v2(&identity, action, 22, &limits)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceStateConflict
        );
    }
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn v2_pause_survives_expired_run_recovery_and_terminate_rejects_late_commits() {
    let (_temp, store, identity) = setup_v2();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let step = do_step(0, json!({"timeout":100,"retries":{"limit":0,"delay":0}}));
    let grant = claim(&store, &run.fence, &step, 1, &limits);
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Pause, 2, &limits)
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
    store.maintain_workflow_due_v2(after, &limits, 10).unwrap();
    assert_eq!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Paused
    );
    store
        .modify_workflow_v2(&identity, WorkflowInstanceAction::Resume, after, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, after, &limits)
        .unwrap()
        .unwrap();
    assert!(
        matches!(store.workflow_step_result_v2(&run.fence,0,after).unwrap(),WorkflowV2StepResult::Failed{code} if code=="WORKFLOW_STEP_TIMEOUT")
    );
    let next = do_step(1, json!({"timeout":100}));
    let next_grant = claim(&store, &run.fence, &next, after, &limits);
    store
        .modify_workflow_v2(
            &identity,
            WorkflowInstanceAction::Terminate,
            after + 1,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .settle_workflow_step_v2(
                &run.fence,
                &next_grant,
                WorkflowStepOutcome::Success("1"),
                after + 2,
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert_eq!(
        store
            .settle_workflow_step_v2(
                &run.fence,
                &grant,
                WorkflowStepOutcome::Success("1"),
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
    assert!(record.durable.unwrap().next_wake_at_ms.is_none());
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn v2_production_do_sleep_event_resume_and_terminal_are_durable() {
    let (temp, store, identity) = setup_v2();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let action = do_step(0, json!({"timeout":100}));
    let grant = claim(&store, &run.fence, &action, 1, &limits);
    assert!(matches!(
        store
            .settle_workflow_step_v2(
                &run.fence,
                &grant,
                WorkflowStepOutcome::Success("7"),
                2,
                &limits
            )
            .unwrap(),
        WorkflowV2StepResult::Complete { .. }
    ));
    let mut sleep = descriptor(1, WorkflowStepKind::Sleep, json!({"duration":10}));
    sleep.name_count = 1;
    assert!(matches!(
        store
            .register_workflow_wait_v2(&run.fence, &sleep, 3, &limits)
            .unwrap(),
        WorkflowV2StepResult::Suspended
    ));
    assert_eq!(
        store.yield_workflow_v2(&run.fence, 3).unwrap(),
        WorkflowState::Waiting
    );
    assert_eq!(store.maintain_workflow_due_v2(12, &limits, 10).unwrap(), 0);
    assert_eq!(store.maintain_workflow_due_v2(13, &limits, 10).unwrap(), 1);
    drop(store);
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 13).unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
    let run = store
        .claim_workflow(&identity, 13, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_batch_v2(
                &run.fence,
                std::slice::from_ref(&action),
                300000,
                13,
                &limits
            )
            .unwrap()[0],
        WorkflowV2StepGrant::Complete
    ));
    assert!(matches!(
        store
            .register_workflow_wait_v2(&run.fence, &sleep, 13, &limits)
            .unwrap(),
        WorkflowV2StepResult::Complete { output_json: None }
    ));
    let mut wait = descriptor(
        2,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approval","timeout":100}),
    );
    wait.name_count = 1;
    assert!(matches!(
        store
            .register_workflow_wait_v2(&run.fence, &wait, 13, &limits)
            .unwrap(),
        WorkflowV2StepResult::Suspended
    ));
    store.yield_workflow_v2(&run.fence, 13).unwrap();
    store
        .send_workflow_event_v2(&identity, "approval", r#"{"decision":true}"#, 14, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 14, &limits)
        .unwrap()
        .unwrap();
    let result = store
        .register_workflow_wait_v2(&run.fence, &wait, 14, &limits)
        .unwrap();
    let WorkflowV2StepResult::Complete {
        output_json: Some(output),
    } = result
    else {
        panic!("event result")
    };
    assert_eq!(
        output,
        r#"{"payload":{"decision":true},"timestampMs":14,"type":"approval"}"#
    );
    assert_eq!(
        store
            .finish_workflow_v2(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: "true".into(),
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
        .durable
        .unwrap();
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
            .send_workflow_event_v2(&identity, "approval", "null", 16, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
}

#[test]
fn v2_retry_is_claimed_only_when_due_and_settled_failures_can_be_caught() {
    let (_temp, store, identity) = setup_v2();
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
            .settle_workflow_step_v2(
                &run.fence,
                &first,
                WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),
                2,
                &limits
            )
            .unwrap(),
        WorkflowV2StepResult::Suspended
    ));
    store.yield_workflow_v2(&run.fence, 2).unwrap();
    assert_eq!(store.maintain_workflow_due_v2(11, &limits, 10).unwrap(), 0);
    assert_eq!(store.maintain_workflow_due_v2(12, &limits, 10).unwrap(), 1);
    let run = store
        .claim_workflow(&identity, 12, &limits)
        .unwrap()
        .unwrap();
    let second = claim(&store, &run.fence, &action, 12, &limits);
    assert_eq!(second.attempt, 2);
    assert!(
        matches!(store.settle_workflow_step_v2(&run.fence,&second,WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),13,&limits).unwrap(),WorkflowV2StepResult::Failed {code} if code=="WORKFLOW_STEP_RETRIES_EXHAUSTED")
    );
    let fallback = do_step(1, json!({"timeout":100}));
    let token = claim(&store, &run.fence, &fallback, 14, &limits);
    store
        .settle_workflow_step_v2(
            &run.fence,
            &token,
            WorkflowStepOutcome::Success("42"),
            15,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .finish_workflow_v2(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: "42".into(),
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
fn v2_unknown_recovery_does_not_extend_attempt_and_late_success_loses_to_deadline() {
    let (_temp, store, identity) = setup_v2();
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
        .claim_workflow_batch_v2(
            &next.fence,
            std::slice::from_ref(&action),
            300000,
            111,
            &limits,
        )
        .unwrap();
    assert!(matches!(
        &grants[0],
        WorkflowV2StepGrant::Run {
            attempt: 1,
            remaining_ms: 90,
            ..
        }
    ));
    let new = attempt(0, grants.into_iter().next().unwrap());
    assert_ne!(new.step_token, old.step_token);
    assert_eq!(
        store
            .settle_workflow_step_v2(
                &next.fence,
                &old,
                WorkflowStepOutcome::Success("1"),
                112,
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepStale
    );
    assert!(
        matches!(store.settle_workflow_step_v2(&next.fence,&new,WorkflowStepOutcome::Success("2"),201,&limits).unwrap(),WorkflowV2StepResult::Failed {code} if code=="WORKFLOW_STEP_TIMEOUT")
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn v2_batch_commits_independently_then_yields_after_siblings_drain() {
    let (_temp, store, identity) = setup_v2();
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
        .claim_workflow_batch_v2(&run.fence, &batch, 300000, 1, &limits)
        .unwrap()
        .into_iter();
    let first = attempt(0, grants.next().unwrap());
    let second = attempt(1, grants.next().unwrap());
    assert_ne!(first.step_token, second.step_token);
    store
        .settle_workflow_step_v2(
            &run.fence,
            &second,
            WorkflowStepOutcome::Failure(ErrorCode::WorkflowExecutionFailed),
            2,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store.yield_workflow_v2(&run.fence, 2).unwrap_err().code(),
        ErrorCode::WorkflowInstanceBusy
    );
    store
        .settle_workflow_step_v2(
            &run.fence,
            &first,
            WorkflowStepOutcome::Success("1"),
            3,
            &limits,
        )
        .unwrap();
    store.yield_workflow_v2(&run.fence, 3).unwrap();
    store.maintain_workflow_due_v2(12, &limits, 10).unwrap();
    let run = store
        .claim_workflow(&identity, 12, &limits)
        .unwrap()
        .unwrap();
    let mut grants = store
        .claim_workflow_batch_v2(&run.fence, &batch, 300000, 12, &limits)
        .unwrap()
        .into_iter();
    assert!(matches!(
        grants.next().unwrap(),
        WorkflowV2StepGrant::Complete
    ));
    let last = attempt(1, grants.next().unwrap());
    assert_eq!(last.attempt, 2);
    store
        .settle_workflow_step_v2(
            &run.fence,
            &last,
            WorkflowStepOutcome::Success("2"),
            13,
            &limits,
        )
        .unwrap();
    store
        .finish_workflow_v2(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: "[1,2]".into(),
                final_ordinal: 2,
            },
            14,
            &limits,
        )
        .unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
}

#[test]
fn v2_buffering_timeout_boundary_and_budget_yield_do_not_consume_callbacks() {
    let (_temp, store, identity) = setup_v2();
    let limits = WorkflowsConfig::default();
    store
        .send_workflow_event_v2(&identity, "ok", "7", 0, &limits)
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
        store
            .register_workflow_wait_v2(&run.fence, &wait, 1, &limits)
            .unwrap(),
        WorkflowV2StepResult::Complete { .. }
    ));
    let mut late = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ok","timeout":1}),
    );
    late.name_count = 2;
    store
        .register_workflow_wait_v2(&run.fence, &late, 1, &limits)
        .unwrap();
    store
        .send_workflow_event_v2(&identity, "ok", "8", 2, &limits)
        .unwrap();
    assert_eq!(
        store.yield_workflow_v2(&run.fence, 2).unwrap(),
        WorkflowState::Queued
    );
    let run = store
        .claim_workflow(&identity, 2, &limits)
        .unwrap()
        .unwrap();
    assert!(
        matches!(store.register_workflow_wait_v2(&run.fence,&late,2,&limits).unwrap(),WorkflowV2StepResult::Failed {code} if code=="WORKFLOW_EVENT_TIMEOUT")
    );
    let mut action = do_step(2, json!({"timeout":100}));
    action.name_count = 1;
    assert!(matches!(
        store
            .claim_workflow_batch_v2(&run.fence, std::slice::from_ref(&action), 100, 2, &limits)
            .unwrap()[0],
        WorkflowV2StepGrant::Suspended
    ));
    store.yield_workflow_v2(&run.fence, 2).unwrap();
    let run = store
        .claim_workflow(&identity, 3, &limits)
        .unwrap()
        .unwrap();
    assert_eq!(claim(&store, &run.fence, &action, 3, &limits).attempt, 1);
    store.verify_workflow_history(identity.instance_id).unwrap();
}
