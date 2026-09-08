use super::*;

#[tokio::test]
async fn workflow_private_http_is_bounded_and_rechecks_startup_generation() {
    let f = fixture();
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig {
            max_in_flight_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("ab".repeat(32)));
    let request = |content: &str, body: axum::body::Body| {
        Request::builder()
            .method("POST")
            .uri("/internal/workflows/runs/claim-batch")
            .header("content-type", content)
            .header("x-open-compute-binding-token", "ab".repeat(32))
            .header("x-open-compute-startup-generation", "generation-one")
            .body(body)
            .unwrap()
    };
    let response = service
        .handle(
            request("text/plain", axum::body::Body::empty()),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_METHOD_UNSUPPORTED"
    );
    let response = service
        .handle(
            request("application/json", axum::body::Body::from("not json")),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_SERIALIZATION_UNSUPPORTED"
    );
    let response = service
        .handle(
            request(
                "application/json",
                axum::body::Body::from("x".repeat(MAX_BODY + 1)),
            ),
            auth.clone(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let permit = service.concurrency.clone().acquire_owned().await.unwrap();
    let response = service
        .handle(
            request("application/json", axum::body::Body::empty()),
            auth.clone(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    drop(permit);
    auth.activate_for_test(SecretString::new("cd".repeat(32)));
    let response = service
        .handle(
            request("application/json", axum::body::Body::from("{}")),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_RUN_STALE"
    );
    assert!(
        to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty()
    );
    for (code, status) in [
        (ErrorCode::WorkflowRuntimeUnavailable, 503),
        (ErrorCode::WorkflowStateQuotaExceeded, 429),
        (ErrorCode::WorkflowEventQueueFull, 429),
        (ErrorCode::WorkflowInstanceNotFound, 404),
        (ErrorCode::WorkflowRunStale, 409),
        (ErrorCode::WorkflowInstanceBusy, 409),
        (ErrorCode::WorkflowInstanceStateConflict, 409),
        (ErrorCode::WorkflowInstanceCleanupPending, 409),
        (ErrorCode::WorkflowResultTooLarge, 413),
        (ErrorCode::WorkflowSerializationUnsupported, 422),
    ] {
        assert_eq!(response_error(code).status().as_u16(), status);
    }
}
