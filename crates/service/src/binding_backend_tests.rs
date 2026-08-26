use super::*;
use crate::metrics::MetricsRegistry;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DurableObjectsConfig, MetricsConfig, StorageConfig};
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
    for operation in [
        "get",
        "get-with-metadata",
        "get-many",
        "put",
        "delete",
        "list",
        "echo",
    ] {
        assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/{operation}")).is_some());
    }
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/get/extra")).is_none());
    assert!(parse_path("/internal/bindings/v1/kv/not-an-id/get").is_none());
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/unknown")).is_none());
    assert_eq!(Operation::Get.metric(), BindingBackendOperation::Get);
    assert_eq!(Operation::Put.metric(), BindingBackendOperation::Put);
    assert_eq!(Operation::Delete.metric(), BindingBackendOperation::Delete);
    assert_eq!(Operation::Echo.metric(), BindingBackendOperation::Echo);
    for (operation, expected) in [
        (Operation::Get, KvOperation::Get),
        (Operation::GetWithMetadata, KvOperation::GetWithMetadata),
        (Operation::GetMany, KvOperation::GetMany),
        (Operation::Put, KvOperation::Put),
        (Operation::Delete, KvOperation::Delete),
        (Operation::List, KvOperation::List),
        (Operation::Echo, KvOperation::Get),
    ] {
        assert_eq!(operation.kv_metric(), expected);
    }
}

