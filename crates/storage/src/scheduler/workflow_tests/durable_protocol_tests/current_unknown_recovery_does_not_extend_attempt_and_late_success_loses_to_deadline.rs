use super::*;

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
