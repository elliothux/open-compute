use super::*;

pub(super) async fn run() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let scheduler_store =
        Arc::new(SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5_000, 1).unwrap());
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let cache = Arc::new(
        ArtifactCache::open(
            storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let runtime = verify_runtime_binary(
        &lock,
        &workerd,
        Duration::from_secs(10),
        &open_compute_core::Redactor::new(),
    )
    .await
    .expect("formal pinned runtime");
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let resource_pins = ResourcePins::new();
    let version_pins = VersionPins::new();
    let service_invocations = Arc::new(ServiceInvocationRegistry::new(
        storage.clone(),
        version_pins.clone(),
    ));
    let (shutdown_tx, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let backend_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = resource_pins.clone();
        let scheduler = scheduler_store.clone();
        let artifacts = artifacts.clone();
        let cache = cache.clone();
        let version_pins = version_pins.clone();
        let services = service_invocations.clone();
        async move {
            let assets = Arc::new(AssetBindingService::new(
                backend_storage.clone(),
                artifacts,
                cache,
                version_pins,
            ));
            serve_binding_backend_with_assets(
                binding_listener,
                backend_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
                DurableObjectsConfig {
                    disk_high_watermark_percent: 98,
                    disk_stop_writes_percent: 99,
                    ..DurableObjectsConfig::default()
                },
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                Some(scheduler),
                assets,
                services,
                None,
                None,
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        root.join("packages/runtime"),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p2.2-scheduler-gate".to_owned(),
        },
        Duration::from_secs(20),
        open_compute_core::Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone());
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
            redactor: open_compute_core::Redactor::new(),
            lease_path: Some(
                storage
                    .data_dir()
                    .runtime_dir()
                    .join("p2-2-scheduler-gate.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let events = create_queue(
        &storage,
        scheduler_store.clone(),
        account,
        "events",
        "events",
    );
    let dlq = create_queue(
        &storage,
        scheduler_store.clone(),
        account,
        "events-dlq",
        "events-dlq",
    );
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "queue-scheduler",
            RequestId::generate(),
            10,
            1_000_000,
        )
        .unwrap();
    let (caller, _) = workers
        .create_worker(
            account,
            "queue-caller",
            RequestId::generate(),
            11,
            1_000_000,
        )
        .unwrap();
    let namespace = create_namespace(&storage, resource_pins.clone(), account, worker.id);
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(product_promotion_for_test(
        storage.clone(),
        scheduler_store.clone(),
    ));
    let version = deploy(
        &versions,
        consumer_request(
            account,
            worker.id,
            events,
            dlq,
            namespace,
            "scheduler-bound",
            20,
        ),
    )
    .await;
    let caller_version = deploy(
        &versions,
        caller_request(account, caller.id, worker.id, "scheduler-caller", 21),
    )
    .await;
    let generation = i64::try_from(
        workers
            .get_worker(account, worker.id)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let caller_generation = i64::try_from(
        workers
            .get_worker(account, caller.id)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let scheduler = Arc::new(SchedulerService::new(
        scheduler_store.clone(),
        storage.clone(),
        transport.clone(),
        SchedulerConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        Arc::new(SystemSchedulerClock),
    ));
    let db = storage.data_dir().scheduler_db_path();
    let catalog = QueueRepository::new(storage.db());
    let events_row = catalog.get(account, events).unwrap();
    let empty = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        None,
        "/operator/metrics",
    )
    .await;
    assert_eq!(empty.status, 200, "{}", empty.body);
    let empty_metrics: serde_json::Value = serde_json::from_str(&empty.body).unwrap();
    assert_eq!(empty_metrics["backlogCount"], 0);
    assert!(empty_metrics.get("oldestMessageTimestamp").is_none());

    let worker_send = dispatch(
        &transport, account, worker.id, &version, generation, None, "/worker",
    )
    .await;
    assert_eq!(worker_send.status, 200, "{}", worker_send.body);
    let do_send = dispatch(
        &transport, account, worker.id, &version, generation, None, "/do",
    )
    .await;
    assert_eq!(
        do_send.status,
        200,
        "{}; diagnostics={:?}",
        do_send.body,
        supervisor.last_diagnostics()
    );
    let do_result: serde_json::Value = serde_json::from_str(&do_send.body).unwrap();
    assert_eq!(do_result["backlogCount"], 2, "{do_result}");
    let workflow = transport
        .dispatch_workflow(
            &WorkflowTarget {
                account_id: account,
                definition_id: WorkflowId::generate(),
                definition_name: "queue-flow".to_owned(),
                workflow_version_id: WorkflowVersionId::generate(),
                worker_id: worker.id,
                worker_version_id: version.id,
                worker_code_sha256: version.worker_code_sha256,
                class_name: "Flow".to_owned(),
                loader_schema_version: 1,
                capability_version: 1,
                descriptor_sha256: [7; 32],
            },
            &WorkflowRunRequest {
                fence: WorkflowFence {
                    instance_id: WorkflowInstanceId::generate(),
                    instance_generation: 1,
                    run_token: WorkflowToken::from_bytes([8; 32]),
                },
                external_instance_id: "queue-scheduler-flow".to_owned(),
                definition_name: "queue-flow".to_owned(),
                created_at_ms: 1_700_000_000_000,
                payload_base64: "T0NEVgECAA==".to_owned(),
                rollback: false,
                schedule: None,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("workflow queue mutation");
    match workflow.result {
        WorkflowOutcome::Complete { .. } => {}
        outcome => panic!("unexpected Workflow outcome: {outcome:?}"),
    }
    let service_send = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        caller_generation,
        None,
        "/",
    )
    .await;
    assert_eq!(service_send.status, 200, "{}", service_send.body);
    assert_eq!(service_send.body, "service");
    assert_eq!(
        scheduler_store
            .queue_metrics(
                events,
                events_row.lifecycle_generation,
                events_row.config_generation,
            )
            .unwrap()
            .backlog_count,
        4
    );
    let live = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        None,
        "/operator/metrics",
    )
    .await;
    let live_metrics: serde_json::Value = serde_json::from_str(&live.body).unwrap();
    assert_eq!(live_metrics["backlogCount"], 4);
    assert!(live_metrics.get("oldestMessageTimestamp").is_some());
    assert!(apply_due(&scheduler).await >= 1);
    assert_eq!(
        scheduler_store
            .queue_metrics(
                events,
                events_row.lifecycle_generation,
                events_row.config_generation,
            )
            .unwrap()
            .backlog_count,
        0
    );

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "ack-then-retry",
    )
    .await;
    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "retry-then-ack",
    )
    .await;
    let mixed = claim_one(&scheduler).await;
    assert_eq!(mixed.messages.len(), 2);
    let before_mixed = wall_ms();
    assert_eq!(claimed_count(&db), 2, "claimed messages must not ack early");
    scheduler.clone().dispatch_queue_batch(mixed).await;
    assert!(text_missing(&db, "ack-then-retry"));
    let retried = text_row(&db, "retry-then-ack").expect("retry-then-ack must remain");
    assert_eq!(retried.state, "ready");
    assert_eq!(retried.attempts, 1);
    let retry_delay = retried.available_at_ms.saturating_sub(before_mixed);
    assert!(
        (3_500..=5_500).contains(&retry_delay),
        "retry-then-ack delay {retry_delay}"
    );
    let [second_mixed] = scheduler_store
        .claim_queue_batches(retried.available_at_ms, 60_000, 0, 1, None)
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(second_mixed.messages[0].delivery_attempt, 2);
    scheduler.clone().dispatch_queue_batch(second_mixed).await;
    assert!(text_missing(&db, "retry-then-ack"));

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "ack-all-then-retry-all",
    )
    .await;
    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "ack-all-then-retry-all",
    )
    .await;
    let ack_all = claim_one(&scheduler).await;
    assert_eq!(ack_all.messages.len(), 2);
    scheduler.clone().dispatch_queue_batch(ack_all).await;
    assert_eq!(text_count(&db, "ack-all-then-retry-all"), 0);

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "retry-all-then-ack-all",
    )
    .await;
    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "retry-all-then-ack-all",
    )
    .await;
    let retry_all = claim_one(&scheduler).await;
    assert_eq!(retry_all.messages.len(), 2);
    let before_retry_all = wall_ms();
    scheduler.clone().dispatch_queue_batch(retry_all).await;
    let delayed = text_rows(&db, "retry-all-then-ack-all");
    assert_eq!(delayed.len(), 2);
    for row in &delayed {
        assert_eq!(row.state, "ready");
        assert_eq!(row.attempts, 1);
        let delay = row.available_at_ms.saturating_sub(before_retry_all);
        assert!((5_500..=7_500).contains(&delay), "retryAll delay {delay}");
    }
    let [retry_all_second] = scheduler_store
        .claim_queue_batches(delayed[0].available_at_ms, 60_000, 0, 1, None)
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(retry_all_second.messages.len(), 2);
    assert!(
        retry_all_second
            .messages
            .iter()
            .all(|message| message.delivery_attempt == 2)
    );
    scheduler
        .clone()
        .dispatch_queue_batch(retry_all_second)
        .await;
    assert_eq!(text_count(&db, "retry-all-then-ack-all"), 0);

    send_text(
        &transport, account, worker.id, &version, generation, "throw",
    )
    .await;
    let thrown = claim_one(&scheduler).await;
    assert_eq!(thrown.messages[0].delivery_attempt, 1);
    assert_eq!(claimed_count(&db), 1);
    scheduler.clone().dispatch_queue_batch(thrown).await;
    let after_throw = text_row(&db, "throw").expect("handler throw must not ack");
    assert_eq!(after_throw.state, "ready");
    assert_eq!(after_throw.attempts, 1);
    let [throw_retry] = scheduler_store
        .claim_queue_batches(
            after_throw.available_at_ms.max(wall_ms()),
            60_000,
            0,
            1,
            None,
        )
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(throw_retry.messages[0].delivery_attempt, 2);
    scheduler.clone().dispatch_queue_batch(throw_retry).await;
    assert!(text_missing(&db, "throw"));

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "wait-until",
    )
    .await;
    let rejected = claim_one(&scheduler).await;
    scheduler.clone().dispatch_queue_batch(rejected).await;
    let after_wait = text_row(&db, "wait-until").expect("rejected waitUntil must not ack");
    assert_eq!(after_wait.state, "ready");
    assert_eq!(after_wait.attempts, 1);
    let [wait_retry] = scheduler_store
        .claim_queue_batches(
            after_wait.available_at_ms.max(wall_ms()),
            60_000,
            0,
            1,
            None,
        )
        .unwrap()
        .0
        .try_into()
        .unwrap();
    scheduler.clone().dispatch_queue_batch(wait_retry).await;
    assert!(text_missing(&db, "wait-until"));

    send_text(
        &transport, account, worker.id, &version, generation, "reclaim",
    )
    .await;
    let leased = claim_one(&scheduler).await;
    assert_eq!(leased.messages[0].delivery_attempt, 1);
    assert_eq!(claimed_count(&db), 1, "visibility timeout must not ack");
    assert_eq!(
        scheduler_store
            .recover_expired_queue_batches(leased.claim_until_ms, 0, 8)
            .unwrap(),
        1
    );
    let recovered = text_row(&db, "reclaim").expect("reclaimed message");
    assert_eq!(recovered.state, "ready");
    assert_eq!(recovered.attempts, 0);
    let [redelivered] = scheduler_store
        .claim_queue_batches(leased.claim_until_ms, 60_000, 0, 1, None)
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(redelivered.messages[0].id, leased.messages[0].id);
    assert_eq!(redelivered.messages[0].delivery_attempt, 1);
    assert_ne!(redelivered.claim_token, leased.claim_token);
    scheduler.clone().dispatch_queue_batch(redelivered).await;
    assert!(text_missing(&db, "reclaim"));

    send_text(
        &transport, account, worker.id, &version, generation, "dlq-me",
    )
    .await;
    let first_dlq = claim_one(&scheduler).await;
    assert_eq!(first_dlq.messages[0].delivery_attempt, 1);
    scheduler.clone().dispatch_queue_batch(first_dlq).await;
    let retried_dlq = text_row(&db, "dlq-me").expect("first retry stays on source");
    assert_eq!(retried_dlq.attempts, 1);
    let [second_dlq] = scheduler_store
        .claim_queue_batches(
            retried_dlq.available_at_ms.max(wall_ms()),
            60_000,
            0,
            1,
            None,
        )
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(second_dlq.messages[0].delivery_attempt, 2);
    scheduler.clone().dispatch_queue_batch(second_dlq).await;
    assert_eq!(text_queue(&db, "dlq-me"), Some(dlq.to_string()));
    let dlq_row = catalog.get(account, dlq).unwrap();
    assert_eq!(
        scheduler_store
            .queue_metrics(
                events,
                events_row.lifecycle_generation,
                events_row.config_generation
            )
            .unwrap()
            .backlog_count,
        0
    );
    assert_eq!(
        scheduler_store
            .queue_metrics(dlq, dlq_row.lifecycle_generation, dlq_row.config_generation)
            .unwrap()
            .backlog_count,
        1
    );

    let v8 = dispatch(
        &transport, account, worker.id, &version, generation, None, "/v8",
    )
    .await;
    assert_eq!(v8.status, 200, "{}", v8.body);
    let v8_body = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT body FROM queue_messages WHERE content_type = 'v8' ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .unwrap();
    assert!(v8_body.starts_with(&[0x4f, 0x43, 0x44, 0x56]));
    let before_restart = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, before_restart, Duration::from_secs(30)).await;
    let restored = claim_one(&scheduler).await;
    assert_eq!(restored.messages[0].content_type.as_str(), "v8");
    scheduler.clone().dispatch_queue_batch(restored).await;
    assert_eq!(
        Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM queue_messages WHERE content_type = 'v8'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    let drained = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        None,
        "/operator/metrics",
    )
    .await;
    let drained_metrics: serde_json::Value = serde_json::from_str(&drained.body).unwrap();
    assert_eq!(drained_metrics["backlogCount"], 0);
    assert!(drained_metrics.get("oldestMessageTimestamp").is_none());
    assert_eq!(apply_due(&scheduler).await, 0);

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
}