#[tokio::test]
async fn default_executor_commands_and_stream_shape_fail_closed() {
    let binding = authorized_binding();
    let executor = RecordingExecutor::default();
    let temp = tempfile::tempdir().unwrap();
    assert!(matches!(
        executor
            .execute(
                &binding,
                KvCommand::Put {
                    key: "text".to_owned(),
                    value: b"value".to_vec(),
                    expiration: None,
                    expiration_ttl: None,
                    metadata: None,
                    metadata_present: false,
                }
            )
            .unwrap(),
        KvCommandResult::Mutation
    ));

    let invalid_dir = temp.path().join("invalid");
    std::fs::create_dir(&invalid_dir).unwrap();
    let invalid_path = invalid_dir.join("invalid-staged");
    std::fs::write(&invalid_path, [0xff]).unwrap();
    let global = Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let resource = Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let invalid = KvStagedValue::with_lease(
        invalid_path.clone(),
        std::fs::File::open(&invalid_path).unwrap(),
        1,
        KvStagingLease::new(global, resource),
    );
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::PutStaged {
                    key: "invalid".to_owned(),
                    value: invalid,
                    expiration: None,
                    expiration_ttl: None,
                    metadata: None,
                    metadata_present: false,
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::BindingCapabilityUnsupported
    );
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::Put {
                    key: "binary".to_owned(),
                    value: vec![0xff],
                    expiration: None,
                    expiration_ttl: None,
                    metadata: None,
                    metadata_present: false,
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::BindingCapabilityUnsupported
    );
    assert!(matches!(
        executor
            .execute(
                &binding,
                KvCommand::Delete {
                    key: "text".to_owned(),
                }
            )
            .unwrap(),
        KvCommandResult::Mutation
    ));
    for command in [
        KvCommand::Get {
            keys: vec!["a".to_owned(), "b".to_owned()],
            cache_ttl: None,
        },
        KvCommand::List {
            prefix: String::new(),
            limit: 1,
            cursor: None,
        },
    ] {
        assert_eq!(
            executor.execute(&binding, command).unwrap_err().code(),
            ErrorCode::BindingCapabilityUnsupported
        );
    }

    let staged_path = temp.path().join("staged");
    std::fs::write(&staged_path, b"staged").unwrap();
    let global = Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let resource = Arc::new(tokio::sync::Semaphore::new(1))
        .acquire_owned()
        .await
        .unwrap();
    let staged = KvStagedValue::with_lease(
        staged_path.clone(),
        std::fs::File::open(&staged_path).unwrap(),
        6,
        KvStagingLease::new(global, resource),
    );
    assert!(matches!(
        executor
            .execute(
                &binding,
                KvCommand::PutStaged {
                    key: "staged".to_owned(),
                    value: staged,
                    expiration: None,
                    expiration_ttl: None,
                    metadata: None,
                    metadata_present: false,
                }
            )
            .unwrap(),
        KvCommandResult::Mutation
    ));

    let mut parts = Vec::new();
    executor
        .stream_get(&binding, "text", None, &mut |part| {
            parts.push(part);
            Ok(())
        })
        .unwrap();
    assert_eq!(parts.len(), 2);

    struct ShapeExecutor(KvCommandResult);
    impl KvBindingExecutor for ShapeExecutor {
        fn get(&self, _: &AuthorizedBinding, _: &str) -> Result<Option<String>, PlatformError> {
            unreachable!()
        }
        fn put(&self, _: &AuthorizedBinding, _: &str, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn delete(&self, _: &AuthorizedBinding, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn execute(
            &self,
            _: &AuthorizedBinding,
            _: KvCommand,
        ) -> Result<KvCommandResult, PlatformError> {
            Ok(self.0.clone())
        }
    }
    for shape in [
        KvCommandResult::Mutation,
        KvCommandResult::Entries(Vec::new()),
    ] {
        assert_eq!(
            ShapeExecutor(shape)
                .stream_get(&binding, "x", None, &mut |_| Ok(()))
                .unwrap_err()
                .code(),
            ErrorCode::BindingProtocolError
        );
    }
}

#[tokio::test]
async fn stream_budget_global_and_per_resource_limits_are_hard() {
    let budget = StreamBudget::new(2, 1);
    let resource = ResourceId::generate();
    let first = budget
        .acquire(resource, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        budget
            .acquire(resource, Duration::ZERO)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::KvBusy
    );
    let other = budget
        .acquire(ResourceId::generate(), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        budget
            .acquire(ResourceId::generate(), Duration::ZERO)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::KvBusy
    );
    drop((first, other));
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
        stream_budget: StreamBudget::new(2, 1),
        r2: None,
        d1: None,
        do_config: DurableObjectsConfig::default(),
        scheduler: None,
        queue: None,
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
async fn legacy_dispatch_maps_join_and_deadline_uncertainty_without_leaking_pins() {
    #[derive(Clone, Copy)]
    enum Behavior {
        Panic,
        Slow,
    }
    struct LegacyExecutor(Behavior);
    impl KvBindingExecutor for LegacyExecutor {
        fn operation_timeout(&self) -> Duration {
            Duration::from_millis(1)
        }
        fn get(&self, _: &AuthorizedBinding, _: &str) -> Result<Option<String>, PlatformError> {
            match self.0 {
                Behavior::Panic => panic!("controlled legacy panic"),
                Behavior::Slow => {
                    std::thread::sleep(Duration::from_millis(20));
                    Ok(None)
                }
            }
        }
        fn put(&self, _: &AuthorizedBinding, _: &str, _: &str) -> Result<(), PlatformError> {
            std::thread::sleep(Duration::from_millis(20));
            Ok(())
        }
        fn delete(&self, _: &AuthorizedBinding, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
    }

    async fn run(
        executor: Arc<dyn KvBindingExecutor>,
        operation: Operation,
        body: &'static str,
        pins: &ResourcePins,
        binding: &AuthorizedBinding,
    ) -> Response {
        dispatch(
            executor,
            binding.clone(),
            operation,
            Request::new(Body::from(body)),
            pins.try_pin(binding.resource.id).unwrap(),
        )
        .await
    }

    let binding = authorized_binding();
    let pins = ResourcePins::new();
    let unsupported = run(
        Arc::new(RecordingExecutor::default()),
        Operation::List,
        "{}",
        &pins,
        &binding,
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
    let panicked = run(
        Arc::new(LegacyExecutor(Behavior::Panic)),
        Operation::Get,
        r#"{"key":"panic"}"#,
        &pins,
        &binding,
    )
    .await;
    assert_eq!(panicked.status(), StatusCode::BAD_REQUEST);
    let slow_get = run(
        Arc::new(LegacyExecutor(Behavior::Slow)),
        Operation::Get,
        r#"{"key":"slow"}"#,
        &pins,
        &binding,
    )
    .await;
    assert_eq!(slow_get.status(), StatusCode::SERVICE_UNAVAILABLE);
    let slow_put = run(
        Arc::new(LegacyExecutor(Behavior::Slow)),
        Operation::Put,
        r#"{"key":"slow","value":"value"}"#,
        &pins,
        &binding,
    )
    .await;
    assert_eq!(
        slow_put.headers()[ERROR_HEADER],
        ErrorCode::BindingResultUnknown.as_str()
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(pins.count(binding.resource.id), 0);
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

#[tokio::test]
async fn streaming_put_frame_stages_exact_bytes_and_cleans_every_terminal_path() {
    let (_temp, storage) = storage();
    let binding = authorized_binding();
    let budget = StreamBudget::new(2, 1);
    let request_id = "550e8400-e29b-41d4-a716-446655440000";
    let header = serde_json::to_vec(&serde_json::json!({
        "key": "stream",
        "metadata": {"a": 1},
        "metadataPresent": true
    }))
    .unwrap();
    let mut frame = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(&header);
    frame.extend_from_slice(b"chunked-value");
    let chunks = frame
        .into_iter()
        .map(|byte| Ok::<Bytes, std::io::Error>(Bytes::from(vec![byte])));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let command = stage_put_frame(
        &storage,
        &binding,
        request_id,
        Body::from_stream(futures::stream::iter(chunks)),
        &budget,
        Duration::from_secs(1),
        Some(&metrics),
    )
    .await
    .unwrap();
    let KvCommand::PutStaged {
        key,
        mut value,
        metadata_present,
        ..
    } = command
    else {
        unreachable!()
    };
    assert_eq!(key, "stream");
    assert!(metadata_present);
    assert_eq!(value.read_all().unwrap(), b"chunked-value");
    let active_metrics = metrics.render(&open_compute_core::PlatformStatus::starting());
    assert!(active_metrics.contains("kv_active_streams 1"));
    assert!(active_metrics.contains("kv_staging_bytes 13"));
    let resource_stage = storage
        .data_dir()
        .root()
        .join("kv/.staging-write")
        .join(binding.resource.id.to_string());
    assert!(resource_stage.join(request_id).is_file());
    drop(value);
    assert!(!resource_stage.exists());
    let idle_metrics = metrics.render(&open_compute_core::PlatformStatus::starting());
    assert!(idle_metrics.contains("kv_active_streams 0"));
    assert!(idle_metrics.contains("kv_staging_bytes 0"));

    let oversized_header = serde_json::to_vec(&serde_json::json!({"key": "oversized"})).unwrap();
    let mut oversized = u32::try_from(oversized_header.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    oversized.extend_from_slice(&oversized_header);
    oversized.resize(
        oversized.len() + open_compute_storage::KV_MAX_VALUE_BYTES + 1,
        7,
    );
    assert_eq!(
        stage_put_frame(
            &storage,
            &binding,
            request_id,
            Body::from(oversized),
            &budget,
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::KvValueTooLarge
    );
    assert!(!resource_stage.join(request_id).exists());

    let broken = Body::from_stream(futures::stream::iter(vec![Err::<Bytes, _>(
        std::io::Error::other("broken body"),
    )]));
    assert_eq!(
        stage_put_frame(
            &storage,
            &binding,
            request_id,
            broken,
            &budget,
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::KvInternalProtocolError
    );
    assert_eq!(
        stage_put_frame(
            &storage,
            &binding,
            request_id,
            Body::from(vec![0, 0, 16, 1]),
            &budget,
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::KvInternalProtocolError
    );

    assert_eq!(
        stage_put_frame(
            &storage,
            &binding,
            request_id,
            Body::empty(),
            &budget,
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::KvInternalProtocolError
    );
    let duplicate = open_compute_storage::KvPaths::open(storage.data_dir().root())
        .unwrap()
        .create_write_staging(binding.resource.id, request_id)
        .unwrap();
    let header = serde_json::to_vec(&serde_json::json!({"key": "duplicate"})).unwrap();
    let mut frame = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(&header);
    assert_eq!(
        stage_put_frame(
            &storage,
            &binding,
            request_id,
            Body::from(frame),
            &budget,
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::PathInvalid
    );
    assert!(duplicate.exists());
    std::fs::remove_file(duplicate).unwrap();
}

#[test]
fn frame_protocol_round_trips_every_shape_and_rejects_ambiguous_inputs() {
    for operation in [Operation::Get, Operation::GetWithMetadata] {
        let KvCommand::Get { keys, cache_ttl } =
            parse_frame_command(operation, br#"{"keys":["one"],"cacheTtl":60}"#).unwrap()
        else {
            panic!("single get decoded to the wrong command")
        };
        assert_eq!(keys, ["one"]);
        assert_eq!(cache_ttl, Some(60));
    }
    let KvCommand::Get { keys, .. } = parse_frame_command(
        Operation::GetMany,
        br#"{"keys":["one","two"],"cacheTtl":null}"#,
    )
    .unwrap() else {
        panic!("multi get decoded to the wrong command")
    };
    assert_eq!(keys, ["one", "two"]);
    for (operation, body) in [
        (Operation::Get, br#"{"keys":[]}"#.as_slice()),
        (Operation::GetWithMetadata, br#"{"keys":["a","b"]}"#),
    ] {
        assert_eq!(
            parse_frame_command(operation, body).unwrap_err().code(),
            ErrorCode::KvTooManyKeys
        );
    }
    let too_many = serde_json::to_vec(&serde_json::json!({
        "keys": vec!["x"; open_compute_storage::KV_MAX_MULTI_GET_KEYS + 1]
    }))
    .unwrap();
    assert_eq!(
        parse_frame_command(Operation::GetMany, &too_many)
            .unwrap_err()
            .code(),
        ErrorCode::KvTooManyKeys
    );

    let header = serde_json::to_vec(&serde_json::json!({
        "key": "put",
        "expiration": 100,
        "expirationTtl": 60,
        "metadata": {"b": 2},
        "metadataPresent": true
    }))
    .unwrap();
    let mut put = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    put.extend_from_slice(&header);
    put.extend_from_slice(b"value");
    let KvCommand::Put {
        key,
        value,
        expiration,
        expiration_ttl,
        metadata_present,
        ..
    } = parse_frame_command(Operation::Put, &put).unwrap()
    else {
        panic!("put decoded to the wrong command")
    };
    assert_eq!(key, "put");
    assert_eq!(value, b"value");
    assert_eq!(expiration, Some(100));
    assert_eq!(expiration_ttl, Some(60));
    assert!(metadata_present);
    assert_eq!(
        parse_frame_command(Operation::Put, b"bad")
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );
    assert_eq!(
        parse_frame_command(Operation::Put, &[0, 0, 16, 1])
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );
    let oversized_header = serde_json::to_vec(&serde_json::json!({"key": "large"})).unwrap();
    let mut oversized_value = u32::try_from(oversized_header.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    oversized_value.extend_from_slice(&oversized_header);
    oversized_value.resize(
        oversized_value.len() + open_compute_storage::KV_MAX_VALUE_BYTES + 1,
        0,
    );
    assert_eq!(
        parse_frame_command(Operation::Put, &oversized_value)
            .unwrap_err()
            .code(),
        ErrorCode::KvValueTooLarge
    );

    assert!(matches!(
        parse_frame_command(Operation::Delete, br#"{"key":"gone"}"#).unwrap(),
        KvCommand::Delete { key } if key == "gone"
    ));
    assert!(matches!(
        parse_frame_command(
            Operation::List,
            br#"{"prefix":"pre","limit":10,"cursor":"next"}"#
        )
        .unwrap(),
        KvCommand::List { prefix, limit: 10, cursor: Some(cursor) }
            if prefix == "pre" && cursor == "next"
    ));
    assert_eq!(
        parse_frame_command(Operation::Echo, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );

    let entry = open_compute_storage::KvEntry {
        value: b"bytes".to_vec(),
        metadata_json: Some(br#"{"a":1}"#.to_vec()),
        expires_at_ms: Some(4_000),
    };
    for operation in [Operation::Get, Operation::GetWithMetadata] {
        let (content_type, bytes) = encode_frame_result(
            operation,
            KvCommandResult::Entries(vec![Some(entry.clone())]),
        )
        .unwrap();
        assert_eq!(content_type, FRAME_CONTENT_TYPE);
        assert!(bytes.starts_with(b"KVS1\x01"));
        assert!(bytes.ends_with(b"bytes"));
    }
    let (_, missing) =
        encode_frame_result(Operation::Get, KvCommandResult::Entries(vec![None])).unwrap();
    assert!(missing.starts_with(b"KVS1\x00"));
    let (_, without_metadata) = encode_frame_result(
        Operation::Get,
        KvCommandResult::Entries(vec![Some(open_compute_storage::KvEntry {
            value: b"plain".to_vec(),
            metadata_json: None,
            expires_at_ms: None,
        })]),
    )
    .unwrap();
    assert!(without_metadata.ends_with(b"plain"));
    let (_, many) = encode_frame_result(
        Operation::GetMany,
        KvCommandResult::Entries(vec![Some(entry), None]),
    )
    .unwrap();
    assert!(many.starts_with(b"KVB1\x00\x02"));
    for operation in [Operation::Put, Operation::Delete] {
        assert!(
            encode_frame_result(operation, KvCommandResult::Mutation)
                .unwrap()
                .1
                .is_empty()
        );
    }
    let (_, listed) = encode_frame_result(
        Operation::List,
        KvCommandResult::List {
            rows: vec![open_compute_storage::KvListRow {
                key: b"listed".to_vec(),
                metadata_json: Some(br#"{"x":true}"#.to_vec()),
                expires_at_ms: Some(9_000),
            }],
            complete: false,
            cursor: Some("cursor".to_owned()),
        },
    )
    .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&listed).unwrap();
    assert_eq!(listed["keys"][0]["name"], "listed");
    assert_eq!(listed["keys"][0]["expiration"], 9);
    assert_eq!(listed["cursor"], "cursor");
    for row in [
        open_compute_storage::KvListRow {
            key: vec![0xff],
            metadata_json: None,
            expires_at_ms: None,
        },
        open_compute_storage::KvListRow {
            key: b"valid".to_vec(),
            metadata_json: Some(b"not-json".to_vec()),
            expires_at_ms: None,
        },
    ] {
        assert_eq!(
            encode_frame_result(
                Operation::List,
                KvCommandResult::List {
                    rows: vec![row],
                    complete: true,
                    cursor: None,
                },
            )
            .unwrap_err()
            .code(),
            ErrorCode::KvCorrupt
        );
    }
    assert_eq!(
        encode_frame_result(Operation::List, KvCommandResult::Mutation)
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );

    assert_eq!(encode_stream_header(None).unwrap().len(), 21);
    let stream_header = encode_stream_header(Some(open_compute_storage::KvEntryInfo {
        value_length: 5,
        metadata_json: Some(b"null".to_vec()),
        expires_at_ms: None,
    }))
    .unwrap();
    assert!(stream_header.starts_with(b"KVS1\x01"));
    assert!(stream_header.ends_with(&5_u32.to_be_bytes()));
}

#[tokio::test]
async fn streamed_get_rejects_invalid_part_order_and_surfaces_terminal_errors() {
    #[derive(Clone)]
    struct StreamExecutor {
        parts: Vec<KvStreamPart>,
        terminal: Option<ErrorCode>,
    }
    impl KvBindingExecutor for StreamExecutor {
        fn get(&self, _: &AuthorizedBinding, _: &str) -> Result<Option<String>, PlatformError> {
            unreachable!()
        }
        fn put(&self, _: &AuthorizedBinding, _: &str, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn delete(&self, _: &AuthorizedBinding, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn stream_get(
            &self,
            _: &AuthorizedBinding,
            _: &str,
            _: Option<u64>,
            sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
        ) -> Result<(), PlatformError> {
            for part in self.parts.clone() {
                sink(part)?;
            }
            match self.terminal {
                Some(code) => Err(PlatformError::new(code, "terminal")),
                None => Ok(()),
            }
        }
    }

    async fn run(executor: StreamExecutor) -> Response {
        let binding = authorized_binding();
        let pin = ResourcePins::new().try_pin(binding.resource.id).unwrap();
        dispatch_stream_get(Arc::new(executor), binding, "key".to_owned(), None, pin).await
    }

    let response = run(StreamExecutor {
        parts: Vec::new(),
        terminal: Some(ErrorCode::KvUnavailable),
    })
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response = run(StreamExecutor {
        parts: vec![KvStreamPart::Bytes(b"early".to_vec())],
        terminal: None,
    })
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = run(StreamExecutor {
        parts: Vec::new(),
        terminal: None,
    })
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = run(StreamExecutor {
        parts: vec![
            KvStreamPart::Entry(Some(open_compute_storage::KvEntryInfo {
                value_length: 1,
                metadata_json: None,
                expires_at_ms: None,
            })),
            KvStreamPart::Bytes(b"x".to_vec()),
            KvStreamPart::Entry(None),
        ],
        terminal: None,
    })
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), MAX_BODY_BYTES)
            .await
            .is_err()
    );

    let response = run(StreamExecutor {
        parts: vec![KvStreamPart::Entry(None)],
        terminal: Some(ErrorCode::KvCorrupt),
    })
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), MAX_BODY_BYTES)
            .await
            .is_err()
    );

    struct SlowStreamExecutor(bool);
    impl KvBindingExecutor for SlowStreamExecutor {
        fn operation_timeout(&self) -> Duration {
            Duration::from_millis(1)
        }
        fn get(&self, _: &AuthorizedBinding, _: &str) -> Result<Option<String>, PlatformError> {
            unreachable!()
        }
        fn put(&self, _: &AuthorizedBinding, _: &str, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn delete(&self, _: &AuthorizedBinding, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn stream_get(
            &self,
            _: &AuthorizedBinding,
            _: &str,
            _: Option<u64>,
            sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
        ) -> Result<(), PlatformError> {
            if self.0 {
                sink(KvStreamPart::Entry(None))?;
            }
            std::thread::sleep(Duration::from_millis(20));
            Ok(())
        }
    }
    async fn run_slow(executor: SlowStreamExecutor) -> Response {
        let binding = authorized_binding();
        let pin = ResourcePins::new().try_pin(binding.resource.id).unwrap();
        dispatch_stream_get(Arc::new(executor), binding, "key".to_owned(), None, pin).await
    }
    let response = run_slow(SlowStreamExecutor(false)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response = run_slow(SlowStreamExecutor(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), MAX_BODY_BYTES)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn frame_dispatch_releases_pins_on_protocol_executor_and_timeout_failures() {
    #[derive(Clone, Copy)]
    enum Behavior {
        Panic,
        Slow,
        WrongShape,
    }
    struct ControlledExecutor(Behavior);
    impl KvBindingExecutor for ControlledExecutor {
        fn operation_timeout(&self) -> Duration {
            Duration::from_millis(1)
        }
        fn get(&self, _: &AuthorizedBinding, _: &str) -> Result<Option<String>, PlatformError> {
            unreachable!()
        }
        fn put(&self, _: &AuthorizedBinding, _: &str, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn delete(&self, _: &AuthorizedBinding, _: &str) -> Result<(), PlatformError> {
            unreachable!()
        }
        fn execute(
            &self,
            _: &AuthorizedBinding,
            _: KvCommand,
        ) -> Result<KvCommandResult, PlatformError> {
            match self.0 {
                Behavior::Panic => panic!("controlled executor panic"),
                Behavior::Slow => {
                    std::thread::sleep(Duration::from_millis(20));
                    Ok(KvCommandResult::Mutation)
                }
                Behavior::WrongShape => Ok(KvCommandResult::Entries(Vec::new())),
            }
        }
    }

    async fn run(
        storage: Arc<PlatformStorage>,
        pins: &ResourcePins,
        binding: &AuthorizedBinding,
        executor: Arc<dyn KvBindingExecutor>,
        operation: Operation,
        body: Body,
    ) -> Response {
        let state = BackendState {
            storage,
            auth: GenerationAuthRegistry::new(),
            pins: pins.clone(),
            executor,
            metrics: None,
            stream_budget: StreamBudget::new(2, 1),
            r2: None,
            d1: None,
            do_config: DurableObjectsConfig::default(),
            scheduler: None,
            queue: None,
        };
        dispatch_frame(
            state,
            binding.clone(),
            operation,
            "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            Request::new(body),
            pins.try_pin(binding.resource.id).unwrap(),
        )
        .await
    }

    let (_temp, storage) = storage();
    let binding = authorized_binding();
    let pins = ResourcePins::new();
    let recording: Arc<dyn KvBindingExecutor> = Arc::new(RecordingExecutor::default());
    let response = run(
        storage.clone(),
        &pins,
        &binding,
        recording.clone(),
        Operation::Echo,
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = run(
        storage.clone(),
        &pins,
        &binding,
        recording.clone(),
        Operation::List,
        Body::from("not-json"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = run(
        storage.clone(),
        &pins,
        &binding,
        recording.clone(),
        Operation::Delete,
        Body::from(br#"{"key":"gone"}"#.as_slice()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = run(
        storage.clone(),
        &pins,
        &binding,
        recording,
        Operation::List,
        Body::from(br#"{"prefix":"","limit":1,"cursor":null}"#.as_slice()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = run(
        storage.clone(),
        &pins,
        &binding,
        Arc::new(ControlledExecutor(Behavior::WrongShape)),
        Operation::Delete,
        Body::from(br#"{"key":"wrong"}"#.as_slice()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = run(
        storage.clone(),
        &pins,
        &binding,
        Arc::new(ControlledExecutor(Behavior::Panic)),
        Operation::Delete,
        Body::from(br#"{"key":"panic"}"#.as_slice()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    for (operation, body, code) in [
        (
            Operation::Delete,
            br#"{"key":"slow"}"#.as_slice(),
            ErrorCode::KvResultUnknown,
        ),
        (
            Operation::List,
            br#"{"prefix":"","limit":1,"cursor":null}"#.as_slice(),
            ErrorCode::KvUnavailable,
        ),
    ] {
        let response = run(
            storage.clone(),
            &pins,
            &binding,
            Arc::new(ControlledExecutor(Behavior::Slow)),
            operation,
            Body::from(body),
        )
        .await;
        assert_eq!(response.headers()[ERROR_HEADER], code.as_str());
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(pins.count(binding.resource.id), 0);
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
