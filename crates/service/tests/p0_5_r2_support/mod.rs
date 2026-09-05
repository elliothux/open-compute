//! Shared real-runtime R2 Gate setup.

use super::*;

pub(super) struct R2Gate {
    pub(super) _temp: tempfile::TempDir,
    pub(super) mock: MockS3,
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) objects: R2ObjectStore,
    pub(super) artifacts: ArtifactStore,
    pub(super) pins: ResourcePins,
    pub(super) transport: WorkerdTransport,
    pub(super) supervisor: Arc<WorkerdSupervisor>,
    pub(super) shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub(super) source_task: tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>,
    pub(super) binding_task: tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>>,
}

pub(super) async fn start(r2_config: R2Config, s3_endpoint: Option<&str>) -> R2Gate {
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
    let mock = MockS3::spawn("open-compute").await;
    if let Some(endpoint) = s3_endpoint {
        let url: axum::http::Uri = endpoint.parse().unwrap();
        assert_eq!(url.scheme_str(), Some("http"));
        assert!(
            url.port_u16().is_some(),
            "qualification fixture needs an explicit port"
        );
        assert_eq!(url.path(), "/");
        assert!(url.query().is_none());
        assert_eq!(
            url.host(),
            Some("127.0.0.1"),
            "qualification fixture must be loopback-only"
        );
    }
    let (artifacts, objects) = stores(s3_endpoint.unwrap_or(&mock.endpoint));
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");

    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let pins = ResourcePins::new();
    let r2_service = Arc::new(
        R2BindingService::new(
            storage.clone(),
            pins.clone(),
            objects.clone(),
            r2_config.clone(),
        )
        .unwrap(),
    );
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
        let binding_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = pins.clone();
        let pins_for_d1 = pins.clone();
        let storage_for_d1 = storage.clone();
        async move {
            serve_binding_backend(
                binding_listener,
                binding_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                Some(r2_service),
                Some(Arc::new(open_compute_service::D1BindingService::new(
                    storage_for_d1,
                    pins_for_d1,
                    open_compute_core::D1Config::default(),
                ))),
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
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
        assets,
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p0.5-gate".to_owned(),
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
            lease_path: Some(storage.data_dir().runtime_dir().join("p0-5-gate.lease")),
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

    R2Gate {
        _temp: temp,
        mock,
        storage,
        objects,
        artifacts,
        pins,
        transport,
        supervisor,
        shutdown_tx,
        source_task,
        binding_task,
    }
}
