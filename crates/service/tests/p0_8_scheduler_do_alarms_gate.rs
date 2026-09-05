//! Real pinned-workerd P0.8 scheduler and Durable Object alarm conformance Gate.

#![cfg(feature = "test-support")]

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, DurableObjectsConfig, PlatformConfig, RuntimeConfig};
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, MetricsConfig, RequestId,
    ResourceId, SchedulerConfig, SystemSchedulerClock, WorkerId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::runtime_bridge::{
    AlarmDispatchOutcome, DispatchTarget, WorkerdTransport, bind_runtime_source,
    serve_runtime_source,
};
use open_compute_service::scheduler::SchedulerService;
use open_compute_service::{
    HealthCoordinator, MetricsRegistry, SqliteKvBindingExecutor, bind_binding_backend,
    serve_binding_backend,
};
use open_compute_storage::{
    AlarmProjection, ClaimResult, DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository,
    PlatformStorage, SchedulerStore, SchedulerSummary, VersionRecord, WorkerRepository,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, DurableObjectResourceDriver, ModuleInput,
    ModuleType, ResourceController, ResourcePins, RuntimeSource, RuntimeValidator,
    VersionBindingInput, VersionController,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_8_real_scheduler_alarm_matrix() {
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
        .with_max_request_body(32 * 1024 * 1024);
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

fn durable_objects_config() -> DurableObjectsConfig {
    DurableObjectsConfig {
        disk_high_watermark_percent: 98,
        disk_stop_writes_percent: 99,
        ..DurableObjectsConfig::default()
    }
}

fn create_namespace(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account_id: AccountId,
    worker_id: WorkerId,
    class_name: &str,
    now_ms: i64,
) -> ResourceId {
    let driver = DurableObjectResourceDriver::new(storage, worker_id, class_name);
    match ResourceController::new(storage, pins, driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: "alarm-namespace".to_owned(),
            idempotency_key: "p0-8-alarm".to_owned(),
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected namespace replay"),
    }
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
    supervisor: &WorkerdSupervisor,
) -> VersionRecord {
    match controller
        .create_version(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "version failed: {error:?}; runtime={:?}; diagnostics={:?}",
                supervisor.snapshot(),
                supervisor.last_diagnostics()
            )
        }) {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

fn version_request(
    account_id: AccountId,
    worker_id: WorkerId,
    namespace: ResourceId,
    key: &str,
    release: &str,
    now_ms: i64,
    promote: bool,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: do_source().as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    bindings.insert(
        "ALARM".to_owned(),
        VersionBindingInput {
            kind: BindingKind::DoNamespace,
            id: namespace,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    let mut vars = BTreeMap::new();
    vars.insert("RELEASE".to_owned(), serde_json::json!(release));
    if let Some(config) = raw_tcp_fixture_json() {
        vars.insert(
            "RAW_TCP_CONFIG_JSON".to_owned(),
            serde_json::Value::String(config),
        );
    }
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: promote.then_some(open_compute_storage::DeploymentSource::ScriptUpload),
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn do_source() -> &'static str {
    r#"import { DurableObject } from "cloudflare:workers";
import { connect } from "cloudflare:sockets";

async function rawTcpProbe(env) {
  if (!env.RAW_TCP_CONFIG_JSON) return false;
  const config = JSON.parse(env.RAW_TCP_CONFIG_JSON);
  const payload = new Uint8Array([17, 18, 19, 20]);
  const socket = connect({ hostname: config.hostname, port: Number(config.tcpPort) }, {
    allowHalfOpen: true, secureTransport: "off",
  });
  await socket.opened;
  const writer = socket.writable.getWriter();
  await writer.write(new TextEncoder().encode(`ECHO ${payload.byteLength}\n`));
  await writer.write(payload);
  await writer.close();
  writer.releaseLock();
  const echoed = new Uint8Array(await new Response(socket.readable).arrayBuffer());
  await socket.close();
  await socket.closed;
  let denied = false;
  const privateSocket = connect({
    hostname: config.privateHostname, port: Number(config.tcpPort),
  });
  try {
    await privateSocket.opened;
    await privateSocket.close();
  } catch {
    denied = true;
    try { await privateSocket.close(); } catch {}
  }
  if (echoed.length !== payload.length
      || !echoed.every((value, index) => value === payload[index]) || !denied) {
    throw new Error("DO raw TCP event-source policy mismatch");
  }
  return true;
}

function scalar(sql, query, fallback = 0) {
  const rows = sql.exec(query).toArray();
  return rows.length ? rows[0].value : fallback;
}

export class AlarmObject extends DurableObject {
  storageAtField = this.ctx.storage;
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx = ctx;
    this.env = env;
    this.constructorStorage = ctx.storage;
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS alarm_events(" +
      "id INTEGER PRIMARY KEY CHECK(id = 1), deliveries INTEGER NOT NULL, failures INTEGER NOT NULL, " +
      "last_release TEXT, last_retry_count INTEGER, last_is_retry INTEGER)"
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO alarm_events(id, deliveries, failures) VALUES(1, 0, 0) ON CONFLICT(id) DO NOTHING"
    );
  }
  proxyStable() {
    return this.storageAtField === this.ctx.storage && this.ctx.storage === this.constructorStorage;
  }
  async setAt(time) { await this.ctx.storage.setAlarm(time); return this.ctx.storage.getAlarm(); }
  async setDate(time) { await this.ctx.storage.setAlarm(new Date(time)); return this.ctx.storage.getAlarm(); }
  async getAlarmValue() { return this.ctx.storage.getAlarm(); }
  async deleteAlarmValue() { await this.ctx.storage.deleteAlarm(); return true; }
  async invalidAlarm(kind) {
    const value = kind === "zero" ? 0 : kind === "nan" ? NaN : kind === "infinity" ? Infinity : "bad";
    try { await this.ctx.storage.setAlarm(value); return false; } catch { return true; }
  }
  async transactionCommit(time) {
    await this.ctx.storage.transaction(async txn => {
      await txn.put("transaction", "committed");
      await txn.setAlarm(time + 1);
      await txn.setAlarm(time);
    });
    return this.ctx.storage.getAlarm();
  }
  async transactionRollback(time) {
    try {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("transaction", "rolled-back");
        await txn.setAlarm(time);
        throw new Error("rollback");
      });
    } catch {}
    const transactionValue = await this.ctx.storage.get("transaction");
    const alarmValue = await this.ctx.storage.getAlarm();
    return transactionValue === "committed" && alarmValue === null;
  }
  transactionSyncRejected() {
    try {
      this.ctx.storage.transactionSync(() => this.ctx.storage.setAlarm(Date.now() + 1000));
      return false;
    } catch (error) { return error instanceof TypeError; }
  }
  async failThenAlarm(count, time) {
    this.ctx.storage.sql.exec("UPDATE alarm_events SET failures = ? WHERE id = 1", count);
    await this.ctx.storage.setAlarm(time);
    return true;
  }
  async status() {
    const rows = this.ctx.storage.sql.exec(
      "SELECT deliveries, failures, last_release, last_retry_count, last_is_retry FROM alarm_events WHERE id = 1"
    ).toArray();
    const row = rows[0];
    return {
      alarm: await this.ctx.storage.getAlarm(),
      deliveries: Number(row.deliveries),
      failures: Number(row.failures),
      lastRelease: row.last_release,
      lastRetryCount: row.last_retry_count === null ? null : Number(row.last_retry_count),
      lastIsRetry: row.last_is_retry === null ? null : Number(row.last_is_retry) === 1,
      rawTcpAlarm: await this.ctx.storage.get("raw-tcp-alarm") === true,
    };
  }
  async deleteEverything(time) {
    await this.ctx.storage.put("delete-all-kv", true);
    this.ctx.storage.sql.exec("CREATE TABLE delete_all_sql(value INTEGER)");
    this.ctx.storage.sql.exec("INSERT INTO delete_all_sql VALUES(1)");
    this.ctx.storage.sql.exec("CREATE TABLE delete_all_parent(id INTEGER PRIMARY KEY)");
    this.ctx.storage.sql.exec(
      "CREATE TABLE delete_all_child(parent_id INTEGER REFERENCES delete_all_parent(id))"
    );
    this.ctx.storage.sql.exec("INSERT INTO delete_all_parent VALUES(1)");
    this.ctx.storage.sql.exec("INSERT INTO delete_all_child VALUES(1)");
    await this.ctx.storage.setAlarm(time);
    await this.ctx.storage.deleteAll();
    const kvGone = await this.ctx.storage.get("delete-all-kv") === undefined;
    const tables = this.ctx.storage.sql.exec(
      "SELECT name FROM sqlite_master WHERE name IN " +
      "('delete_all_sql', 'delete_all_parent', 'delete_all_child')"
    ).toArray();
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS alarm_events(" +
      "id INTEGER PRIMARY KEY CHECK(id = 1), deliveries INTEGER NOT NULL, failures INTEGER NOT NULL, " +
      "last_release TEXT, last_retry_count INTEGER, last_is_retry INTEGER)"
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO alarm_events(id, deliveries, failures) VALUES(1, 0, 0) ON CONFLICT(id) DO NOTHING"
    );
    return kvGone && tables.length === 0;
  }
  async alarm(info) {
    if (this.env.RAW_TCP_CONFIG_JSON
        && await this.ctx.storage.get("raw-tcp-alarm") !== true) {
      await rawTcpProbe(this.env);
      await this.ctx.storage.put("raw-tcp-alarm", true);
    }
    this.ctx.storage.sql.exec(
      "UPDATE alarm_events SET deliveries = deliveries + 1, last_release = ?, " +
      "last_retry_count = ?, last_is_retry = ? WHERE id = 1",
      this.env.RELEASE, info.retryCount, info.isRetry ? 1 : 0
    );
    const failures = Number(scalar(this.ctx.storage.sql, "SELECT failures AS value FROM alarm_events WHERE id = 1"));
    if (failures > 0) {
      this.ctx.storage.sql.exec("UPDATE alarm_events SET failures = failures - 1 WHERE id = 1");
      throw new Error("expected alarm failure");
    }
  }
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/raw-tcp") {
      return Response.json({ probed: await rawTcpProbe(this.env) });
    }
    if (path === "/proxy") {
      return new Response(String(this.proxyStable()));
    }
    return new Response(null, { status: 404 });
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const stub = env.ALARM.getByName("singleton");
    const time = Number(url.searchParams.get("time"));
    if (url.pathname === "/proxy-rpc") return new Response(String(await stub.proxyStable()));
    if (url.pathname === "/proxy-fetch") {
      return stub.fetch(new Request("https://object.invalid/proxy"));
    }
    if (url.pathname === "/raw-tcp") {
      return stub.fetch(new Request("https://object.invalid/raw-tcp"));
    }
    if (url.pathname === "/set") return new Response(String(await stub.setAt(time)));
    if (url.pathname === "/set-date") return new Response(String(await stub.setDate(time)));
    if (url.pathname === "/get") return new Response(String(await stub.getAlarmValue()));
    if (url.pathname === "/delete") return new Response(String(await stub.deleteAlarmValue()));
    if (url.pathname === "/txn-commit") return new Response(String(await stub.transactionCommit(time)));
    if (url.pathname === "/txn-rollback") return new Response(String(await stub.transactionRollback(time)));
    if (url.pathname === "/txn-sync") return new Response(String(await stub.transactionSyncRejected()));
    if (url.pathname === "/status") return Response.json(await stub.status());
    if (url.pathname === "/fail") {
      await stub.failThenAlarm(Number(url.searchParams.get("count")), time);
      return new Response("ok");
    }
    if (url.pathname === "/forge-private-alarm") {
      await stub.__openComputeAlarm({ rowToken: crypto.randomUUID(), retryCount: 0 });
      return new Response("forged");
    }
    if (url.pathname === "/delete-all") return new Response(String(await stub.deleteEverything(time)));
    if (url.pathname === "/invalid") {
      return Response.json({
        zero: await stub.invalidAlarm("zero"),
        nan: await stub.invalidAlarm("nan"),
        infinity: await stub.invalidAlarm("infinity"),
        type: await stub.invalidAlarm("type"),
      });
    }
    return new Response(null, { status: 404 });
  }
};
"#
}

