use super::*;

#[test]
fn workflow_private_protocol_rejects_shape_drift_and_persists_failure_delay() {
    let f = fixture();
    let (definition, _binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let identity = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            definition,
            WorkflowOperationId::generate(),
            Some("private-failure-delay"),
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
                    "ordinal":0,"kind":"do","name":"retry","nameCount":1,
                    "config":{"timeout":10,"retries":{"limit":1,"delay":{"dynamic":true}}},
                    "dependencies":[],"batchFirstOrdinal":0,"batchSize":1
                }],"remainingMs":config.dispatch_timeout_ms}),
            ),
            0,
        )
        .unwrap();
    let grant = &claim["steps"][0];

    assert_eq!(
        service
            .run(
                "timeout",
                body(
                    &run.fence,
                    json!({
                        "ordinal":0,"attempt":grant["attempt"],
                        "stepToken":grant["stepToken"],"extra":true
                    }),
                ),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
    );
    assert_eq!(
        service
            .run(
                "success",
                body(&run.fence, json!({"ordinal":0,"attempt":1})),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        service
            .run("result", json!("not-an-object"), 1)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let settled = service
        .run(
            "failure",
            body(
                &run.fence,
                json!({
                    "ordinal":0,"attempt":grant["attempt"],"stepToken":grant["stepToken"],
                    "code":"WORKFLOW_STEP_TIMEOUT","resolvedDelayMs":3
                }),
            ),
            1,
        )
        .unwrap();
    assert_eq!(settled["state"], "suspended");
}
