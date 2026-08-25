use super::*;
use open_compute_core::SystemClock;
use open_compute_core::config::StorageConfig;

#[test]
fn private_paths_are_strict_and_typed() {
    let id = BindingId::generate();
    assert_eq!(
        parse_path(&format!("/internal/bindings/v1/d1/{id}/query"))
            .unwrap()
            .1,
        Operation::Query,
    );
    assert!(parse_path(&format!("/internal/bindings/v1/d1/{id}/query/extra")).is_err());
    assert!(parse_path("/internal/bindings/v1/d1/not-an-id/query").is_err());
}

#[tokio::test]
async fn service_protocol_and_error_surface_are_bounded_before_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 2,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let service = D1BindingService::new(storage.clone(), ResourcePins::new(), D1Config::default())
        .with_response_loss_once();
    assert!(format!("{service:?}").contains("D1BindingService"));
    service.arm_response_loss_once();
    ensure_d1_storage_headroom(&storage).unwrap();

    for request in [
        axum::extract::Request::builder()
            .method("GET")
            .uri("/internal/bindings/v1/d1/nope/query")
            .body(Body::empty())
            .unwrap(),
        axum::extract::Request::builder()
            .method("POST")
            .uri("/not-d1")
            .body(Body::empty())
            .unwrap(),
        axum::extract::Request::builder()
            .method("POST")
            .uri(format!(
                "/internal/bindings/v1/d1/{}/query",
                BindingId::generate()
            ))
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = service.handle(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(ERROR_HEADER).unwrap(),
            ErrorCode::D1InternalProtocolError.as_str()
        );
    }

    assert_eq!(metric_operation("/x/exec"), Some(D1MetricOperation::Exec));
    assert_eq!(metric_operation("/x/query"), Some(D1MetricOperation::Query));
    assert_eq!(metric_operation("/x"), None);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    assert!(content_type_is(&headers, "application/json"));
    headers.insert(
        "x-open-compute-descriptor-sha256",
        HeaderValue::from_str(&"01".repeat(32)).unwrap(),
    );
    assert_eq!(parse_digest(&headers).unwrap(), [1; 32]);
    headers.insert(
        "x-open-compute-request-id",
        HeaderValue::from_str(&uuid::Uuid::now_v7().hyphenated().to_string()).unwrap(),
    );
    parse_request_id(&headers).unwrap();
    headers.insert(
        "x-open-compute-descriptor-sha256",
        HeaderValue::from_static("bad"),
    );
    assert!(parse_digest(&headers).is_err());
    assert_eq!(
        response(vec![1, 2, 3], D1_FRAME_CONTENT_TYPE)
            .headers()
            .get(header::CONTENT_LENGTH)
            .unwrap(),
        "3"
    );
    assert_eq!(protocol_error().code(), ErrorCode::D1InternalProtocolError);
    assert_eq!(limit_error().code(), ErrorCode::D1LimitError);
    assert_eq!(overloaded().code(), ErrorCode::D1Overloaded);
    assert_eq!(
        permission_denied().code(),
        ErrorCode::BindingPermissionDenied
    );
    assert!(wall_now_ms() > 0);

    for (code, status) in [
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (ErrorCode::D1LimitError, StatusCode::PAYLOAD_TOO_LARGE),
        (ErrorCode::D1Overloaded, StatusCode::TOO_MANY_REQUESTS),
        (ErrorCode::D1ResultUnknown, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::D1DatabaseCorrupt,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::BindingCapabilityUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::D1TypeError, StatusCode::BAD_REQUEST),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        assert_eq!(
            error_response(&PlatformError::new(code, "sanitized")).status(),
            status
        );
    }
}

#[tokio::test]
async fn same_database_serializes_while_different_database_runs() {
    let lanes = D1HandleManager::new(2, 4, Duration::from_secs(60));
    let first_id = ResourceId::generate();
    let second_id = ResourceId::generate();
    let first = lanes
        .acquire(first_id, Duration::from_secs(1))
        .await
        .unwrap();
    let different = lanes
        .acquire(second_id, Duration::from_secs(1))
        .await
        .unwrap();
    let same = tokio::time::timeout(
        Duration::from_millis(20),
        lanes.acquire(first_id, Duration::from_secs(1)),
    )
    .await;
    assert!(same.is_err());
    drop(different);
    drop(first);
    lanes
        .acquire(first_id, Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn queue_limit_returns_stable_overload() {
    let lanes = D1HandleManager::new(1, 1, Duration::from_secs(60));
    let id = ResourceId::generate();
    let _active = lanes.acquire(id, Duration::from_secs(1)).await.unwrap();
    let lanes_for_waiter = lanes.clone();
    let waiter =
        tokio::spawn(async move { lanes_for_waiter.acquire(id, Duration::from_secs(1)).await });
    tokio::task::yield_now().await;
    let error = lanes
        .acquire(id, Duration::from_millis(10))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::D1Overloaded);
    waiter.abort();
}

#[tokio::test]
async fn handle_limit_refuses_active_eviction_and_reuses_idle_capacity() {
    let lanes = D1HandleManager::new(1, 2, Duration::from_secs(60));
    let first_id = ResourceId::generate();
    let second_id = ResourceId::generate();
    let active = lanes
        .acquire(first_id, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        lanes
            .acquire(second_id, Duration::from_millis(10))
            .await
            .err()
            .unwrap()
            .code(),
        ErrorCode::D1Overloaded,
    );
    drop(active);
    let second = lanes
        .acquire(second_id, Duration::from_secs(1))
        .await
        .unwrap();
    drop(second);
    lanes
        .acquire(first_id, Duration::from_secs(1))
        .await
        .unwrap();
}
