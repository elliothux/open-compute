use super::*;

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
