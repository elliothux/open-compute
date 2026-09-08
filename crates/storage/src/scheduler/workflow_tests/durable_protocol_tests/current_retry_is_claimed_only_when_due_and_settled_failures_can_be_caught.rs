use super::*;

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
