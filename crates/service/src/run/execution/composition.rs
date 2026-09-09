use super::startup::{ObjectPlatform, PreparedPlatform, RuntimePlatform, StoredPlatform};
use super::*;

pub(super) struct ComposedPlatform {
    pub(super) loaded: LoadedConfig,
    pub(super) opts: RunInner,
    pub(super) metrics: Arc<MetricsRegistry>,
    pub(super) health: HealthCoordinator,
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) scheduler_store: Arc<open_compute_storage::SchedulerStore>,
    pub(super) observability: Arc<ObservabilityService>,
    pub(super) cache: Arc<ArtifactCache>,
    pub(super) response_cache: Arc<CacheBindingService>,
    pub(super) response_cache_manager: Arc<CacheManager>,
    pub(super) images: Arc<ImageBindingService>,
    pub(super) document_parser: Arc<DocumentParserBindingService>,
    pub(super) generation_auth: GenerationAuthRegistry,
    pub(super) binding_generation_auth: GenerationAuthRegistry,
    pub(super) observability_generation_auth: GenerationAuthRegistry,
    pub(super) runtime_source_listener: tokio::net::TcpListener,
    pub(super) runtime_source_addr: SocketAddr,
    pub(super) binding_backend_listener: tokio::net::TcpListener,
    pub(super) binding_backend_addr: SocketAddr,
    pub(super) observability_backend_listener: tokio::net::TcpListener,
    pub(super) observability_backend_addr: SocketAddr,
    pub(super) compiler: StaticConfigCompiler,
    pub(super) maintenance_backend: open_compute_artifacts::ObjectBackend,
    pub(super) maintenance_object_storage: open_compute_core::ObjectStorageConfig,
    pub(super) r2_objects: R2ObjectStore,
    pub(super) store: ArtifactStore,
    pub(super) snapshot_pins: Arc<SnapshotPins>,
    pub(super) redactor: Redactor,
    pub(super) runtime: open_compute_runtime::VerifiedRuntime,
    pub(super) runtime_lease_path: std::path::PathBuf,
    pub(super) durable_object_storage: std::path::PathBuf,
    pub(super) public_addr: SocketAddr,
    pub(super) admin_addr: Option<SocketAddr>,
    pub(super) merged: bool,
    pub(super) version_pins: VersionPins,
    pub(super) service_invocations: Arc<ServiceInvocationRegistry>,
    pub(super) supervisor_handle: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>,
    pub(super) transport: WorkerdTransport,
    pub(super) scheduler_service: Arc<SchedulerService>,
    pub(super) bundle_limits: BundleLimits,
    pub(super) resource_pins: ResourcePins,
    pub(super) r2_backend: Arc<R2BindingService>,
    pub(super) d1_backend: Arc<D1BindingService>,
    pub(super) maintenance_do_lifecycle: DurableObjectLifecycleService,
    pub(super) binding_executor: Arc<SqliteKvBindingExecutor>,
    pub(super) binding_ai_search: Arc<AiSearchBindingService>,
    pub(super) dashboard_dispatch: Arc<RwLock<Option<crate::dashboard::DashboardDispatch>>>,
    pub(super) generation_startup_id: StartupId,
    pub(super) dashboard_auth: Arc<crate::dashboard_auth::DashboardAuth>,
    pub(super) state: HttpState,
}

