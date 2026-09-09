use super::*;

pub(super) async fn run() {
    let raw_tcp_qualification = raw_tcp_fixture_json().is_some();
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
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler_store = Arc::new(SchedulerStore::open(&scheduler_path, 5_000, now_ms()).unwrap());
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
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
        async move {
            serve_binding_backend(
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
                durable_objects_config(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                Some(scheduler),
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
            version: "p0.8-gate".to_owned(),
        },
        Duration::from_secs(20),
        open_compute_core::Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
        .with_test_request_body_limit(32 * 1024 * 1024);
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-8-gate.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
            ExternalServiceAddress::loopback("observability-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth, binding_auth],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "alarm-matrix",
            RequestId::generate(),
            10,
            1_000_000,
        )
        .unwrap();
    let namespace = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "AlarmObject",
        11,
    );
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(&storage, artifacts, validator, BundleLimits::default());
    let version_a = deploy(
        &versions,
        version_request(account, worker.id, namespace, "deploy-a", "A", 20, true),
        &supervisor,
    )
    .await;
    let generation_a = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    let scheduler_metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "p0.8-gate", "pinned-workerd").unwrap(),
    );
    let scheduler_health = HealthCoordinator::new();
    let mut scheduler_config = SchedulerConfig::default();
    scheduler_config.pools.alarm.claim_batch = 1;
    let scheduler = Arc::new(
        SchedulerService::new(
            scheduler_store.clone(),
            storage.clone(),
            transport.clone(),
            scheduler_config,
            open_compute_core::WorkflowsConfig::default(),
            Arc::new(SystemSchedulerClock),
        )
        .with_metrics(scheduler_metrics)
        .with_health(scheduler_health),
    );
    assert!(format!("{scheduler:?}").contains("SchedulerService"));
    assert!(Arc::ptr_eq(scheduler.store(), &scheduler_store));
    scheduler.pause();
    assert!(scheduler.is_paused());
    assert_eq!(scheduler.poll_once().await.unwrap(), 0);
    scheduler.resume();
    assert!(!scheduler.is_paused());

    let proxy_rpc = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/proxy-rpc",
    )
    .await;
    assert_ok(&proxy_rpc);
    assert_eq!(proxy_rpc.body, "true");
    let proxy_fetch = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/proxy-fetch",
    )
    .await;
    assert_ok(&proxy_fetch);
    assert_eq!(proxy_fetch.body, "true");
    if raw_tcp_qualification {
        let raw_tcp_fetch = dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/raw-tcp",
        )
        .await;
        assert_ok(&raw_tcp_fetch);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw_tcp_fetch.body).unwrap(),
            serde_json::json!({"probed": true})
        );
    }

    let invalid = dispatch(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/invalid",
    )
    .await;
    assert_eq!(invalid.status, 200, "{}", invalid.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&invalid.body).unwrap(),
        serde_json::json!({
            "zero": true, "nan": true, "infinity": true, "type": true
        })
    );
    let past = now_ms().saturating_sub(1_000).max(1);
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set?time={past}"),
        )
        .await,
    );
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 1);
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    let initial_status = status(&transport, account, worker.id, &version_a, generation_a).await;
    assert_eq!(initial_status["deliveries"], 1);
    assert_eq!(initial_status["alarm"], serde_json::Value::Null);
    assert_eq!(initial_status["lastRelease"], "A");
    assert_eq!(initial_status["lastRetryCount"], 0);
    assert_eq!(initial_status["lastIsRetry"], false);
    if raw_tcp_qualification {
        assert_eq!(initial_status["rawTcpAlarm"], true);
    }

    // The generated private methods cannot be invoked through tenant RPC.
    let forged = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/forge-private-alarm",
    )
    .await;
    assert_ne!(
        forged.status, 200,
        "private scheduler dispatch was tenant-callable"
    );
    assert_eq!(
        status(&transport, account, worker.id, &version_a, generation_a).await["deliveries"],
        1
    );

    // A claimed old token cannot invoke the handler or delete an overwrite.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set?time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    let [old_claim] = scheduler_store
        .claim_due(now_ms(), 60_000, 1)
        .unwrap()
        .try_into()
        .unwrap();
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set?time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    let stale = transport
        .dispatch_alarm(&old_claim, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(stale.outcome, AlarmDispatchOutcome::Stale);
    assert!(
        !scheduler_store
            .finish_claim(&old_claim, ClaimResult::Delete, now_ms())
            .unwrap()
    );
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(
        status(&transport, account, worker.id, &version_a, generation_a).await["deliveries"],
        2
    );

    // Delete is idempotent and removes only the exact projection token.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set?time={}", now_ms().saturating_add(60_000)),
        )
        .await,
    );
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/delete",
        )
        .await,
    );
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/delete",
        )
        .await,
    );
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 0);

    let date_due = now_ms().saturating_add(60_000);
    let date = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        &format!("/set-date?time={date_due}"),
    )
    .await;
    assert_ok(&date);
    assert_eq!(date.body, date_due.to_string());
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/delete",
        )
        .await,
    );

    // Async transaction commit flushes one coalesced projection; rollback flushes none.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/txn-commit?time={}", now_ms().saturating_add(60_000)),
        )
        .await,
    );
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 1);
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/delete",
        )
        .await,
    );
    let rollback = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        &format!("/txn-rollback?time={}", now_ms().saturating_add(60_000)),
    )
    .await;
    assert_ok(&rollback);
    assert_eq!(rollback.body, "true");
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 0);
    let sync = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/txn-sync",
    )
    .await;
    assert_ok(&sync);
    assert_eq!(sync.body, "true");

    // getAlarm and cold activation independently repair a missing projection.
    let future = now_ms().saturating_add(120_000);
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set?time={future}"),
        )
        .await,
    );
    let object = DurableObjectRepository::new(&storage)
        .alarm_repair_candidates(None, 1)
        .unwrap()
        .pop()
        .unwrap();
    scheduler_store
        .delete_object(namespace, object.object_id, object.generation)
        .unwrap();
    let get = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/get",
    )
    .await;
    assert_ok(&get);
    assert_eq!(get.body, future.to_string());
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 1);
    scheduler_store
        .delete_object(namespace, object.object_id, object.generation)
        .unwrap();
    let old_pid = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    let _ = status(&transport, account, worker.id, &version_a, generation_a).await;
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 1);

    // A bounded private scan independently reconstructs a missing projection and advances/reset
    // its stable cursor. With no authority row, the same path exact-clears a stale projection.
    scheduler_store
        .delete_object(namespace, object.object_id, object.generation)
        .unwrap();
    assert_eq!(scheduler.repair_once().await.unwrap(), 1);
    assert_eq!(scheduler.summary().unwrap().scheduled, 1);
    assert_eq!(scheduler.repair_once().await.unwrap(), 0);
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/delete",
        )
        .await,
    );
    assert_eq!(scheduler.repair_once().await.unwrap(), 1);
    assert_eq!(scheduler.summary().unwrap(), SchedulerSummary::default());
    assert_eq!(scheduler.repair_once().await.unwrap(), 0);

    // Pending alarms always execute current promoted or rolled-back code.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set?time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    let version_b = deploy(
        &versions,
        version_request(account, worker.id, namespace, "deploy-b", "B", 30, true),
        &supervisor,
    )
    .await;
    let generation_b = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert!(generation_b > generation_a);
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(
        status(&transport, account, worker.id, &version_b, generation_b).await["lastRelease"],
        "B"
    );

    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_b,
            generation_b,
            &format!("/set?time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    workers
        .promote_checked(
            account,
            worker.id,
            version_a.id,
            Some(version_b.id),
            Some(generation_b),
            RequestId::generate(),
            40,
        )
        .unwrap();
    let generation_rollback = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(
        status(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_rollback
        )
        .await["lastRelease"],
        "A"
    );

    // One real 2-second retry proves retryCount/isRetry and object-before-projection ordering.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            &format!("/fail?count=1&time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 1);
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    let retry_status = status(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
    )
    .await;
    assert_eq!(retry_status["lastRetryCount"], 1);
    assert_eq!(retry_status["lastIsRetry"], true);

    // deleteAll removes user KV/SQL plus alarm authority, then exact-clears projection.
    let delete_all = dispatch_path(
        &transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        &format!("/delete-all?time={}", now_ms().saturating_add(60_000)),
    )
    .await;
    assert_ok(&delete_all);
    assert_eq!(delete_all.body, "true");
    assert_eq!(scheduler_store.summary(now_ms()).unwrap().scheduled, 0);

    // A projection whose control authority disappeared is stale and is deleted without workerd.
    for (namespace_resource_id, row_token) in [
        (
            ResourceId::generate(),
            "00000000-0000-4000-8000-000000000008",
        ),
        (
            ResourceId::generate(),
            "00000000-0000-4000-8000-00000000000a",
        ),
    ] {
        scheduler_store
            .upsert_alarm(
                &AlarmProjection {
                    namespace_resource_id,
                    object_id: object.object_id,
                    object_generation: object.generation,
                    row_token: row_token.to_owned(),
                    due_at_ms: now_ms().saturating_sub(31_000).max(1),
                    target_version_id: version_a.id,
                    execution_generation: generation_rollback,
                    retry_count: 0,
                },
                now_ms(),
            )
            .unwrap();
    }
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(scheduler.summary().unwrap().scheduled, 1);
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(scheduler.summary().unwrap(), SchedulerSummary::default());

    // A projection carrying the wrong object row token dispatches as stale without invoking alarm.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            &format!("/set?time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    scheduler_store
        .upsert_alarm(
            &AlarmProjection {
                namespace_resource_id: namespace,
                object_id: object.object_id,
                object_generation: object.generation,
                row_token: "00000000-0000-4000-8000-000000000009".to_owned(),
                due_at_ms: now_ms().saturating_sub(1).max(1),
                target_version_id: version_a.id,
                execution_generation: generation_rollback,
                retry_count: 0,
            },
            now_ms(),
        )
        .unwrap();
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/delete",
        )
        .await,
    );

    // Exercise the production poll/repair loop and its bounded clean shutdown, not only poll_once.
    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = tokio::sync::watch::channel(false);
    let scheduler_task = tokio::spawn(scheduler.clone().run(scheduler_shutdown_rx));
    tokio::time::sleep(Duration::from_millis(150)).await;
    scheduler_shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), scheduler_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // An unknown transport result retains the claim lease instead of overlapping a retry.
    assert_ok(
        &dispatch_path(
            &transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            &format!("/set?time={}", now_ms().saturating_sub(1).max(1)),
        )
        .await,
    );
    supervisor.shutdown().await;
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(scheduler.summary().unwrap().claimed, 1);
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P0.8 scheduler/alarm API/token/retry/repair/promotion/deleteAll PASS");
}
