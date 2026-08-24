use super::*;
use crate::metrics::MetricsRegistry;
use futures::StreamExt as _;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{
    AccountId, CanonicalBindingConfig, CanonicalPermissions, RequestId, ResourceAvailability,
    ResourceId, ResourceState, SecretString,
};
use open_compute_storage::{DeploymentBindingRecord, ResourceRecord};
use std::sync::Mutex;

#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<String>>,
}

impl KvBindingExecutor for RecordingExecutor {
    fn get(
        &self,
        _binding: &AuthorizedBinding,
        key: &str,
    ) -> Result<Option<String>, PlatformError> {
        self.calls.lock().unwrap().push(format!("get:{key}"));
        Ok(Some("value".to_owned()))
    }

    fn put(
        &self,
        _binding: &AuthorizedBinding,
        key: &str,
        value: &str,
    ) -> Result<(), PlatformError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("put:{key}:{value}"));
        Ok(())
    }

    fn delete(&self, _binding: &AuthorizedBinding, key: &str) -> Result<(), PlatformError> {
        self.calls.lock().unwrap().push(format!("delete:{key}"));
        Ok(())
    }
}

#[test]
fn path_parser_is_exact_and_typed() {
    let id = BindingId::generate();
    for operation in ["get", "put", "delete", "echo"] {
        assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/{operation}")).is_some());
    }
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/get/extra")).is_none());
    assert!(parse_path("/internal/bindings/v1/kv/not-an-id/get").is_none());
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/unknown")).is_none());
    assert_eq!(Operation::Get.metric(), BindingBackendOperation::Get);
    assert_eq!(Operation::Put.metric(), BindingBackendOperation::Put);
    assert_eq!(Operation::Delete.metric(), BindingBackendOperation::Delete);
    assert_eq!(Operation::Echo.metric(), BindingBackendOperation::Echo);
}