pub(super) async fn compose(prepared: PreparedPlatform) -> Result<ComposedPlatform, PlatformError> {
    let PreparedPlatform {
        base,
        cache,
        response_cache,
        response_cache_manager,
        images,
        document_parser,
        generation_auth,
        binding_generation_auth,
        observability_generation_auth,
        runtime_source_listener,
        runtime_source_addr,
        binding_backend_listener,
        binding_backend_addr,
        observability_backend_listener,
        observability_backend_addr,
        compiler,
    } = prepared;
    let ObjectPlatform {
        base,
        maintenance_backend,
        maintenance_object_storage,
        r2_objects,
        ai_search_objects,
        store,
        snapshot_pins,
    } = base;
    let RuntimePlatform {
        base,
        redactor,
        package: _,
        runtime,
        runtime_lease_path,
        durable_object_storage,
    } = base;
    let StoredPlatform {
        loaded,
        opts,
        metrics,
        health,
        storage,
        scheduler_store,
        observability,
    } = base;
    let public_addr = loaded.config.server.public_addr()?;
    let admin_addr = loaded.config.server.admin_addr()?;
    let merged = !matches!(admin_addr, Some(admin) if admin != public_addr);

    let version_pins = VersionPins::new();
    let service_invocations = Arc::new(ServiceInvocationRegistry::new(
        storage.clone(),
        version_pins.clone(),
    ));
    let supervisor_handle: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>> = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(generation_auth.clone(), supervisor_handle.clone())
        .with_version_pins(version_pins.clone())
        .with_service_invocations(service_invocations.as_ref().clone());
    let scheduler_service = Arc::new(
        SchedulerService::new(
            scheduler_store.clone(),
            storage.clone(),
            transport.clone(),
            loaded.config.scheduler.clone(),
            loaded.config.workflows.clone(),
            Arc::new(SystemSchedulerClock),
        )
        .with_metrics(metrics.clone())
        .with_health(health.clone()),
    );
    scheduler_service.repair_products(1_000)?;
    scheduler_service.repair_workflows(32)?;
    let bundle_limits = BundleLimits {
        max_artifact_bytes: usize::try_from(loaded.config.workers.max_bundle_bytes).map_err(
            |_| PlatformError::new(ErrorCode::LimitInvalid, "Worker bundle limit is invalid"),
        )?,
        ..BundleLimits::default()
    };
    let resource_pins = ResourcePins::new();
    let r2_backend = Arc::new(
        R2BindingService::new(
            storage.clone(),
            resource_pins.clone(),
            r2_objects.clone(),
            loaded.config.r2.clone(),
        )?
        .with_metrics(metrics.clone()),
    );
    let r2_api = R2ApiState::new(
        storage.clone(),
        r2_objects.clone(),
        resource_pins.clone(),
        loaded.config.r2.clone(),
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    )
    .with_binding(r2_backend.clone());
    r2_api.reconcile_pending().await?;
    let d1_backend = Arc::new(
        D1BindingService::new(
            storage.clone(),
            resource_pins.clone(),
            loaded.config.d1.clone(),
        )
        .with_metrics(metrics.clone()),
    );
    let d1_api = D1ApiState::new(
        storage.clone(),
        store.clone(),
        resource_pins.clone(),
        d1_backend.clone(),
        loaded.config.d1.clone(),
        loaded.config.hardening.max_resources_per_kind_per_account,
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    );
    let do_lifecycle = DurableObjectLifecycleService::new(
        storage.clone(),
        transport.clone(),
        loaded.config.durable_objects.clone(),
    )
    .with_metrics(metrics.clone())
    .with_scheduler(Some(scheduler_store.clone()));
    let queue_api = QueueApiState::new(
        storage.clone(),
        scheduler_service.clone(),
        loaded.config.queues.max_consumer_concurrency,
    )
    .with_metrics(metrics.clone())
    .with_default_max_backlog_bytes(loaded.config.queues.default_max_backlog_bytes);
    let workflow_api = crate::workflow_http::WorkflowApiState::new(
        storage.clone(),
        scheduler_store.clone(),
        transport.clone(),
        loaded.config.workflows.clone(),
    );
    queue_api.reconcile_pending().await?;
    metrics.set_do_storage_watermark(0);
    let maintenance_do_lifecycle = do_lifecycle.clone();
    let binding_executor = Arc::new(
        SqliteKvBindingExecutor::with_config(
            storage.clone(),
            Arc::new(SystemClock),
            &loaded.config.kv,
        )
        .with_metrics(metrics.clone()),
    );
    let binding_ai_search = Arc::new(
        AiSearchBindingService::new(
            storage.clone(),
            resource_pins.clone(),
            loaded.config.ai.clone(),
            ai_search_objects,
            snapshot_pins.clone(),
            document_parser.clone(),
        )?
        .with_metrics(metrics.clone()),
    );
    let worker_api = WorkerApiState::new(
        storage.clone(),
        store.clone(),
        transport.clone(),
        version_pins.clone(),
        bundle_limits,
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    )
    .with_response_cache(response_cache_manager.clone())
    .with_queue_consumer_limit(loaded.config.queues.max_consumer_concurrency)
    .with_product_promoter(Arc::new(P23PromotionCoordinator::new(
        storage.clone(),
        scheduler_store.clone(),
        Duration::from_millis(loaded.config.scheduler.shutdown_drain_ms),
    )))
    .with_observability(observability.clone());
    let dashboard_dispatch = Arc::new(RwLock::new(None));
    let generation_startup_id = StartupId::generate();
    let dashboard_auth = Arc::new(crate::dashboard_auth::DashboardAuth::new(
        generation_startup_id,
    ));
    let state = HttpState::new(
        health.clone(),
        metrics.clone(),
        loaded.config.metrics.enabled,
        loaded.config.dashboard.enabled,
        &loaded.config.server,
    )?
    .with_platform_storage(storage.clone())
    .with_dashboard_dispatch(dashboard_dispatch.clone())
    .with_dashboard_auth(dashboard_auth.clone())
    .with_worker_api(worker_api)
    .with_kv_api(
        KvApiState::new(
            storage.clone(),
            store.clone(),
            resource_pins.clone(),
            binding_executor.clone(),
            loaded.config.kv.clone(),
            loaded.config.hardening.max_resources_per_kind_per_account,
            Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
        )
        .with_snapshot_pins(snapshot_pins.clone()),
    )
    .with_r2_api(r2_api)
    .with_d1_api(d1_api)
    .with_queue_api(Some(queue_api))
    .with_workflow_api(Some(workflow_api))
    .with_scheduler(Some(scheduler_service.clone()))
    .with_cache_images_api(CacheImagesApiState::new(
        storage.clone(),
        response_cache_manager.clone(),
        images.clone(),
        store.clone(),
        loaded.config.workers.clone(),
        snapshot_pins.clone(),
        metrics.clone(),
    ))
    .with_search_api(
        SearchApiState::new(
            storage.clone(),
            resource_pins.clone(),
            loaded.config.data.sqlite_busy_timeout_ms,
            Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
        )
        .with_ai_search(binding_ai_search.clone()),
    );

    Ok(ComposedPlatform {
        loaded,
        opts,
        metrics,
        health,
        storage,
        scheduler_store,
        observability,
        cache,
        response_cache,
        response_cache_manager,
        images,
        document_parser,
        generation_auth,
        binding_generation_auth,
        observability_generation_auth,
        runtime_source_listener,
        runtime_source_addr,
        binding_backend_listener,
        binding_backend_addr,
        observability_backend_listener,
        observability_backend_addr,
        compiler,
        maintenance_backend,
        maintenance_object_storage,
        r2_objects,
        store,
        snapshot_pins,
        redactor,
        runtime,
        runtime_lease_path,
        durable_object_storage,
        public_addr,
        admin_addr,
        merged,
        version_pins,
        service_invocations,
        supervisor_handle,
        transport,
        scheduler_service,
        bundle_limits,
        resource_pins,
        r2_backend,
        d1_backend,
        maintenance_do_lifecycle,
        binding_executor,
        binding_ai_search,
        dashboard_dispatch,
        generation_startup_id,
        dashboard_auth,
        state,
    })
}
