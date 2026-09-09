use super::*;

#[tokio::test]
async fn liveness_ready_status_and_bounds() {
    let health = HealthCoordinator::new();
    let state = test_state(health.clone(), None);
    let debug = format!("{state:?}");
    assert!(debug.contains("HttpState"));
    assert!(debug.contains("worker_api: false"));
    let app = http::merged_router(state.clone());
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get(REQUEST_ID_HEADER).is_some());

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "STARTING");

    health
        .set_component(
            ComponentName::Process,
            ComponentState::Healthy,
            Some(ReadinessReason::Ready),
        )
        .unwrap();
    for name in [
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::ObjectStorage,
        ComponentName::Cache,
        ComponentName::Runtime,
        ComponentName::Scheduler,
        ComponentName::Operations,
        ComponentName::VectorizeStorage,
        ComponentName::VectorizeMutations,
        ComponentName::AiSearchStorage,
        ComponentName::AiSearchIndexing,
        ComponentName::AiModels,
    ] {
        health
            .set_component(name, ComponentState::Healthy, Some(ReadinessReason::Ready))
            .unwrap();
    }
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    health.begin_drain().unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/workers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .header("x-pad", "x".repeat(9000))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let version_path = "/client/v4/accounts/acct_test/workers/scripts/wrk_test/versions";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(version_path)
                .header("content-length", 18 * 1024 * 1024)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(version_path)
                .header("content-length", 64 * 1024 * 1024 + 1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let staged_upload_path =
        "/client/v4/accounts/acct_test/workers/scripts/wrk_test/assets-upload-session";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(staged_upload_path)
                .header("content-length", 16 * 1024)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(staged_upload_path)
                .header("content-length", 64 * 1024 * 1024 + 1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/client/v4/accounts/acct_test/storage/kv/namespaces/ns_test/bulk")
                .header("content-length", 16 * 1024)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    for method in ["PUT", "OPTIONS"] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/unknown")
                .header("x-one", "x".repeat(6000))
                .header("x-two", "x".repeat(6000))
                .header("x-three", "x".repeat(6000))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let metrics_disabled = test_state(HealthCoordinator::new(), None);
    let admin_without_metrics = http::admin_router(HttpState::for_test(
        metrics_disabled.health().clone(),
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap()),
        false,
        None,
    ));
    let res = admin_without_metrics
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = admin_without_metrics
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();
    assert_eq!(
        http::bind(occupied_addr).await.unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    http::serve_until(listener, http::merged_router(state), async {})
        .await
        .unwrap();
}
