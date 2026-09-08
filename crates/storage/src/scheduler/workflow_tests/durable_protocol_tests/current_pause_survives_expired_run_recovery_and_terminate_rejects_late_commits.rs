use super::*;

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
