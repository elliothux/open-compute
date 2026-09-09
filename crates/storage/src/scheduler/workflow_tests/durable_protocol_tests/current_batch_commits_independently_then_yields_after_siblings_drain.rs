use super::*;

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
