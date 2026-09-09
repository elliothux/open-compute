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
    let scheduler = output_crash::open_scheduler(&storage);
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
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
        let scheduler = scheduler.clone();
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
            version: "p0.7-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
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
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-7-gate.lease")),
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
    let (output_queue, output_queue_resource) =
        output_crash::create_queue(&storage, scheduler.clone(), account);
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "do-matrix", RequestId::generate(), 10, 1_000_000)
        .unwrap();
    let counter = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "Counter",
        "counter",
        11,
    );
    let other = create_namespace(
        &storage,
        resource_pins.clone(),
        account,
        worker.id,
        "OtherCounter",
        "other",
        12,
    );
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(&storage, artifacts, validator, BundleLimits::default());
    let version_a = deploy(
        &versions,
        version_request(
            account,
            worker.id,
            counter,
            other,
            output_queue_resource,
            "deploy-a",
            "A",
            20,
            true,
        ),
        &supervisor,
    )
    .await;
    let generation_a = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;

    let case = MatrixCase {
        storage: &storage,
        scheduler: &scheduler,
        transport: &transport,
        supervisor: &supervisor,
        versions: &versions,
        account,
        worker: &worker,
        counter,
        other,
        output_queue,
        output_queue_resource,
        version_a: &version_a,
        generation_a,
    };
    let named_id = protocol::verify_identity(&case).await;
    protocol::verify_fetch_and_rpc(&case).await;
    protocol::verify_rpc_edges(&case).await;
    protocol::verify_ordering(&case).await;
    protocol::verify_storage_and_parallelism(&case).await;
    lifecycle::reject_missing_class(&case).await;
    let (version_b, generation_b) = lifecycle::promote(&case).await;
    let generation_rollback =
        lifecycle::rollback_and_restart(&case, &version_b, generation_b).await;
    lifecycle::verify_recovery(&case, generation_rollback).await;
    lifecycle::delete_objects(&case, &named_id, generation_rollback).await;

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    println!("P0.7 identity/fetch/RPC/SQL/parallel/promotion/rollback/restart/delete/purge PASS");
}

struct MatrixCase<'a> {
    storage: &'a Arc<PlatformStorage>,
    scheduler: &'a Arc<open_compute_storage::SchedulerStore>,
    transport: &'a WorkerdTransport,
    supervisor: &'a Arc<WorkerdSupervisor>,
    versions: &'a VersionController<'a>,
    account: AccountId,
    worker: &'a open_compute_storage::WorkerRecord,
    counter: ResourceId,
    other: ResourceId,
    output_queue: open_compute_core::QueueId,
    output_queue_resource: ResourceId,
    version_a: &'a VersionRecord,
    generation_a: u64,
}

mod lifecycle;
mod protocol;
