use super::kv::MAX_FRAME_BODY_BYTES;
use super::*;
use crate::metrics::MetricsRegistry;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DurableObjectsConfig, MetricsConfig, QueuesConfig, StorageConfig};
use open_compute_core::{
    AccountId, CanonicalBindingConfig, CanonicalPermissions, RequestId, ResourceAvailability,
    ResourceId, ResourceState, SecretString,
};
use open_compute_storage::{DeploymentBindingRecord, ResourceRecord};

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
    let deployment = DeploymentId::generate();
    let request = RequestId::generate();
    let mut headers = HeaderMap::new();
    headers.insert(DEPLOYMENT_HEADER, deployment.to_string().parse().unwrap());
    headers.insert(REQUEST_HEADER, request.to_string().parse().unwrap());
    headers.insert(DESCRIPTOR_HEADER, "ab".repeat(32).parse().unwrap());
    headers.insert(header::CONTENT_TYPE, FRAME_CONTENT_TYPE.parse().unwrap());
    assert_eq!(
        parse_header::<DeploymentId>(&headers, DEPLOYMENT_HEADER).unwrap(),
        deployment
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
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (
            ErrorCode::BindingLimitExceeded,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (ErrorCode::ResourceNotReady, StatusCode::CONFLICT),
        (ErrorCode::ResourceReferenced, StatusCode::CONFLICT),
        (
            ErrorCode::ResourceUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::KvResultUnknown, StatusCode::SERVICE_UNAVAILABLE),
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

    let missing_deployment = handle(
        State(state.clone()),
        base_request(Method::POST, &path, &token, "generation")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(missing_deployment.status(), StatusCode::BAD_REQUEST);

    let deployment = DeploymentId::generate();
    let invalid_request_id = handle(
        State(state.clone()),
        base_request(Method::POST, &path, &token, "generation")
            .header(DEPLOYMENT_HEADER, deployment.to_string())
            .header(REQUEST_HEADER, "bad")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid_request_id.status(), StatusCode::BAD_REQUEST);

    let invalid_digest = handle(
        State(state.clone()),
        authorized_request(&path, &token, deployment)
            .header(DESCRIPTOR_HEADER, "bad")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid_digest.status(), StatusCode::BAD_REQUEST);

    let wrong_content_type = handle(
        State(state.clone()),
        authorized_request(&path, &token, deployment)
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

    let missing_binding = handle(
        State(state),
        authorized_request(&path, &token, deployment)
            .header(DESCRIPTOR_HEADER, "ab".repeat(32))
            .header(header::CONTENT_TYPE, FRAME_CONTENT_TYPE)
            .body(Body::from(r#"{"key":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(missing_binding.status(), StatusCode::NOT_FOUND);
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

fn authorized_request(
    path: &str,
    token: &str,
    deployment: DeploymentId,
) -> axum::http::request::Builder {
    base_request(Method::POST, path, token, "generation")
        .header(DEPLOYMENT_HEADER, deployment.to_string())
        .header(REQUEST_HEADER, "550e8400-e29b-41d4-a716-446655440000")
}

pub(super) fn storage() -> (tempfile::TempDir, Arc<PlatformStorage>) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
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
    .unwrap();
    (temp, Arc::new(storage))
}

pub(super) fn authorized_binding() -> AuthorizedBinding {
    let account_id = AccountId::generate();
    let resource_id = ResourceId::generate();
    AuthorizedBinding {
        binding: DeploymentBindingRecord {
            id: BindingId::generate(),
            deployment_id: DeploymentId::generate(),
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
