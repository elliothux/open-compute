use super::super::ERROR_HEADER;
use super::super::tests::{authorized_binding, storage};
use super::*;
use open_compute_core::{DurableObjectsConfig, MetricsConfig};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_workers::ResourcePins;

#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<String>>,
}

impl KvBindingExecutor for RecordingExecutor {
    fn execute(
        &self,
        _: &AuthorizedBinding,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        match command {
            KvCommand::Delete { key } => {
                self.calls.lock().unwrap().push(format!("delete:{key}"));
                Ok(KvCommandResult::Mutation)
            }
            _ => Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "unsupported test command",
            )),
        }
    }

    fn stream_get(
        &self,
        _: &AuthorizedBinding,
        _: &str,
        _: Option<u64>,
        _: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        unreachable!("this test executor only dispatches commands")
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
    assert_eq!(value.read_all_for_test().unwrap(), b"chunked-value");
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

#[tokio::test]
async fn streamed_get_rejects_invalid_part_order_and_surfaces_terminal_errors() {
    #[derive(Clone)]
    struct StreamExecutor {
        parts: Vec<KvStreamPart>,
        terminal: Option<ErrorCode>,
    }
    impl KvBindingExecutor for StreamExecutor {
        fn execute(
            &self,
            _: &AuthorizedBinding,
            _: KvCommand,
        ) -> Result<KvCommandResult, PlatformError> {
            unreachable!("stream fixture")
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

    for bytes in [b"".as_slice(), b"xx".as_slice()] {
        let response = run(StreamExecutor {
            parts: vec![
                KvStreamPart::Entry(Some(open_compute_storage::KvEntryInfo {
                    value_length: 1,
                    metadata_json: None,
                    expires_at_ms: None,
                })),
                KvStreamPart::Bytes(bytes.to_vec()),
            ],
            terminal: None,
        })
        .await;
        assert!(
            to_bytes(response.into_body(), MAX_FRAME_BODY_BYTES)
                .await
                .is_err()
        );
    }

    let response = run(StreamExecutor {
        parts: vec![
            KvStreamPart::Entry(Some(open_compute_storage::KvEntryInfo {
                value_length: 2,
                metadata_json: None,
                expires_at_ms: None,
            })),
            KvStreamPart::Bytes(vec![0, 0xff]),
        ],
        terminal: None,
    })
    .await;
    assert!(
        to_bytes(response.into_body(), MAX_FRAME_BODY_BYTES)
            .await
            .unwrap()
            .ends_with(&[0, 0xff])
    );

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
        to_bytes(response.into_body(), MAX_FRAME_BODY_BYTES)
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
        to_bytes(response.into_body(), MAX_FRAME_BODY_BYTES)
            .await
            .is_err()
    );

    struct SlowStreamExecutor(bool);
    impl KvBindingExecutor for SlowStreamExecutor {
        fn operation_timeout(&self) -> Duration {
            Duration::from_millis(1)
        }
        fn execute(
            &self,
            _: &AuthorizedBinding,
            _: KvCommand,
        ) -> Result<KvCommandResult, PlatformError> {
            unreachable!("stream fixture")
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
        to_bytes(response.into_body(), MAX_FRAME_BODY_BYTES)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn frame_dispatch_releases_pins_on_protocol_executor_and_timeout_failures() {
    enum Behavior {
        Panic,
        Slow(Arc<(Mutex<bool>, std::sync::Condvar)>),
        WrongShape,
    }
    struct ControlledExecutor(Behavior);
    impl KvBindingExecutor for ControlledExecutor {
        fn operation_timeout(&self) -> Duration {
            match &self.0 {
                Behavior::Slow(_) => Duration::from_millis(1),
                Behavior::Panic | Behavior::WrongShape => Duration::from_millis(100),
            }
        }
        fn stream_get(
            &self,
            _: &AuthorizedBinding,
            _: &str,
            _: Option<u64>,
            _: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
        ) -> Result<(), PlatformError> {
            unreachable!("command fixture")
        }
        fn execute(
            &self,
            _: &AuthorizedBinding,
            _: KvCommand,
        ) -> Result<KvCommandResult, PlatformError> {
            match &self.0 {
                Behavior::Panic => panic!("controlled executor panic"),
                Behavior::Slow(release) => {
                    let (released, timeout) = release
                        .1
                        .wait_timeout_while(
                            release.0.lock().unwrap(),
                            Duration::from_secs(10),
                            |released| !*released,
                        )
                        .unwrap();
                    assert!(
                        *released && !timeout.timed_out(),
                        "test did not release backend operation"
                    );
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
            workflow: None,
            assets: None,
            services: None,
            cache: None,
            images: None,
        };
        dispatch(
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
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let response = run(
            storage.clone(),
            &pins,
            &binding,
            Arc::new(ControlledExecutor(Behavior::Slow(release.clone()))),
            operation,
            Body::from(body),
        )
        .await;
        assert_eq!(response.headers()[ERROR_HEADER], code.as_str());
        assert_eq!(
            pins.count(binding.resource.id),
            1,
            "a timeout cannot release a running operation"
        );
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        tokio::time::timeout(Duration::from_secs(1), async {
            while pins.count(binding.resource.id) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(pins.count(binding.resource.id), 0);
}
