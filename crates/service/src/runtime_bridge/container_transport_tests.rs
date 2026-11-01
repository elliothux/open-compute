use super::*;

#[tokio::test]
async fn container_start_control_is_private_closed_and_bounded() {
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing::instrument::WithSubscriber as _;

    let directory = tempfile::TempDir::new().unwrap();
    let log_path = directory.path().join("dispatch.log");
    let log_file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&log_path)
        .unwrap();
    let subscriber = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_target(false)
            .with_current_span(false)
            .with_span_list(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(Arc::new(log_file))
            .finish(),
    );

    let namespace = open_compute_core::ResourceId::generate();
    let mut object_bytes = [7; 32];
    object_bytes[..8].copy_from_slice(&open_compute_core::durable_object_namespace_prefix(
        namespace,
    ));
    let object =
        open_compute_core::DurableObjectId::for_namespace(object_bytes, namespace).unwrap();
    let expected = serde_json::json!({
        "namespaceResourceId": namespace,
        "objectId": object,
        "objectGeneration": 3,
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();

    let app = Router::new().fallback(move |request: Request| {
        let expected = expected.clone();
        let calls = calls.clone();
        async move {
            assert_eq!(request.method(), Method::POST);
            assert_eq!(request.uri().path(), "/internal/container-start");
            let bytes = to_bytes(request.into_body(), 4096).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                expected
            );
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => "{\"started\":true}".into_response(),
                1 => "{\"started\":false}".into_response(),
                2 => "{\"started\":true,\"untrusted\":1}".into_response(),
                3 => "invalid".into_response(),
                4 => vec![b'x'; 4097].into_response(),
                5 => (
                    StatusCode::UNAUTHORIZED,
                    [(ERROR_HEADER, "DO_VERSION_STALE")],
                )
                    .into_response(),
                6 => (
                    StatusCode::UNAUTHORIZED,
                    [(ERROR_HEADER, "untrusted-header-value")],
                )
                    .into_response(),
                _ => StatusCode::BAD_GATEWAY.into_response(),
            }
        }
    });
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = receiver.await;
            })
            .await
            .unwrap();
    });
    let transport = WorkerdTransport::for_test_endpoint(&port);
    transport
        .start_container(namespace, object, 3)
        .await
        .unwrap();

    for _ in 0..7 {
        let error = transport
            .start_container(namespace, object, 3)
            .with_subscriber(subscriber.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::RuntimeUnavailable);
        assert!(!error.to_string().contains("untrusted"));
    }
    assert_eq!(observed.load(Ordering::SeqCst), 8);
    let inactive = WorkerdTransport::new(Arc::new(Mutex::new(None)));
    assert!(
        inactive
            .start_container(namespace, object, 3)
            .with_subscriber(subscriber.clone())
            .await
            .is_err()
    );
    assert_eq!(observed.load(Ordering::SeqCst), 8);
    let log = std::fs::read_to_string(&log_path).unwrap();
    let diagnostics: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|event| event["fields"]["message"] == "Controlled native DO dispatch rejection")
        .map(|event| event["fields"].clone())
        .collect();
    assert_eq!(diagnostics.len(), 3);
    assert_eq!(diagnostics[0]["code"], "DO_VERSION_STALE");
    assert_eq!(diagnostics[0]["status"], 401);
    assert_eq!(diagnostics[1]["code"], "UNRECOGNIZED_CODE");
    assert_eq!(diagnostics[2]["code"], "UNRECOGNIZED_CODE");
    assert_eq!(diagnostics[2]["status"], 502);
    assert!(!log.contains("untrusted-header-value"));
    drop(transport);
    shutdown.send(()).unwrap();
    server.await.unwrap();
}
