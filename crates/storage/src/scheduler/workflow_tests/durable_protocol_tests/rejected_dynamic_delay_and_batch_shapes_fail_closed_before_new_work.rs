use super::*;

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
