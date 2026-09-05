use super::*;
use open_compute_core::SystemClock;
use open_compute_core::config::DataConfig;
use open_compute_core::{BindingKind, RequestId};
use open_compute_storage::{D1ExportOptions, D1Migration, D1TransferState};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, D1ResourceDriver, ResourceController,
};
use sha2::{Digest as _, Sha256};

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
            &DataConfig {
                path: root.clone(),
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
    assert_eq!(
        crate::d1_session::session_error().code(),
        ErrorCode::D1SessionError
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
        (ErrorCode::D1SessionError, StatusCode::BAD_REQUEST),
        (ErrorCode::D1DumpError, StatusCode::BAD_REQUEST),
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

#[tokio::test]
async fn sql_transfer_and_time_travel_round_trip_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
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
    let account = storage.identity().default_account_id;
    let source = create_database(&storage, account, "transfer-source", 10);
    let destination = create_database(&storage, account, "transfer-destination", 11);
    let service = D1BindingService::new(storage.clone(), ResourcePins::new(), D1Config::default());
    service.user_version(account, source).await.unwrap();
    service.user_version(account, destination).await.unwrap();

    let sql = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\
               INSERT INTO notes VALUES (1, 'before')";
    service
        .apply_migrations(
            account,
            source,
            vec![D1Migration {
                id: 1,
                name: "0001_notes.sql".to_owned(),
                sql: sql.to_owned(),
                sha256: Sha256::digest(sql.as_bytes()).into(),
            }],
            20,
        )
        .await
        .unwrap();
    let restore_point = service
        .time_travel_bookmark(account, source, None)
        .await
        .unwrap();

    let export = service
        .begin_export(account, source, D1ExportOptions::default())
        .await
        .unwrap();
    assert_eq!(export.transfer.state, D1TransferState::Complete);
    let bytes = service
        .download_export(
            account,
            source,
            export.transfer.id.clone(),
            export.token.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .download_export(
                account,
                source,
                export.transfer.id.clone(),
                "wrong".to_owned()
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound,
    );

    let etag: [u8; 16] = md5::Md5::digest(&bytes).into();
    let import = service
        .begin_import(account, destination, etag)
        .await
        .unwrap();
    let uploaded = service
        .upload_import(
            account,
            destination,
            import.transfer.id.clone(),
            import.token.clone(),
            bytes.clone(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.state, D1TransferState::Uploaded);
    service
        .upload_import(
            account,
            destination,
            import.transfer.id.clone(),
            import.token,
            bytes,
        )
        .await
        .unwrap();
    let imported = service
        .ingest_import(account, destination, import.transfer.id.clone())
        .await
        .unwrap();
    assert_eq!(imported.state, D1TransferState::Complete);
    assert_eq!(imported.num_queries, Some(2));
    assert!(imported.duration_ms.is_some_and(|value| value >= 0.0));
    assert_eq!(imported.rows_read, Some(0));
    assert_eq!(imported.rows_written, Some(1));
    assert!(imported.result_size_after.is_some_and(|value| value > 0));
    let rows = service
        .operator_query(
            account,
            destination,
            "SELECT body FROM notes ORDER BY id".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(rows.rows, vec![vec![D1Value::Text("before".to_owned())]]);

    service
        .operator_query(
            account,
            source,
            "INSERT INTO notes VALUES (2, 'after')".to_owned(),
        )
        .await
        .unwrap();
    let restored = service
        .time_travel_restore(account, source, D1TimeTravelTarget::Bookmark(restore_point))
        .await
        .unwrap();
    let current = service
        .time_travel_bookmark(account, source, None)
        .await
        .unwrap();
    assert_eq!(
        storage
            .crypto()
            .open_d1_bookmark(account, source, &restored)
            .unwrap(),
        storage
            .crypto()
            .open_d1_bookmark(account, source, &current)
            .unwrap(),
    );
    let rows = service
        .operator_query(
            account,
            source,
            "SELECT body FROM notes ORDER BY id".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(rows.rows, vec![vec![D1Value::Text("before".to_owned())]]);

    drop(service);
    let restarted = D1BindingService::new(storage, ResourcePins::new(), D1Config::default());
    assert_eq!(
        restarted
            .transfer(account, destination, import.transfer.id)
            .await
            .unwrap()
            .state,
        D1TransferState::Complete,
    );
}

fn create_database(
    storage: &Arc<PlatformStorage>,
    account: AccountId,
    name: &str,
    now_ms: i64,
) -> ResourceId {
    match ResourceController::new(
        storage.as_ref(),
        ResourcePins::new(),
        D1ResourceDriver::new(storage.as_ref(), 256 * 1024 * 1024),
    )
    .create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::D1Database,
        name: name.to_owned(),
        idempotency_key: name.to_owned(),
        driver_schema_version: open_compute_storage::D1_DATABASE_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms,
    })
    .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first D1 create replayed"),
    }
}
