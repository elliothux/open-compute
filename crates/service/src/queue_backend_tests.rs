use super::*;
use crate::metrics::MetricsRegistry;
use open_compute_core::SystemClock;
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_storage::PlatformStorage;

fn private_request(method: &str, path: &str, content_type: Option<&str>, body: Vec<u8>) -> Request {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(REQUEST_HEADER, "550e8400-e29b-41d4-a716-446655440000")
        .header(OUTPUT_GATE_HEADER, "0")
        .header(VERSION_HEADER, VersionId::generate().to_string())
        .header(DESCRIPTOR_HEADER, "00".repeat(32));
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body)).unwrap()
}

fn backend_fixture() -> (tempfile::TempDir, QueueBindingService) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    (
        temp,
        QueueBindingService::new(storage, scheduler)
            .with_metrics(metrics)
            .with_concurrency_limits(2, 1),
    )
}

#[tokio::test]
async fn per_binding_waiters_do_not_hoard_global_queue_admission() {
    let budget = QueueConcurrencyBudget::new(2, 1);
    let first_binding = BindingId::generate();
    let second_binding = BindingId::generate();
    let first = budget.acquire(first_binding).await.unwrap();
    let waiting_budget = budget.clone();
    let waiting = tokio::spawn(async move { waiting_budget.acquire(first_binding).await });
    tokio::task::yield_now().await;
    let independent =
        tokio::time::timeout(Duration::from_millis(100), budget.acquire(second_binding))
            .await
            .expect("same-binding waiter must not consume a global permit")
            .unwrap();
    drop(first);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    drop(independent);
}

