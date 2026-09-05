use super::kv::MAX_FRAME_BODY_BYTES;
use super::*;
use crate::metrics::MetricsRegistry;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, DurableObjectsConfig, MetricsConfig, QueuesConfig};
use open_compute_core::{
    AccountId, CanonicalBindingConfig, CanonicalPermissions, RequestId, ResourceAvailability,
    ResourceId, ResourceState, SecretString,
};
use open_compute_storage::{ResourceRecord, VersionBindingRecord};

#[test]
fn unavailable_executor_fails_closed() {
    assert_eq!(unavailable().code(), ErrorCode::ResourceUnavailable);
    let binding = authorized_binding();
    let executor = UnavailableKvBindingExecutor;
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::Delete {
                    key: "key".to_owned()
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::ResourceUnavailable
    );
    assert_eq!(
        executor
            .stream_get(&binding, "key", None, &mut |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ResourceUnavailable
    );
}

#[test]
fn protocol_parsers_and_error_mapping_cover_every_stable_class() {
    let version = VersionId::generate();
    let request = RequestId::generate();
    let mut headers = HeaderMap::new();
    headers.insert(VERSION_HEADER, version.to_string().parse().unwrap());
    headers.insert(REQUEST_HEADER, request.to_string().parse().unwrap());
    headers.insert(DESCRIPTOR_HEADER, "ab".repeat(32).parse().unwrap());
    headers.insert(header::CONTENT_TYPE, FRAME_CONTENT_TYPE.parse().unwrap());
    assert_eq!(
        parse_header::<VersionId>(&headers, VERSION_HEADER).unwrap(),
        version
    );
    assert_eq!(parse_digest(&headers).unwrap(), [0xab; 32]);
    assert!(valid_request_id(&headers));
    assert!(content_type_is(&headers, FRAME_CONTENT_TYPE));
    for rejected in [
        "application/vnd.open-compute.kv.v1+json",
        "application/vnd.open-compute.kv.v1+octet-stream",
    ] {
        headers.insert(header::CONTENT_TYPE, rejected.parse().unwrap());
        assert!(!content_type_is(&headers, FRAME_CONTENT_TYPE));
    }
    headers.insert(REQUEST_HEADER, "not-a-uuid".parse().unwrap());
    assert!(!valid_request_id(&headers));
    headers.remove(REQUEST_HEADER);
    assert!(!valid_request_id(&headers));
    headers.insert(DESCRIPTOR_HEADER, "bad".parse().unwrap());
    assert_eq!(
        parse_digest(&headers).unwrap_err().code(),
        ErrorCode::BindingProtocolError
    );
    assert_eq!(
        parse_json::<DoResolveRequest>(br#"{"key":"x","extra":true}"#)
            .err()
            .unwrap()
            .code(),
        ErrorCode::BindingProtocolError
    );

    for (code, status) in [
        (ErrorCode::BindingNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::ServiceEntrypointNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::DoNamespaceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (ErrorCode::ServiceBindingDenied, StatusCode::FORBIDDEN),
        (
            ErrorCode::BindingLimitExceeded,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (ErrorCode::ResourceNotReady, StatusCode::CONFLICT),
        (ErrorCode::ResourceReferenced, StatusCode::CONFLICT),
        (ErrorCode::ServiceTargetNotReady, StatusCode::CONFLICT),
        (
            ErrorCode::ResourceUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::KvResultUnknown, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::ServiceUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::ServiceTimeout, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::ServiceLimitExceeded,
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            ErrorCode::SchedulerInternalProtocolError,
            StatusCode::BAD_REQUEST,
        ),
        (ErrorCode::DoClassNotFound, StatusCode::UNPROCESSABLE_ENTITY),
        (
            ErrorCode::DoRuntimeException,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::BindingTypeMismatch,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::BindingCapabilityUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::ResourceInvariantViolation,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::KvCorrupt, StatusCode::UNPROCESSABLE_ENTITY),
        (ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let response = platform_error(&PlatformError::new(code, "test"));
        assert_eq!(response.status(), status);
        assert_eq!(response.headers().get(ERROR_HEADER).unwrap(), code.as_str());
    }
    assert_eq!(
        alarm_protocol_error().code(),
        ErrorCode::SchedulerInternalProtocolError
    );
}

#[tokio::test]
async fn error_envelope_exposes_only_stable_retry_semantics() {
    for (code, retryable, result_unknown) in [
        (ErrorCode::BindingNotFound, false, false),
        (ErrorCode::ResourceNotReady, true, false),
        (ErrorCode::ResourceUnavailable, true, false),
        (ErrorCode::BindingProtocolError, true, false),
        (ErrorCode::KvResultUnknown, false, true),
    ] {
        let response = platform_error(&PlatformError::new(code, "must not escape"));
        let body = to_bytes(response.into_body(), MAX_FRAME_BODY_BYTES)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], code.as_str());
        assert_eq!(value["error"]["retryable"], retryable);
        assert_eq!(value["error"]["resultUnknown"], result_unknown);
        assert!(
            !String::from_utf8(body.to_vec())
                .unwrap()
                .contains("must not escape")
        );
    }
}

#[tokio::test]
async fn authenticated_boundary_rejects_before_lookup_and_observes_metrics() {
    let (_temp, storage) = storage();
    let auth = GenerationAuthRegistry::new();
    let token = "ab".repeat(32);
    auth.activate_for_test(SecretString::new(&token));
    let state = BackendState {
        storage,
        auth,
        pins: ResourcePins::new(),
        executor: Arc::new(UnavailableKvBindingExecutor),
        metrics: Some(Arc::new(
            MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap(),
        )),
        stream_budget: StreamBudget::new(2, 1),
        r2: None,
        d1: None,
        do_config: DurableObjectsConfig::default(),
        scheduler: None,
        queue: None,
        workflow: None,
        assets: None,
        services: None,
        cache: None,
        images: None,
        document_parser: None,
        ai_search: None,
    };
    let binding_id = BindingId::generate();
    let path = format!("/internal/bindings/v1/kv/{binding_id}/get");

    let unauthenticated = handle(
        State(state.clone()),
        Request::builder().uri(&path).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(unauthenticated.status(), StatusCode::NOT_FOUND);

    let wrong_method = handle(
        State(state.clone()),
        base_request(Method::GET, &path, &token, "generation")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);

    let too_large = handle(
        State(state.clone()),
        base_request(Method::POST, &path, &token, "generation")
            .header(header::CONTENT_LENGTH, MAX_FRAME_BODY_BYTES + 1)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let invalid_path = handle(
        State(state.clone()),
        base_request(Method::POST, "/invalid", &token, "generation")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid_path.status(), StatusCode::NOT_FOUND);

    let missing_version = handle(
        State(state.clone()),
        base_request(Method::POST, &path, &token, "generation")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(missing_version.status(), StatusCode::BAD_REQUEST);

    let version = VersionId::generate();
    let invalid_request_id = handle(
        State(state.clone()),
        base_request(Method::POST, &path, &token, "generation")
            .header(VERSION_HEADER, version.to_string())
            .header(REQUEST_HEADER, "bad")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid_request_id.status(), StatusCode::BAD_REQUEST);

    let invalid_digest = handle(
        State(state.clone()),
        authorized_request(&path, &token, version)
            .header(DESCRIPTOR_HEADER, "bad")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid_digest.status(), StatusCode::BAD_REQUEST);

    let wrong_content_type = handle(
        State(state.clone()),
        authorized_request(&path, &token, version)
            .header(DESCRIPTOR_HEADER, "ab".repeat(32))
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        wrong_content_type.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    for unavailable_product in [
        "/internal/ai/to-markdown/v1/parse",
        "/internal/ai-search/v1/call",
        "/internal/services/v1/call",
        "/internal/assets/v1/fetch",
        "/internal/cache/v1/match",
        "/internal/images/v1/info",
        "/internal/workflows/runs/missing",
        "/internal/bindings/v1/queue/missing/send",
        "/internal/bindings/v1/r2/missing/get",
        "/internal/bindings/v1/d1/missing/query",
    ] {
        let response = handle(
            State(state.clone()),
            base_request(Method::POST, unavailable_product, &token, "generation")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{unavailable_product}"
        );
    }

    let missing_binding = handle(
        State(state),
        authorized_request(&path, &token, version)
            .header(DESCRIPTOR_HEADER, "ab".repeat(32))
            .header(header::CONTENT_TYPE, FRAME_CONTENT_TYPE)
            .body(Body::from(r#"{"key":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(missing_binding.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn alarm_and_durable_object_protocols_reject_malformed_frames() {
    let (_temp, storage) = storage();
    let auth = GenerationAuthRegistry::new();
    let token = "ab".repeat(32);
    auth.activate_for_test(SecretString::new(&token));
    let state = BackendState {
        storage,
        auth,
        pins: ResourcePins::new(),
        executor: Arc::new(UnavailableKvBindingExecutor),
        metrics: None,
        stream_budget: StreamBudget::new(2, 1),
        r2: None,
        d1: None,
        do_config: DurableObjectsConfig::default(),
        scheduler: None,
        queue: None,
        workflow: None,
        assets: None,
        services: None,
        cache: None,
        images: None,
        document_parser: None,
        ai_search: None,
    };
    let binding_id = BindingId::generate();
    let version = VersionId::generate();

    let valid_request_id = "550e8400-e29b-41d4-a716-446655440000";
    let alarm_request = |path: &str, body: Body| {
        base_request(Method::POST, path, &token, "generation")
            .header(header::CONTENT_TYPE, "application/json")
            .header(REQUEST_HEADER, valid_request_id)
            .body(body)
            .unwrap()
    };
    for (request, status) in [
        (
            base_request(
                Method::POST,
                "/internal/alarms/v1/resolve",
                &token,
                "generation",
            )
            .body(Body::empty())
            .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            alarm_request("/internal/alarms/v1/unknown", Body::empty()),
            StatusCode::NOT_FOUND,
        ),
        (
            alarm_request("/internal/alarms/v1/resolve", Body::from("{}")),
            StatusCode::BAD_REQUEST,
        ),
        (
            alarm_request("/internal/alarms/v1/resolve", Body::from(vec![b'x'; 4097])),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        assert_eq!(handle(State(state.clone()), request).await.status(), status);
    }

    let ready_path = format!("/internal/bindings/v1/do/{binding_id}/ready");
    let ready_request = |body: Body| {
        authorized_request(&ready_path, &token, version)
            .header(header::CONTENT_TYPE, "application/json")
            .header(DESCRIPTOR_HEADER, "ab".repeat(32))
            .body(body)
            .unwrap()
    };
    for (request, status) in [
        (
            base_request(
                Method::POST,
                "/internal/bindings/v1/do/invalid/ready",
                &token,
                "generation",
            )
            .body(Body::empty())
            .unwrap(),
            StatusCode::NOT_FOUND,
        ),
        (
            base_request(Method::POST, &ready_path, &token, "generation")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            base_request(Method::POST, &ready_path, &token, "generation")
                .header(header::CONTENT_TYPE, "application/json")
                .header(REQUEST_HEADER, valid_request_id)
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            authorized_request(&ready_path, &token, version)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (ready_request(Body::from("{}")), StatusCode::BAD_REQUEST),
        (
            ready_request(Body::from(vec![b'x'; 4097])),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        assert_eq!(handle(State(state.clone()), request).await.status(), status);
    }

    let resolve_path = format!("/internal/bindings/v1/do/{binding_id}/resolve");
    let resolve_request = |body: Body| {
        authorized_request(&resolve_path, &token, version)
            .header("x-open-compute-do-operation", "fetch")
            .header(header::CONTENT_TYPE, "application/json")
            .header(DESCRIPTOR_HEADER, "ab".repeat(32))
            .header(ROUTE_GENERATION_HEADER, "1")
            .body(body)
            .unwrap()
    };
    for (request, status) in [
        (
            base_request(Method::POST, &resolve_path, &token, "generation")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            base_request(
                Method::POST,
                "/internal/bindings/v1/do/invalid/resolve",
                &token,
                "generation",
            )
            .header("x-open-compute-do-operation", "fetch")
            .body(Body::empty())
            .unwrap(),
            StatusCode::NOT_FOUND,
        ),
        (
            base_request(Method::POST, &resolve_path, &token, "generation")
                .header("x-open-compute-do-operation", "fetch")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            authorized_request(&resolve_path, &token, version)
                .header("x-open-compute-do-operation", "fetch")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            base_request(Method::POST, &resolve_path, &token, "generation")
                .header("x-open-compute-do-operation", "fetch")
                .header(header::CONTENT_TYPE, "application/json")
                .header(REQUEST_HEADER, valid_request_id)
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            authorized_request(&resolve_path, &token, version)
                .header("x-open-compute-do-operation", "fetch")
                .header(header::CONTENT_TYPE, "application/json")
                .header(DESCRIPTOR_HEADER, "ab".repeat(32))
                .header(ROUTE_GENERATION_HEADER, "0")
                .body(Body::empty())
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (resolve_request(Body::from("{}")), StatusCode::BAD_REQUEST),
        (
            resolve_request(Body::from(r#"{"objectId":"invalid"}"#)),
            StatusCode::BAD_REQUEST,
        ),
        (
            resolve_request(Body::from(vec![b'x'; 4097])),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        assert_eq!(handle(State(state.clone()), request).await.status(), status);
    }
}

#[tokio::test]
async fn listener_wrappers_bind_and_shutdown_cleanly() {
    let (_temp, storage) = storage();
    let listener = bind_binding_backend().await.unwrap();
    assert!(listener.local_addr().unwrap().ip().is_loopback());
    serve_binding_backend(
        listener,
        storage,
        GenerationAuthRegistry::new(),
        ResourcePins::new(),
        Arc::new(UnavailableKvBindingExecutor),
        None,
        None,
        None,
        open_compute_core::DurableObjectsConfig::default(),
        QueuesConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        None,
        async {},
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn document_parser_composition_wrapper_binds_every_owned_product_authority() {
    let fixture = crate::p3_3_test_support::RuntimeFeatureFixture::create(Default::default()).await;
    let pins = open_compute_workers::VersionPins::new();
    let assets = Arc::new(crate::asset_backend::AssetBindingService::new(
        fixture.storage.clone(),
        fixture.artifacts.clone(),
        fixture.artifact_cache.clone(),
        pins.clone(),
    ));
    let services = Arc::new(crate::service_invocations::ServiceInvocationRegistry::new(
        fixture.storage.clone(),
        pins,
    ));
    let parser = Arc::new(
        crate::document_parser_backend::DocumentParserBindingService::with_executable(
            fixture.storage.clone(),
            open_compute_core::DocumentParserConfig::default(),
            std::env::current_exe().unwrap(),
        ),
    );
    let listener = bind_binding_backend().await.unwrap();
    serve_binding_backend_with_document_parser(
        listener,
        fixture.storage,
        GenerationAuthRegistry::new(),
        ResourcePins::new(),
        Arc::new(UnavailableKvBindingExecutor),
        None,
        None,
        None,
        DurableObjectsConfig::default(),
        QueuesConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        None,
        assets,
        services,
        None,
        None,
        parser,
        async {},
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn service_invocation_boundary_rejects_unknown_unauthorized_and_malformed_requests() {
    let (_temp, storage) = storage();
    let registry = crate::service_invocations::ServiceInvocationRegistry::new(
        storage,
        open_compute_workers::VersionPins::new(),
    );
    let auth = GenerationAuthRegistry::new();
    let token = "ab".repeat(32);
    auth.activate_for_test(SecretString::new(&token));
    let metrics = MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap();

    let unknown = handle_service_invocation(
        &registry,
        &auth,
        &token,
        "generation",
        Some(&metrics),
        base_request(
            Method::POST,
            "/internal/services/v1/unknown",
            &token,
            "generation",
        )
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let unauthorized = handle_service_invocation(
        &registry,
        &auth,
        "cdcd",
        "generation",
        Some(&metrics),
        Request::builder()
            .uri("/internal/services/v1/unknown")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unauthorized.status(), StatusCode::NOT_FOUND);

    let malformed = handle_service_invocation(
        &registry,
        &auth,
        &token,
        "generation",
        Some(&metrics),
        Request::builder()
            .uri("/internal/services/v1/capabilities/begin")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let oversized = handle_service_invocation(
        &registry,
        &auth,
        &token,
        "generation",
        None,
        Request::builder()
            .uri("/internal/services/v1/resolve")
            .body(Body::from(vec![b'x'; 16 * 1024 + 1]))
            .unwrap(),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
}

fn base_request(
    method: Method,
    path: &str,
    token: &str,
    generation: &str,
) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(TOKEN_HEADER, token)
        .header(GENERATION_HEADER, generation)
}

fn authorized_request(path: &str, token: &str, version: VersionId) -> axum::http::request::Builder {
    base_request(Method::POST, path, token, "generation")
        .header(VERSION_HEADER, version.to_string())
        .header(REQUEST_HEADER, "550e8400-e29b-41d4-a716-446655440000")
}

pub(super) fn storage() -> (tempfile::TempDir, Arc<PlatformStorage>) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &SystemClock,
    )
    .unwrap();
    (temp, Arc::new(storage))
}

pub(super) fn authorized_binding() -> AuthorizedBinding {
    let account_id = AccountId::generate();
    let resource_id = ResourceId::generate();
    AuthorizedBinding {
        binding: VersionBindingRecord {
            id: BindingId::generate(),
            version_id: VersionId::generate(),
            name: "KV".to_owned(),
            kind: BindingKind::KvNamespace,
            resource_id,
            resource_spec_generation: 1,
            capability_version: 1,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
            descriptor_sha256: [0; 32],
            created_at_ms: 1,
        },
        resource: ResourceRecord {
            id: resource_id,
            account_id,
            kind: BindingKind::KvNamespace,
            name: "cache".to_owned(),
            state: ResourceState::Ready,
            availability: ResourceAvailability::Healthy,
            availability_code: None,
            spec_generation: 1,
            driver_schema_version: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
            deleted_at_ms: None,
        },
        account_id,
    }
}
