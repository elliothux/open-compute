use super::*;
use open_compute_core::{
    AccountId, DeploymentId, SecretString, WorkerId, WorkflowFence, WorkflowInstanceId,
    WorkflowToken,
};
use open_compute_runtime::GenerationAuthRegistry;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

fn target() -> DispatchTarget {
    DispatchTarget {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        worker_code_sha256: "11".repeat(32),
        entrypoint: Some("Flow".into()),
        route_generation: 1,
        request_id: RequestId::generate(),
    }
}

#[tokio::test]
async fn workflow_probe_unknown_and_generation_bound_completion_fail_closed() {
    let status = Arc::new(AtomicU16::new(503));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app=Router::new().route("/internal/validate-workflow",post({let status=status.clone();move || {let status=status.clone();async move { (StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap(),axum::Json(serde_json::json!({"valid":true}))) }}}))
        .route("/internal/workflow",post(||async {axum::Json(serde_json::json!({"outcome":"complete","finalOrdinal":1,"outputJson":"null","loaderOutcome":"warm"}))}));
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = receiver.await;
            })
            .await
            .unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("aa".repeat(32)));
    let transport = WorkerdTransport::for_test_endpoint(auth.clone(), port);
    assert_eq!(
        transport
            .probe_workflow(&target())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRuntimeUnavailable
    );
    status.store(422, Ordering::SeqCst);
    assert_eq!(
        transport
            .probe_workflow(&target())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowVersionNotReady
    );
    status.store(200, Ordering::SeqCst);
    transport.probe_workflow(&target()).await.unwrap();
    let request = WorkflowRunRequest {
        fence: WorkflowFence {
            instance_id: WorkflowInstanceId::generate(),
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([3; 32]),
        },
        external_instance_id: "public-id".into(),
        definition_name: "public-name".into(),
        created_at_ms: 0,
        payload_json: "null".into(),
    };
    let response = transport
        .dispatch_workflow(&target(), &request, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(transport.commit_workflow(response, |_| Ok(7)).unwrap(), 7);
    let stale = transport
        .dispatch_workflow(&target(), &request, Duration::from_secs(1))
        .await
        .unwrap();
    auth.activate_for_test(SecretString::new("bb".repeat(32)));
    let called = AtomicBool::new(false);
    assert_eq!(
        transport
            .commit_workflow(stale, |_| {
                called.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert!(!called.load(Ordering::SeqCst));
    shutdown.send(()).unwrap();
    server.await.unwrap();
}

#[test]
fn workflow_generation_transaction_rejects_unbound_and_retired_tokens() {
    let auth = GenerationAuthRegistry::new();
    let called = AtomicBool::new(false);
    assert!(
        auth.with_authorized("token", "generation", || called
            .store(true, Ordering::SeqCst))
            .is_none()
    );
    auth.activate_for_test(SecretString::new("aa".repeat(32)));
    let credential = auth.credential().unwrap();
    assert!(
        auth.with_authorized(&"aa".repeat(32), "first", || ())
            .is_some()
    );
    assert!(
        auth.with_authorized(&"aa".repeat(32), "second", || called
            .store(true, Ordering::SeqCst))
            .is_none()
    );
    assert!(auth.with_authorized(&"aa".repeat(32), "", || ()).is_none());
    assert!(auth.with_authorized("wrong", "first", || ()).is_none());
    auth.activate_for_test(SecretString::new("bb".repeat(32)));
    assert!(
        auth.with_current(&credential, || called.store(true, Ordering::SeqCst))
            .is_none()
    );
    assert!(!called.load(Ordering::SeqCst));
}