#[test]
fn key_budget_is_byte_bounded() {
    assert_eq!(
        validate_key("").unwrap_err().code(),
        ErrorCode::BindingLimitExceeded
    );
    assert!(validate_key(&"a".repeat(MAX_KEY_BYTES)).is_ok());
    assert_eq!(
        validate_key(&"界".repeat(MAX_KEY_BYTES))
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
}

#[test]
fn unavailable_executor_fails_closed() {
    assert_eq!(unavailable().code(), ErrorCode::ResourceUnavailable);
    let binding = authorized_binding();
    let executor = UnavailableKvBindingExecutor;
    assert_eq!(
        executor.get(&binding, "key").unwrap_err().code(),
        ErrorCode::ResourceUnavailable
    );
    assert_eq!(
        executor.put(&binding, "key", "value").unwrap_err().code(),
        ErrorCode::ResourceUnavailable
    );
    assert_eq!(
        executor.delete(&binding, "key").unwrap_err().code(),
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
    headers.insert(header::CONTENT_TYPE, JSON_CONTENT_TYPE.parse().unwrap());
    assert_eq!(
        parse_header::<DeploymentId>(&headers, DEPLOYMENT_HEADER).unwrap(),
        deployment
    );
    assert_eq!(parse_digest(&headers).unwrap(), [0xab; 32]);
    assert!(valid_request_id(&headers));
    assert!(content_type_matches(&headers, Operation::Get));
    headers.insert(header::CONTENT_TYPE, STREAM_CONTENT_TYPE.parse().unwrap());
    assert!(content_type_matches(&headers, Operation::Echo));
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
        parse_json::<KeyRequest>(br#"{"key":"x","extra":true}"#)
            .err()
            .unwrap()
            .code(),
        ErrorCode::BindingProtocolError
    );
    assert!(serialize_get_response(Some("ok".to_owned())).is_ok());
    assert_eq!(
        serialize_get_response(Some("x".repeat(MAX_BODY_BYTES)))
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
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
        (
            ErrorCode::BindingResultUnknown,
            StatusCode::SERVICE_UNAVAILABLE,
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
        (ErrorCode::BindingResultUnknown, true, true),
    ] {
        let response = platform_error(&PlatformError::new(code, "must not escape"));
        let body = to_bytes(response.into_body(), MAX_BODY_BYTES)
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
            .header(header::CONTENT_LENGTH, MAX_BODY_BYTES + 1)
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
            .header(header::CONTENT_TYPE, JSON_CONTENT_TYPE)
            .body(Body::from(r#"{"key":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(missing_binding.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pinned_stream_enforces_limit_propagates_errors_and_releases_pin() {
    let resource_id = ResourceId::generate();
    let pins = ResourcePins::new();
    let pin = pins.try_pin(resource_id).unwrap();
    let mut limited = PinnedLimitedStream::new(Body::from("abc"), pin, 2);
    assert!(limited.next().await.unwrap().is_err());
    assert!(limited.next().await.is_none());
    assert_eq!(pins.count(resource_id), 1);
    drop(limited);
    assert_eq!(pins.count(resource_id), 0);

    let pin = pins.try_pin(resource_id).unwrap();
    let body = Body::from_stream(futures::stream::once(async {
        Err::<Bytes, _>(std::io::Error::other("stream failed"))
    }));
    let mut failed = PinnedLimitedStream::new(body, pin, 10);
    assert!(failed.next().await.unwrap().is_err());
    drop(failed);
    assert_eq!(pins.count(resource_id), 0);
}

#[tokio::test]
async fn dispatch_covers_typed_operations_payload_failures_and_pin_release() {
    let executor = Arc::new(RecordingExecutor::default());
    let binding = authorized_binding();
    let pins = ResourcePins::new();
    let resource_id = binding.resource.id;

    let response = dispatch(
        executor.clone(),
        binding.clone(),
        Operation::Get,
        Request::new(Body::from(r#"{"key":"one"}"#)),
        pins.try_pin(resource_id).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        to_bytes(response.into_body(), MAX_BODY_BYTES)
            .await
            .unwrap(),
        Bytes::from_static(br#"{"value":"value"}"#)
    );

    let response = dispatch(
        executor.clone(),
        binding.clone(),
        Operation::Put,
        Request::new(Body::from(r#"{"key":"two","value":"written"}"#)),
        pins.try_pin(resource_id).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = dispatch(
        executor.clone(),
        binding.clone(),
        Operation::Delete,
        Request::new(Body::from(r#"{"key":"three"}"#)),
        pins.try_pin(resource_id).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = dispatch(
        executor.clone(),
        binding.clone(),
        Operation::Echo,
        Request::new(Body::from("stream")),
        pins.try_pin(resource_id).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        STREAM_CONTENT_TYPE
    );
    assert_eq!(
        to_bytes(response.into_body(), MAX_BODY_BYTES)
            .await
            .unwrap(),
        Bytes::from_static(b"stream")
    );

    for body in [r#"{"key":""}"#, r#"{"unknown":true}"#] {
        let response = dispatch(
            executor.clone(),
            binding.clone(),
            Operation::Get,
            Request::new(Body::from(body)),
            pins.try_pin(resource_id).unwrap(),
        )
        .await;
        assert!(response.status().is_client_error());
    }
    let response = dispatch(
        executor.clone(),
        binding.clone(),
        Operation::Get,
        Request::new(Body::from(vec![b'x'; MAX_BODY_BYTES + 1])),
        pins.try_pin(resource_id).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    assert_eq!(
        executor.calls.lock().unwrap().as_slice(),
        ["get:one", "put:two:written", "delete:three"]
    );
    assert_eq!(pins.count(resource_id), 0);
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
        async {},
    )
    .await
    .unwrap();
}

#[test]
fn permission_matrix_is_operation_specific() {
    let mut binding = authorized_binding();
    binding.binding.permissions = CanonicalPermissions {
        read: true,
        write: false,
    };
    assert!(permission_allows(&binding, Operation::Get));
    assert!(permission_allows(&binding, Operation::Echo));
    assert!(!permission_allows(&binding, Operation::Put));
    assert!(!permission_allows(&binding, Operation::Delete));
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

fn storage() -> (tempfile::TempDir, Arc<PlatformStorage>) {
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

fn authorized_binding() -> AuthorizedBinding {
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
