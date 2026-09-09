use super::*;

#[test]
fn workflow_private_dynamic_delay_round_trip_is_durable() {
    let f = fixture();
    let (definition, _binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let identity = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            definition,
            WorkflowOperationId::generate(),
            Some("dynamic-delay"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            0,
        )
        .unwrap();
    let run = f
        .scheduler
        .claim_workflow(&identity, 0, &config)
        .unwrap()
        .unwrap();
    let service =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config.clone())
            .unwrap();
    let claim = service
        .run(
            "claim-batch",
            body(
                &run.fence,
                json!({"steps":[{
                    "ordinal":0,"kind":"do","name":"dynamic","nameCount":1,
                    "config":{"timeout":5,"retries":{"limit":1,"delay":{"dynamic":true}}},
                    "dependencies":[],"batchFirstOrdinal":0,"batchSize":1
                }],"remainingMs":config.dispatch_timeout_ms}),
            ),
            0,
        )
        .unwrap();
    let grant = &claim["steps"][0];
    let timeout = service
        .run(
            "timeout",
            body(
                &run.fence,
                json!({
                    "ordinal":0,
                    "attempt":grant["attempt"],
                    "stepToken":grant["stepToken"]
                }),
            ),
            5,
        )
        .unwrap();
    assert_eq!(timeout["state"], "resolve_delay");
    let resolved = service
        .run(
            "resolve-delay",
            body(
                &run.fence,
                json!({
                    "ordinal":0,"attempt":1,"code":"WORKFLOW_STEP_TIMEOUT",
                    "resolvedDelayMs":0
                }),
            ),
            5,
        )
        .unwrap();
    assert_eq!(resolved["state"], "suspended");
    assert_eq!(
        service
            .run("yield", body(&run.fence, json!({"finalOrdinal":1025})), 5,)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepLimitExceeded
    );
    assert_eq!(
        service
            .run("unknown", body(&run.fence, json!({})), 5)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
    );
}
