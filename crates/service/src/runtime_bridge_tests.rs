use super::*;

#[test]
fn tenant_headers_strip_forged_identity_and_hop_by_hop() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-open-compute-account-id",
        HeaderValue::from_static("forged"),
    );
    headers.insert("x-forwarded-for", HeaderValue::from_static("127.0.0.1"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("x-remove"));
    headers.insert("x-remove", HeaderValue::from_static("yes"));
    headers.insert("x-tenant", HeaderValue::from_static("kept"));
    let clean = sanitize_tenant_headers(headers);
    assert!(clean.get("x-open-compute-account-id").is_none());
    assert!(clean.get("x-forwarded-for").is_none());
    assert!(clean.get("x-remove").is_none());
    assert_eq!(clean.get("x-tenant").unwrap(), "kept");
}

#[test]
fn private_alarm_result_shapes_are_strict() {
    let success = AlarmDispatchResult {
        outcome: AlarmDispatchOutcome::Success,
        scheduled_time_ms: None,
        retry_count: None,
        error_code: None,
    };
    assert!(validate_alarm_dispatch_result(success).is_ok());
    let invalid_success = AlarmDispatchResult {
        outcome: AlarmDispatchOutcome::Success,
        scheduled_time_ms: Some(1),
        retry_count: None,
        error_code: None,
    };
    assert_eq!(
        validate_alarm_dispatch_result(invalid_success)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError
    );
    let retry = AlarmDispatchResult {
        outcome: AlarmDispatchOutcome::Retry,
        scheduled_time_ms: Some(2_000),
        retry_count: Some(6),
        error_code: Some("DO_RUNTIME_EXCEPTION".to_owned()),
    };
    assert!(validate_alarm_dispatch_result(retry).is_ok());

    let present = AlarmRepairResult {
        exists: true,
        scheduled_time_ms: Some(10),
        retry_count: Some(0),
        row_token: Some("00000000-0000-4000-8000-000000000000".to_owned()),
    };
    assert!(validate_alarm_repair_result(present).is_ok());
    let malformed_absent = AlarmRepairResult {
        exists: false,
        scheduled_time_ms: None,
        retry_count: None,
        row_token: Some("00000000-0000-4000-8000-000000000000".to_owned()),
    };
    assert_eq!(
        validate_alarm_repair_result(malformed_absent)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError
    );
    assert!(
        serde_json::from_str::<AlarmRepairResult>(r#"{"exists":false,"unexpected":true}"#).is_err()
    );
}

#[test]
fn queue_and_scheduled_custom_event_protocols_are_bounded_and_strict() {
    let message_id = QueueMessageId::generate().to_string();
    let request = QueueDispatchRequest {
        queue_name: "events".to_owned(),
        messages: vec![QueueDispatchMessage {
            id: message_id.clone(),
            timestamp_ms: 60_000,
            attempts: 1,
            content_type: QueueContentType::Json,
            body_base64: base64::engine::general_purpose::STANDARD.encode(br#"{"ok":true}"#),
        }],
    };
    assert!(validate_queue_dispatch_request(&request).is_ok());
    for invalid in [
        QueueDispatchRequest {
            queue_name: String::new(),
            ..request.clone()
        },
        QueueDispatchRequest {
            messages: Vec::new(),
            ..request.clone()
        },
        QueueDispatchRequest {
            messages: vec![QueueDispatchMessage {
                id: message_id.clone(),
                attempts: 0,
                ..request.messages[0].clone()
            }],
            ..request.clone()
        },
        QueueDispatchRequest {
            messages: vec![QueueDispatchMessage {
                id: message_id.clone(),
                body_base64: "not-base64".to_owned(),
                ..request.messages[0].clone()
            }],
            ..request.clone()
        },
        QueueDispatchRequest {
            messages: vec![request.messages[0].clone(), request.messages[0].clone()],
            ..request.clone()
        },
    ] {
        assert_eq!(
            validate_queue_dispatch_request(&invalid)
                .unwrap_err()
                .code(),
            ErrorCode::QueueDispositionInvalid
        );
    }

    let result = QueueDispatchResult {
        outcome: "ok".to_owned(),
        ack_all: true,
        retry_batch: QueueRetryBatchResult {
            retry: false,
            delay_seconds: None,
        },
        explicit_acks: vec![message_id.clone()],
        retry_messages: Vec::new(),
    };
    assert!(validate_queue_dispatch_result(result.clone(), 1).is_ok());
    assert_eq!(
        validate_queue_dispatch_result(
            QueueDispatchResult {
                outcome: "forged".to_owned(),
                ..result.clone()
            },
            1,
        )
        .unwrap_err()
        .code(),
        ErrorCode::QueueDispositionInvalid
    );
    assert_eq!(
        validate_queue_dispatch_result(
            QueueDispatchResult {
                retry_messages: vec![QueueRetryMessageResult {
                    msg_id: message_id,
                    delay_seconds: Some(86_401),
                }],
                ..result
            },
            1,
        )
        .unwrap_err()
        .code(),
        ErrorCode::QueueDispositionInvalid
    );

    let scheduled = ScheduledDispatchRequest {
        scheduled_time_ms: 120_000,
        cron: "*/5 * * * *".to_owned(),
    };
    assert!(validate_scheduled_dispatch_request(&scheduled).is_ok());
    assert_eq!(
        validate_scheduled_dispatch_request(&ScheduledDispatchRequest {
            scheduled_time_ms: 120_001,
            ..scheduled.clone()
        })
        .unwrap_err()
        .code(),
        ErrorCode::CronActivationStale
    );
    assert_eq!(
        validate_scheduled_dispatch_request(&ScheduledDispatchRequest {
            cron: "bad".to_owned(),
            ..scheduled
        })
        .unwrap_err()
        .code(),
        ErrorCode::CronExpressionInvalid
    );
    assert!(
        validate_scheduled_dispatch_result(ScheduledDispatchResult {
            outcome: "aborted".to_owned(),
            no_retry: false,
        })
        .is_ok()
    );
}

#[tokio::test]
async fn transport_and_source_helpers_fail_closed_without_a_generation() {
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)))
            .with_max_request_body(0);
    assert!(format!("{transport:?}").contains("WorkerdTransport"));
    let candidate = ValidationCandidate {
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        worker_code_sha256: [7; 32],
    };
    assert_eq!(
        transport
            .validate(candidate.clone())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );
    assert_eq!(
        transport
            .validate_entrypoint(candidate.clone(), "named".to_owned())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );
    let target = DispatchTarget {
        account_id: candidate.account_id,
        worker_id: candidate.worker_id,
        deployment_id: candidate.deployment_id,
        worker_code_sha256: hex::encode(candidate.worker_code_sha256),
        entrypoint: None,
        route_generation: 1,
        request_id: RequestId::generate(),
    };
    assert!(!target.loader_key().is_empty());
    assert_eq!(
        transport
            .dispatch(
                target,
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );

    assert_eq!(
        original_url(&HeaderMap::new(), &"/path".parse().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::RouteNotFound
    );
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_static("example.com"));
    assert_eq!(
        original_url(&headers, &"/path?q=1".parse().unwrap()).unwrap(),
        "http://example.com/path?q=1"
    );
    assert_eq!(
        insert_header(&mut headers, "x-test", "bad\nvalue")
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeUnavailable
    );
    assert_eq!(runtime_unavailable().code(), ErrorCode::RuntimeUnavailable);

    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    response_headers.insert("x-open-compute-private", HeaderValue::from_static("remove"));
    response_headers.insert(
        "x-open-compute-request-id",
        HeaderValue::from_static("keep"),
    );
    sanitize_response_headers(&mut response_headers);
    assert!(response_headers.get(header::CONNECTION).is_none());
    assert!(response_headers.get("x-open-compute-private").is_none());
    assert!(response_headers.get("x-open-compute-request-id").is_some());

    for (code, expected) in [
        (ErrorCode::DeploymentNotReady, StatusCode::CONFLICT),
        (
            ErrorCode::ArtifactUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::BundleInvalid, StatusCode::UNPROCESSABLE_ENTITY),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let response = source_platform_error(PlatformError::new(code, "safe"));
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers().get(ERROR_HEADER).unwrap(), code.as_str());
    }

    let listener = bind_runtime_source().await.unwrap();
    assert!(listener.local_addr().unwrap().ip().is_loopback());
}
