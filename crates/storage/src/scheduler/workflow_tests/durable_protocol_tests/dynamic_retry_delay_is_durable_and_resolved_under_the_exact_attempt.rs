use super::*;

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
