use super::*;

pub(super) async fn run() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let scheduler = Arc::new(
        SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 100, 1).unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let mut binding_shutdown_rx = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                storage.clone(),
                auth,
                ResourcePins::new(),
                Arc::new(SqliteKvBindingExecutor::new(storage, Arc::new(SystemClock))),
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                async move {
                    let _ = binding_shutdown_rx.changed().await;
                },
            )
            .await
        }
    });

    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.2-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(auth.clone(), supervisor_slot.clone());
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            runtime.version_output(),
        )
        .unwrap();
    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-2-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;
    let first_pid = supervisor.snapshot().pid.unwrap();
    let first_credential = auth.credential().unwrap();

    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(account, "runtime-gate", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(open_compute_service::product_promotion_for_test(
        storage.clone(),
        scheduler.clone(),
    ));

    let a = deploy(
        &controller,
        account,
        worker.id,
        "deploy-a",
        "A",
        true,
        false,
    )
    .await;
    assert_eq!(
        supervisor.snapshot().state,
        SupervisorState::Running,
        "runtime left Running after version validation: {:?}",
        supervisor.last_diagnostics()
    );
    assert!(
        !source_task.is_finished(),
        "runtime-source server stopped during version validation"
    );
    assert!(
        !binding_task.is_finished(),
        "binding-backend server stopped during version validation"
    );
    let response = dispatch(&transport, account, worker.id, &a, None, "hello-a").await;
    assert_eq!(
        response.status, 200,
        "unexpected dispatch response: {response:?}"
    );
    assert_eq!(response.loader_outcome, Some(LoaderOutcome::Cold));
    assert!(response.body.contains("A:hello-a:production:gate-secret"));
    assert!(response.body.ends_with(":API_TOKEN,MODE"));

    // Warm path is still descriptor-resolved and produces the same immutable code.
    let warm = dispatch(&transport, account, worker.id, &a, None, "warm").await;
    assert_eq!(warm.status, 200);
    assert_eq!(warm.loader_outcome, Some(LoaderOutcome::Warm));
    assert!(warm.body.contains("A:warm:production:gate-secret"));

    // Native Queue and scheduled custom events traverse the same immutable dynamic loader.
    let queue_ids = [
        QueueMessageId::generate(),
        QueueMessageId::generate(),
        QueueMessageId::generate(),
    ];
    let queue_result = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, None),
            &QueueDispatchRequest {
                queue_name: "runtime-gate".to_owned(),
                messages: vec![
                    QueueDispatchMessage {
                        id: queue_ids[0].to_string(),
                        timestamp_ms: 1_787_700_000_000,
                        attempts: 1,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("ack"),
                    },
                    QueueDispatchMessage {
                        id: queue_ids[1].to_string(),
                        timestamp_ms: 1_787_700_000_001,
                        attempts: 2,
                        content_type: QueueContentType::Json,
                        body_base64: base64::engine::general_purpose::STANDARD
                            .encode(br#"{"action":"retry"}"#),
                    },
                    QueueDispatchMessage {
                        id: queue_ids[2].to_string(),
                        timestamp_ms: 1_787_700_000_002,
                        attempts: 3,
                        content_type: QueueContentType::Bytes,
                        body_base64: base64::engine::general_purpose::STANDARD.encode([0, 255, 7]),
                    },
                ],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await;
    let queue_result = match queue_result {
        Ok(result) => result,
        Err(error) => {
            supervisor.shutdown().await;
            panic!(
                "native Queue custom event: {error:?}; diagnostics: {:?}",
                supervisor.last_diagnostics()
            );
        }
    };
    assert_eq!(queue_result.outcome, "ok");
    assert_eq!(queue_result.explicit_acks, [queue_ids[0].to_string()]);
    assert_eq!(queue_result.retry_messages.len(), 1);
    assert_eq!(
        queue_result.retry_messages[0].msg_id,
        queue_ids[1].to_string()
    );
    assert_eq!(queue_result.retry_messages[0].delay_seconds, Some(7));
    assert!(!queue_result.ack_all);
    assert!(!queue_result.retry_batch.retry);

    let scheduled = transport
        .dispatch_scheduled(
            &dispatch_target(account, worker.id, &a, None),
            &ScheduledDispatchRequest {
                scheduled_time_ms: 1_787_700_060_000,
                cron: "*/5 * * * *".to_owned(),
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("native scheduled custom event");
    assert_eq!(scheduled.outcome, "ok");
    assert!(scheduled.no_retry);

    for (queue_name, expected) in [
        ("runtime-gate-throw", "exception"),
        ("runtime-gate-wait-until", "exception"),
    ] {
        let result = transport
            .dispatch_queue(
                &dispatch_target(account, worker.id, &a, None),
                &QueueDispatchRequest {
                    queue_name: queue_name.to_owned(),
                    messages: vec![QueueDispatchMessage {
                        id: QueueMessageId::generate().to_string(),
                        timestamp_ms: 1_787_700_000_000,
                        attempts: 1,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("failure"),
                    }],
                    metadata: Default::default(),
                },
                Duration::from_secs(5),
            )
            .await
            .expect("known Queue failure result");
        assert_eq!(result.outcome, expected);
    }
    for cron in ["1 * * * *", "2 * * * *"] {
        let result = transport
            .dispatch_scheduled(
                &dispatch_target(account, worker.id, &a, None),
                &ScheduledDispatchRequest {
                    scheduled_time_ms: 1_787_700_060_000,
                    cron: cron.to_owned(),
                    scheduled_handler: true,
                    workflow_bindings: Vec::new(),
                },
                Duration::from_secs(5),
            )
            .await
            .expect("known scheduled failure result");
        assert_eq!(result.outcome, "exception");
        assert!(!result.no_retry);
    }

    let named_queue = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, Some("Named")),
            &QueueDispatchRequest {
                queue_name: "runtime-gate".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_787_700_000_003,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("named"),
                }],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("named Queue custom event");
    assert_eq!(named_queue.outcome, "ok");
    assert!(named_queue.ack_all);

    let named = dispatch(&transport, account, worker.id, &a, Some("Named"), "named").await;
    assert_eq!(named.status, 200, "unexpected named response: {named:?}");
    assert_eq!(named.body, "named:A:named");
    let missing = dispatch(
        &transport,
        account,
        worker.id,
        &a,
        Some("Missing"),
        "missing",
    )
    .await;
    assert_eq!(missing.status, 404);
    assert!(missing.body.contains("ENTRYPOINT_NOT_FOUND"));

    let conformance = dispatch(&transport, account, worker.id, &a, None, "conformance").await;
    assert_eq!(conformance.status, 200);
    let conformance: serde_json::Value = serde_json::from_str(&conformance.body).unwrap();
    for api in [
        "fetch",
        "request",
        "response",
        "headers",
        "url",
        "streams",
        "crypto",
        "timers",
        "webSocket",
    ] {
        assert_eq!(conformance[api], true, "conformance API {api}");
    }

    // The platform proxy keeps both directions streaming. The echo path does
    // not materialize the request in ocd, and an early tenant response
    // cancels a request producer that has not reached EOF.
    let stream_payload = vec![b's'; 4 * 1024 * 1024];
    let stream = futures::stream::iter(
        stream_payload
            .chunks(32 * 1024)
            .map(|chunk| Ok::<Bytes, Infallible>(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>(),
    );
    let stream_response = transport
        .dispatch(
            dispatch_target(account, worker.id, &a, None),
            Request::builder()
                .method("POST")
                .uri("/runtime-gate/stream")
                .header(header::HOST, "workers.example.test")
                .body(Body::from_stream(stream))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream_response.status(), 200);
    assert_eq!(
        to_bytes(stream_response.into_body(), 5 * 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        stream_payload
    );

    let producer_dropped = Arc::new(AtomicBool::new(false));
    let early_response = tokio::time::timeout(
        Duration::from_secs(3),
        transport.dispatch(
            dispatch_target(account, worker.id, &a, None),
            Request::builder()
                .method("POST")
                .uri("/runtime-gate/early")
                .header(header::HOST, "workers.example.test")
                .body(Body::from_stream(PendingUpload {
                    first: Some(Bytes::from_static(b"first-chunk")),
                    dropped: producer_dropped.clone(),
                }))
                .unwrap(),
        ),
    )
    .await
    .expect("tenant must be able to respond before request EOF")
    .unwrap();
    assert_eq!(
        to_bytes(early_response.into_body(), 1024).await.unwrap(),
        "early-response"
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while !producer_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("early response must stop polling and drop the upload stream");

    let b = deploy(
        &controller,
        account,
        worker.id,
        "deploy-b",
        "B",
        true,
        false,
    )
    .await;
    assert_ne!(a.id, b.id);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(b.id)
    );

    let egress_fixture = egress_fixture_from_env();
    if let Some(fixture) = egress_fixture.as_ref() {
        run_tls_fixture(&workerd, &root, fixture).await;
    }
    let egress = deploy_egress(&controller, account, worker.id, egress_fixture.as_ref()).await;
    let denied = dispatch(&transport, account, worker.id, &egress, None, "").await;
    assert_eq!(
        denied.status,
        200,
        "egress response: {denied:?}; diagnostics: {:?}",
        supervisor.last_diagnostics()
    );
    let egress_result: serde_json::Value = serde_json::from_str(&denied.body).unwrap();
    let expected_denied = if egress_fixture.is_some() { 11 } else { 9 };
    assert_eq!(egress_result["denied"], expected_denied);
    let allowed = egress_result["allowed"].as_array().unwrap();
    assert_eq!(allowed.len(), egress_fixture.as_ref().map_or(0, |_| 3));
    assert!(allowed.iter().all(|value| value == "fixture-ok"));
    assert_eq!(
        egress_result["ctxExports"]["ok"], true,
        "ctx.exports connect: {}",
        egress_result["ctxExports"]
    );
    assert_eq!(egress_result["ctxExports"]["bytes"], 96 * 1024);
    assert_eq!(
        egress_result["ctxExports"]["localAddress"],
        "loopback.invalid:7000"
    );
    assert_eq!(
        egress_result["ctxExports"]["remoteAddress"],
        serde_json::Value::Null
    );
    assert!(
        egress_result["ctxExports"]["chunks"]
            .as_u64()
            .is_some_and(|chunks| chunks > 1),
        "ctx.exports socket echo must cross stream chunks: {}",
        egress_result["ctxExports"]
    );
    if let Some(egress_fixture) = &egress_fixture {
        assert_raw_tcp_fixture(&egress_result["rawTcp"], egress_fixture);
        let event_message = QueueMessageId::generate();
        let event_queue = transport
            .dispatch_queue(
                &dispatch_target(account, worker.id, &egress, None),
                &QueueDispatchRequest {
                    queue_name: "raw-tcp-event-source".to_owned(),
                    messages: vec![QueueDispatchMessage {
                        id: event_message.to_string(),
                        timestamp_ms: 1_787_700_000_010,
                        attempts: 1,
                        content_type: QueueContentType::Text,
                        body_base64: base64::engine::general_purpose::STANDARD.encode("socket"),
                    }],
                    metadata: Default::default(),
                },
                Duration::from_secs(10),
            )
            .await
            .expect("Queue raw TCP event source");
        assert_eq!(event_queue.outcome, "ok");
        assert!(event_queue.ack_all);
        let event_scheduled = transport
            .dispatch_scheduled(
                &dispatch_target(account, worker.id, &egress, None),
                &ScheduledDispatchRequest {
                    scheduled_time_ms: 1_787_700_060_000,
                    cron: "3 * * * *".to_owned(),
                    scheduled_handler: true,
                    workflow_bindings: Vec::new(),
                },
                Duration::from_secs(10),
            )
            .await
            .expect("scheduled raw TCP event source");
        assert_eq!(event_scheduled.outcome, "ok");
        assert!(event_scheduled.no_retry);
    } else {
        assert_eq!(egress_result["rawTcp"], serde_json::Value::Null);
    }
    let node = deploy_node(&controller, account, worker.id).await;
    let node_response = dispatch(&transport, account, worker.id, &node, None, "").await;
    assert_eq!(node_response.status, 200);
    assert_eq!(node_response.body, "node-compat");
    assert!(
        dispatch(&transport, account, worker.id, &b, None, "active-b")
            .await
            .body
            .contains("B:active-b")
    );
    repo.promote(
        account,
        worker.id,
        a.id,
        Some(b.id),
        RequestId::generate(),
        10,
    )
    .unwrap();
    assert!(
        dispatch(&transport, account, worker.id, &a, None, "rollback-a")
            .await
            .body
            .contains("A:rollback-a")
    );

    // Deterministic parse/startup failure is rejected and cannot move active.
    let active_before = repo
        .get_worker(account, worker.id)
        .unwrap()
        .active_version_id;
    let invalid = create_request(account, worker.id, "deploy-invalid", "C", false, true);
    let error = controller.create_version(invalid).await.unwrap_err();
    assert_eq!(error.code(), ErrorCode::BundleRuntimeInvalid);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        active_before
    );
    assert_eq!(
        repo.list_versions(account, worker.id).unwrap()[0].state,
        VersionState::Rejected
    );

    // Restart rotates credentials and forces a new workerd process/cold cache.
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, first_pid, Duration::from_secs(30)).await;
    assert_ne!(
        auth.credential().unwrap().expose(),
        first_credential.expose(),
        "generation token must rotate"
    );
    let concurrent = futures::future::join_all((0..100).map(|index| {
        let transport = transport.clone();
        let version = a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &version,
                None,
                &format!("restart-{index}"),
            )
            .await
        }
    }))
    .await;
    assert!(concurrent.iter().all(|response| response.status == 200));
    assert_eq!(
        concurrent
            .iter()
            .filter(|response| response.loader_outcome == Some(LoaderOutcome::Cold))
            .count(),
        1,
        "100 concurrent cold requests must invoke exactly one native loader callback"
    );

    // The stable HTTP surface drives the same real validation and dispatch path.
    http::api_matrix(
        storage.clone(),
        artifacts.clone(),
        transport.clone(),
        account,
        scheduler,
    )
    .await;

    // Once response headers/body have started, a runtime crash truncates the
    // stream. ocd must not rewrite or replay it as a clean JSON error.
    let crash_pid = supervisor.snapshot().pid.unwrap();
    let timeout = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, None),
            &QueueDispatchRequest {
                queue_name: "runtime-gate-timeout".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_787_700_000_000,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("timeout"),
                }],
                metadata: Default::default(),
            },
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
    assert_eq!(timeout.code(), ErrorCode::QueueSendResultUnknown);
    let crash_response = transport
        .dispatch(
            dispatch_target(account, worker.id, &a, None),
            Request::builder()
                .method("GET")
                .uri("/runtime-gate/midstream")
                .header(header::HOST, "workers.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(crash_response.status(), 200);
    let mut crash_body = crash_response.into_body();
    let first = tokio::time::timeout(Duration::from_secs(3), crash_body.frame())
        .await
        .expect("midstream prefix deadline")
        .expect("midstream prefix frame")
        .expect("midstream prefix transport");
    assert_eq!(first.into_data().expect("data frame"), "stream-prefix");
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, crash_pid, Duration::from_secs(30)).await;
    let tail = tokio::time::timeout(Duration::from_secs(3), crash_body.frame())
        .await
        .expect("crashed response stream must terminate");
    assert!(
        tail.is_none() || tail.is_some_and(|frame| frame.is_err()),
        "a started response must truncate, not become a platform error body"
    );
    assert_eq!(
        dispatch(&transport, account, worker.id, &a, None, "post-crash")
            .await
            .status,
        200
    );
    let restarted_queue = transport
        .dispatch_queue(
            &dispatch_target(account, worker.id, &a, None),
            &QueueDispatchRequest {
                queue_name: "runtime-gate-throw".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: QueueMessageId::generate().to_string(),
                    timestamp_ms: 1_787_700_000_000,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("restart"),
                }],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("Queue custom event after restart");
    assert_eq!(restarted_queue.outcome, "exception");
    let restarted_scheduled = transport
        .dispatch_scheduled(
            &dispatch_target(account, worker.id, &a, None),
            &ScheduledDispatchRequest {
                scheduled_time_ms: 1_787_700_060_000,
                cron: "1 * * * *".to_owned(),
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
            },
            Duration::from_secs(5),
        )
        .await
        .expect("scheduled custom event after restart");
    assert_eq!(restarted_scheduled.outcome, "exception");

    http::cron_generation_cycle(&controller, &storage, &transport, account, worker.id).await;

    // A warm WorkerLoader entry must not bypass the pre-get source/descriptor
    // check. Corrupting the authority after warm load fails closed instead of
    // executing the already-cached isolate.
    let artifact = ArtifactRef::new(
        1,
        &hex::encode(a.artifact_sha256.unwrap()),
        a.artifact_size.unwrap(),
    )
    .unwrap();
    mock.corrupt_body(&artifact.physical_key("system/"));
    let warm_corrupt = dispatch(&transport, account, worker.id, &a, None, "must-not-run").await;
    assert_eq!(warm_corrupt.status, 500);
    assert!(warm_corrupt.body.contains("ARTIFACT_INTEGRITY_ERROR"));

    supervisor.shutdown().await;
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    assert!(supervisor.snapshot().pid.is_none());
}
