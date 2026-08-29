use super::*;
use open_compute_core::{
    AccountId, DeploymentId, SecretString, WorkerId, WorkflowFence, WorkflowId, WorkflowInstanceId,
    WorkflowToken, WorkflowVersionId,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::WorkflowTarget;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

fn target() -> WorkflowTarget {
    WorkflowTarget {
        account_id: AccountId::generate(),
        definition_id: WorkflowId::generate(),
        definition_name: "flow".into(),
        version_id: WorkflowVersionId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        worker_code_sha256: [0x11; 32],
        class_name: "Flow".into(),
        loader_schema_version: 1,
        capability_version: 1,
        descriptor_sha256: [0x22; 32],
    }
}

#[tokio::test]
async fn workflow_probe_unknown_and_generation_bound_completion_fail_closed() {
    let status = Arc::new(AtomicU16::new(503));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app=Router::new().route("/internal/validate-workflow",post({let status=status.clone();move || {let status=status.clone();async move { (StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap(),axum::Json(serde_json::json!({"valid":true}))) }}}))
        .route("/internal/workflow",post(||async {axum::Json(serde_json::json!({"result":{"outcome":"complete","finalOrdinal":1,"outputJson":"null"},"loaderOutcome":"warm","drainIncomplete":false}))}));
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

#[tokio::test]
async fn invalid_workflow_results_quarantine_all_transport_clones_until_rotation() {
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    let received = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(
        json!({"result":{"outcome":"errored","finalOrdinal":1,
        "errorCode":"WORKFLOW_NON_RETRYABLE"},"loaderOutcome":"warm","drainIncomplete":false}),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/internal/workflow",
        post({
            let received = received.clone();
            let result = result.clone();
            move |request: Request| {
                let received = received.clone();
                let result = result.clone();
                async move {
                    to_bytes(request.into_body(), 8192).await.unwrap();
                    received.fetch_add(1, Ordering::SeqCst);
                    axum::Json(result.lock().unwrap().clone())
                }
            }
        }),
    );
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
    auth.activate_for_test(SecretString::new("00".repeat(32)));
    let mut transport = WorkerdTransport::new(auth.clone(), Arc::new(Mutex::new(None)));
    // A compiled credential is not readiness. Refusal before dispatch must not
    // quarantine the generation that is about to become available.
    assert_eq!(
        transport.ensure_workflow_admission().unwrap_err().code(),
        ErrorCode::WorkflowRuntimeUnavailable
    );
    transport.test_endpoint = Some(port);
    transport.ensure_workflow_admission().unwrap();
    let version = target();
    let request = WorkflowRunRequest {
        fence: WorkflowFence {
            instance_id: WorkflowInstanceId::generate(),
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([3; 32]),
        },
        external_instance_id: "external".into(),
        definition_name: "flow".into(),
        created_at_ms: 0,
        payload_json: "null".into(),
    };
    transport
        .dispatch_workflow(&version, &request, Duration::from_secs(1))
        .await
        .unwrap();
    let invalid = [
        json!({"outcome":"errored","finalOrdinal":1,"errorCode":"private exception text"}),
        json!({"outcome":"errored","finalOrdinal":1,"errorCode":"WORKFLOW_RUNTIME_UNAVAILABLE"}),
        json!({"outcome":"complete","finalOrdinal":1025,"outputJson":"null"}),
        json!({"outcome":"complete","finalOrdinal":1,"outputJson":"x".repeat(1024*1024+1)}),
        json!({"outcome":"unknown","finalOrdinal":1}),
    ];
    for (index, bad) in invalid.into_iter().enumerate() {
        auth.activate_for_test(SecretString::new(format!("{:064x}", index + 1)));
        result.lock().unwrap()["result"] = bad;
        let before = received.load(Ordering::SeqCst);
        assert_eq!(
            transport
                .dispatch_workflow(&version, &request, Duration::from_secs(1))
                .await
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowRuntimeUnavailable
        );
        assert_eq!(received.load(Ordering::SeqCst), before + 1);
        assert_eq!(
            transport
                .clone()
                .dispatch_workflow(&version, &request, Duration::from_secs(1))
                .await
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowRuntimeUnavailable
        );
        assert_eq!(received.load(Ordering::SeqCst), before + 1);
    }
    shutdown.send(()).unwrap();
    server.await.unwrap();
}