struct DispatchResponse {
    status: u16,
    body: String,
}

async fn dispatch_path(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    dispatch(
        transport,
        account_id,
        worker_id,
        version,
        route_generation,
        path,
    )
    .await
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
    path: &str,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "alarm.test")
        .body(Body::empty())
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: None,
                route_generation: i64::try_from(route_generation).unwrap(),
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn status(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: WorkerId,
    version: &VersionRecord,
    route_generation: u64,
) -> serde_json::Value {
    let response = dispatch(
        transport,
        account_id,
        worker_id,
        version,
        route_generation,
        "/status",
    )
    .await;
    assert_ok(&response);
    serde_json::from_str(&response.body).unwrap()
}

#[track_caller]
fn assert_ok(response: &DispatchResponse) {
    assert_eq!(response.status, 200, "{}", response.body);
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed && start.elapsed() < timeout,
            "runtime failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, old_pid: i32, timeout: Duration) {
    let start = Instant::now();
    loop {
        let snapshot = supervisor.snapshot();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid) {
            return;
        }
        assert!(
            start.elapsed() < timeout,
            "runtime did not restart: {snapshot:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000
"#,
        mock.endpoint
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 64 * 1024 * 1024).unwrap())
}

fn raw_tcp_fixture_json() -> Option<String> {
    const NAMES: [&str; 3] = [
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TCP_PORT",
    ];
    let values = NAMES.map(std::env::var);
    if values.iter().all(Result::is_err) {
        return None;
    }
    let [hostname, private_hostname, tcp_port] =
        values.map(|value| value.expect("all raw TCP fixture values must be set"));
    Some(
        serde_json::json!({
            "hostname": hostname,
            "privateHostname": private_hostname,
            "tcpPort": tcp_port,
        })
        .to_string(),
    )
}

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    }
}

fn runtime_config() -> RuntimeConfig {
    let mut config = PlatformConfig::local_test_config().runtime;
    config.startup_timeout_ms = 20_000;
    config.shutdown_grace_ms = 1_000;
    config.kill_timeout_ms = 2_000;
    config
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}
