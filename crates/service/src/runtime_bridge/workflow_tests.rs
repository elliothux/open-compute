use super::*;
use open_compute_core::{
    InstanceId, VersionId, WorkerId, WorkflowFence, WorkflowId, WorkflowInstanceId, WorkflowToken,
    WorkflowVersionId,
};
use open_compute_storage::workflows::WorkflowTarget;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

fn target() -> WorkflowTarget {
    WorkflowTarget {
        instance_id: InstanceId::generate(),
        definition_id: WorkflowId::generate(),
        definition_name: "flow".into(),
        workflow_version_id: WorkflowVersionId::generate(),
        worker_id: WorkerId::generate(),
        worker_version_id: VersionId::generate(),
        worker_code_sha256: [0x11; 32],
        class_name: "Flow".into(),
        loader_schema_version: 1,
        capability_version: 1,
        descriptor_sha256: [0x22; 32],
    }
}

#[test]
fn workflow_completion_without_an_admitted_generation_cannot_commit() {
    let transport = WorkerdTransport::new(Arc::new(Mutex::new(None)));
    let response: WorkflowDispatchResult = serde_json::from_value(serde_json::json!({
        "result": {"outcome":"terminated", "finalOrdinal":0},
        "loaderOutcome":"warm", "drainIncomplete":false,
    }))
    .unwrap();
    let called = AtomicBool::new(false);
    assert!(
        transport
            .commit_workflow(response, |_| {
                called.store(true, Ordering::SeqCst);
                Ok(())
            })
            .is_err()
    );
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn workflow_probe_unknown_and_generation_bound_completion_fail_closed() {
    let status = Arc::new(AtomicU16::new(503));
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    let app=Router::new().route("/internal/validate-workflow",post({let status=status.clone();move || {let status=status.clone();async move { (StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap(),axum::Json(serde_json::json!({"valid":true}))) }}}))
        .route("/internal/workflow",post(||async {axum::Json(serde_json::json!({"result":{"outcome":"complete","finalOrdinal":1,"outputBase64":"T0NEVgECAA=="},"loaderOutcome":"warm","drainIncomplete":false}))}));
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = receiver.await;
            })
            .await
            .unwrap();
    });

    let pins = VersionPins::new();
    let version = target();
    let mut transport = WorkerdTransport::for_test_endpoint(&port).with_version_pins(pins.clone());
    assert_eq!(
        transport.probe_workflow(&version).await.unwrap_err().code(),
        ErrorCode::WorkflowRuntimeUnavailable
    );
    status.store(422, Ordering::SeqCst);
    assert_eq!(
        transport.probe_workflow(&version).await.unwrap_err().code(),
        ErrorCode::WorkflowVersionNotReady
    );
    status.store(200, Ordering::SeqCst);
    transport.probe_workflow(&version).await.unwrap();
    let request = WorkflowRunRequest {
        fence: WorkflowFence {
            instance_id: WorkflowInstanceId::generate(),
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([3; 32]),
        },
        external_instance_id: "public-id".into(),
        definition_name: "public-name".into(),
        created_at_ms: 0,
        payload_base64: "T0NEVgECAA==".into(),
        rollback: false,
        schedule: None,
    };
    let response = transport
        .dispatch_workflow(&version, &request, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(transport.commit_workflow(response, |_| Ok(7)).unwrap(), 7);
    assert_eq!(pins.count(version.worker_version_id), 0);
    let stale = transport
        .dispatch_workflow(&version, &request, Duration::from_secs(1))
        .await
        .unwrap();

    transport.test_generation = Some(open_compute_core::StartupId::generate());
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

#[tokio::test]
async fn invalid_workflow_results_quarantine_all_transport_clones_until_rotation() {
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    let received = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(
        json!({"result":{"outcome":"errored","finalOrdinal":1,
        "errorCode":"WORKFLOW_NON_RETRYABLE"},"loaderOutcome":"warm","drainIncomplete":false}),
    ));
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
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

    let pins = VersionPins::new();
    let mut transport =
        WorkerdTransport::new(Arc::new(Mutex::new(None))).with_version_pins(pins.clone());
    // A compiled credential is not readiness. Refusal before dispatch must not
    // quarantine the generation that is about to become available.
    assert_eq!(
        transport.ensure_workflow_admission().unwrap_err().code(),
        ErrorCode::WorkflowRuntimeUnavailable
    );
    transport.test_endpoint = Some(port);
    transport.test_generation = Some(open_compute_core::StartupId::generate());
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
        payload_base64: "T0NEVgECAA==".into(),
        rollback: false,
        schedule: None,
    };
    transport
        .dispatch_workflow(&version, &request, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(pins.count(version.worker_version_id), 0);
    let invalid = [
        json!({"outcome":"errored","finalOrdinal":1,"errorCode":"private exception text"}),
        json!({"outcome":"errored","finalOrdinal":1,"errorCode":"WORKFLOW_RUNTIME_UNAVAILABLE"}),
        json!({"outcome":"complete","finalOrdinal":1025,"outputBase64":"T0NEVgECAA=="}),
        json!({"outcome":"complete","finalOrdinal":1,"outputBase64":"x".repeat(1024*1024+1)}),
        json!({"outcome":"unknown","finalOrdinal":1}),
    ];
    for bad in invalid {
        pins.clear_generation_retentions();
        transport.test_generation = Some(open_compute_core::StartupId::generate());
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
        assert_eq!(pins.count(version.worker_version_id), 1);
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
        assert_eq!(pins.count(version.worker_version_id), 1);
    }
    pins.clear_generation_retentions();
    transport.test_generation = Some(open_compute_core::StartupId::generate());
    *result.lock().unwrap() = json!({"result":{"outcome":"unknown","finalOrdinal":1},"loaderOutcome":"warm","drainIncomplete":true});
    let unknown = transport
        .dispatch_workflow(&version, &request, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(unknown.drain_incomplete);
    assert_eq!(pins.count(version.worker_version_id), 1);
    assert!(transport.ensure_workflow_admission().is_err());
    pins.clear_generation_retentions();
    assert_eq!(pins.count(version.worker_version_id), 0);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn cancelled_workflow_dispatch_remains_pinned_until_confirmed_generation_exit() {
    let sockets = tempfile::tempdir_in("/tmp").unwrap();
    let path = sockets.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let app = Router::new().route("/internal/workflow", post({
        let entered = entered.clone(); let finish = finish.clone();
        move |_: axum::body::Bytes| { let entered = entered.clone(); let finish = finish.clone(); async move {
            entered.notify_one(); finish.notified().await;
            axum::Json(serde_json::json!({"result":{"outcome":"complete","finalOrdinal":0,"outputBase64":"T0NEVgECAA=="},"loaderOutcome":"warm","drainIncomplete":false}))
        }}
    }));
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let pins = VersionPins::new();
    let transport = WorkerdTransport::for_test_endpoint(&path).with_version_pins(pins.clone());
    let version = target();
    let id = version.worker_version_id;
    let dispatch = tokio::spawn(async move {
        transport
            .dispatch_workflow(
                &version,
                &WorkflowRunRequest {
                    fence: WorkflowFence {
                        instance_id: WorkflowInstanceId::generate(),
                        instance_generation: 1,
                        run_token: WorkflowToken::from_bytes([3; 32]),
                    },
                    external_instance_id: "cancelled".to_owned(),
                    definition_name: "flow".to_owned(),
                    created_at_ms: 0,
                    payload_base64: "T0NEVgECAA==".to_owned(),
                    rollback: false,
                    schedule: None,
                },
                Duration::from_secs(5),
            )
            .await
    });
    entered.notified().await;
    dispatch.abort();
    assert!(dispatch.await.unwrap_err().is_cancelled());
    assert_eq!(pins.count(id), 1);
    assert_eq!(
        pins.fence_and_wait(id, Duration::from_millis(1))
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    finish.notify_one();
    stop.send(()).unwrap();
    server.await.unwrap();
    pins.clear_generation_retentions();
    pins.fence_and_wait(id, Duration::from_secs(1))
        .await
        .unwrap();
}