fn frame(operation: u8, batch_delay: i32, messages: &[(u8, i32, &[u8])]) -> Vec<u8> {
    let mut bytes = b"OCQ1".to_vec();
    bytes.push(operation);
    bytes.extend_from_slice(&u16::try_from(messages.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&batch_delay.to_be_bytes());
    for (content_type, delay, body) in messages {
        bytes.push(*content_type);
        bytes.extend_from_slice(&delay.to_be_bytes());
        bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(body);
    }
    bytes
}

#[test]
fn queue_frame_preserves_content_order_and_explicit_zero_delay() {
    let bytes = frame(
        2,
        5,
        &[
            (1, -1, b"{}"),
            (2, 0, b"text"),
            (3, 8, &[1, 2]),
            (4, -1, b"OCDV"),
        ],
    );
    let parsed = parse_frame(&bytes, QueueOperation::Batch).unwrap();
    assert_eq!(parsed.batch_delay_seconds, Some(5));
    assert_eq!(parsed.messages.len(), 4);
    assert_eq!(parsed.messages[0].content_type, QueueContentType::Json);
    assert_eq!(parsed.messages[0].delay_seconds, None);
    assert_eq!(parsed.messages[1].content_type, QueueContentType::Text);
    assert_eq!(parsed.messages[1].delay_seconds, Some(0));
    assert_eq!(parsed.messages[2].content_type, QueueContentType::Bytes);
    assert_eq!(parsed.messages[2].body, vec![1, 2]);
    assert_eq!(parsed.messages[3].content_type, QueueContentType::V8);
    assert_eq!(parsed.messages[3].body, b"OCDV");
}

#[test]
fn queue_frame_and_private_path_reject_ambiguous_or_oversized_inputs() {
    let binding = BindingId::generate();
    assert_eq!(
        parse_path(&format!("/internal/bindings/v1/queue/{binding}/send")),
        Some((binding, QueueOperation::Send))
    );
    assert_eq!(
        parse_path(&format!("/internal/bindings/v1/queue/{binding}/finalize")),
        Some((binding, QueueOperation::Finalize))
    );
    assert!(parse_path(&format!("/internal/bindings/v1/queue/{binding}/pull")).is_none());
    assert_eq!(
        parse_frame(&frame(2, -1, &[(1, -1, b"x")]), QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );
    assert_eq!(
        parse_frame(&frame(1, 86_401, &[(1, -1, b"x")]), QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueDelayInvalid
    );
    assert_eq!(
        parse_frame(&frame(1, -1, &[(9, -1, b"x")]), QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueContentTypeUnsupported
    );
    let mut trailing = frame(1, -1, &[(1, -1, b"x")]);
    trailing.push(0);
    assert_eq!(
        parse_frame(&trailing, QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );
    let oversized = vec![0_u8; usize::try_from(QUEUE_MAX_MESSAGE_BYTES).unwrap() + 1];
    assert_eq!(
        parse_frame(&frame(1, -1, &[(3, -1, &oversized)]), QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueMessageTooLarge
    );
}

#[test]
fn queue_private_request_id_accepts_canonical_transport_uuids_only() {
    let mut headers = HeaderMap::new();
    headers.insert(
        REQUEST_HEADER,
        HeaderValue::from_static("550e8400-e29b-41d4-a716-446655440000"),
    );
    assert!(parse_request_id(&headers).is_some());

    headers.insert(
        REQUEST_HEADER,
        HeaderValue::from_static("550E8400-E29B-41D4-A716-446655440000"),
    );
    assert!(parse_request_id(&headers).is_none());
    headers.insert(REQUEST_HEADER, HeaderValue::from_static("not-a-uuid"));
    assert!(parse_request_id(&headers).is_none());
    headers.remove(REQUEST_HEADER);
    assert!(parse_request_id(&headers).is_none());
}

#[test]
fn queue_backend_error_surface_is_stable_and_body_free() {
    let response = platform_error(&PlatformError::new(
        ErrorCode::QueueSendResultUnknown,
        "secret body must not escape",
    ));
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(ERROR_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("QUEUE_SEND_RESULT_UNKNOWN")
    );
}

#[tokio::test]
async fn queue_backend_rejects_every_unauthorized_protocol_shape_before_mutation() {
    let (_temp, backend) = backend_fixture();
    let binding = BindingId::generate();
    let send = format!("/internal/bindings/v1/queue/{binding}/send");
    let batch = format!("/internal/bindings/v1/queue/{binding}/batch");
    let metrics = format!("/internal/bindings/v1/queue/{binding}/metrics");

    let response = backend
        .handle(
            Request::builder()
                .uri("/invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = backend
        .handle(private_request(
            "GET",
            &send,
            Some(FRAME_CONTENT_TYPE),
            frame(1, -1, &[(2, -1, b"message")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = backend
        .handle(
            Request::builder()
                .method("POST")
                .uri(&send)
                .header(header::CONTENT_TYPE, FRAME_CONTENT_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let mut missing_version = private_request(
        "POST",
        &send,
        Some(FRAME_CONTENT_TYPE),
        frame(1, -1, &[(2, -1, b"message")]),
    );
    missing_version.headers_mut().remove(VERSION_HEADER);
    assert_eq!(
        backend.handle(missing_version).await.status(),
        StatusCode::BAD_REQUEST
    );

    let mut missing_descriptor = private_request(
        "POST",
        &send,
        Some(FRAME_CONTENT_TYPE),
        frame(1, -1, &[(2, -1, b"message")]),
    );
    missing_descriptor.headers_mut().remove(DESCRIPTOR_HEADER);
    assert_eq!(
        backend.handle(missing_descriptor).await.status(),
        StatusCode::BAD_REQUEST
    );

    for (path, content_type) in [(&metrics, FRAME_CONTENT_TYPE), (&send, "application/json")] {
        assert_eq!(
            backend
                .handle(private_request(
                    "POST",
                    path,
                    Some(content_type),
                    Vec::new()
                ))
                .await
                .status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    let mut declared = private_request(
        "POST",
        &batch,
        Some(FRAME_CONTENT_TYPE),
        frame(2, -1, &[(2, -1, b"message")]),
    );
    declared.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&(MAX_FRAME_BYTES + 1).to_string()).unwrap(),
    );
    assert_eq!(
        backend.handle(declared).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    assert_eq!(
        backend
            .handle(private_request(
                "POST",
                &send,
                Some(FRAME_CONTENT_TYPE),
                b"malformed".to_vec(),
            ))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    assert_eq!(
        backend
            .handle(private_request(
                "POST",
                &send,
                Some(FRAME_CONTENT_TYPE),
                frame(1, -1, &[(2, -1, b"message")]),
            ))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        backend
            .handle(private_request(
                "POST",
                &metrics,
                Some("application/json; charset=utf-8"),
                Vec::new(),
            ))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn queue_frame_limits_and_error_status_mapping_are_complete() {
    assert_eq!(
        parse_path(&format!(
            "/internal/bindings/v1/queue/{}/batch",
            BindingId::generate()
        ))
        .unwrap()
        .1,
        QueueOperation::Batch
    );
    assert_eq!(
        parse_path(&format!(
            "/internal/bindings/v1/queue/{}/metrics",
            BindingId::generate()
        ))
        .unwrap()
        .1,
        QueueOperation::Metrics
    );
    assert!(parse_path("/internal/bindings/v1/queue/bad/send").is_none());
    assert!(parse_path("/internal/bindings/v1/queue").is_none());
    assert!(
        parse_path(&format!(
            "/internal/bindings/v1/queue/{}/send/extra",
            BindingId::generate()
        ))
        .is_none()
    );

    for bytes in [Vec::new(), b"OCQ1".to_vec(), b"BAD10000000".to_vec()] {
        assert_eq!(
            parse_frame(&bytes, QueueOperation::Send)
                .unwrap_err()
                .code(),
            ErrorCode::QueueInvariantViolation
        );
    }
    let mut zero = frame(1, -1, &[(2, -1, b"x")]);
    zero[5..7].copy_from_slice(&0_u16.to_be_bytes());
    zero.truncate(11);
    assert_eq!(
        parse_frame(&zero, QueueOperation::Send).unwrap_err().code(),
        ErrorCode::QueueBatchLimitExceeded
    );
    let too_many = usize::try_from(QUEUE_MAX_BATCH_MESSAGES).unwrap() + 1;
    let mut count = b"OCQ1".to_vec();
    count.push(2);
    count.extend_from_slice(&u16::try_from(too_many).unwrap().to_be_bytes());
    count.extend_from_slice(&(-1_i32).to_be_bytes());
    assert_eq!(
        parse_frame(&count, QueueOperation::Batch)
            .unwrap_err()
            .code(),
        ErrorCode::QueueBatchLimitExceeded
    );
    assert_eq!(
        parse_frame(&frame(1, -2, &[(2, -1, b"x")]), QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueDelayInvalid
    );
    let mut truncated = frame(1, -1, &[(2, -1, b"body")]);
    truncated.pop();
    assert_eq!(
        parse_frame(&truncated, QueueOperation::Send)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );

    for (code, status) in [
        (ErrorCode::QueueNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::QueueNotReady, StatusCode::CONFLICT),
        (
            ErrorCode::QueueMessageTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            ErrorCode::QueueContentTypeUnsupported,
            StatusCode::BAD_REQUEST,
        ),
        (
            ErrorCode::QueueBacklogLimitExceeded,
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (ErrorCode::StoragePressure, StatusCode::SERVICE_UNAVAILABLE),
        (ErrorCode::Internal, StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        assert_eq!(
            platform_error(&PlatformError::new(code, "not returned")).status(),
            status
        );
    }
    assert!(unix_ms() > 0);
}
