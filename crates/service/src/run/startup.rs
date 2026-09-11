use super::*;

pub(super) struct StoredPlatform {
    pub(super) loaded: LoadedConfig,
    pub(super) opts: RunInner,
    pub(super) metrics: Arc<MetricsRegistry>,
    pub(super) health: HealthCoordinator,
    pub(super) storage: Arc<PlatformStorage>,
    pub(super) scheduler_store: Arc<open_compute_storage::SchedulerStore>,
    pub(super) observability: Arc<ObservabilityService>,
}

pub(super) struct RuntimePlatform {
    pub(super) base: StoredPlatform,
    pub(super) redactor: Redactor,
    pub(super) package: open_compute_runtime::RuntimePackage,
    pub(super) runtime: open_compute_runtime::VerifiedRuntime,
    pub(super) runtime_lease_path: std::path::PathBuf,
    pub(super) durable_object_storage: std::path::PathBuf,
}

pub(super) struct ObjectPlatform {
    pub(super) base: RuntimePlatform,
    pub(super) maintenance_backend: open_compute_artifacts::ObjectBackend,
    pub(super) maintenance_object_storage: open_compute_core::ObjectStorageConfig,
    pub(super) r2_objects: R2ObjectStore,
    pub(super) ai_search_objects: AiSearchObjectStore,
    pub(super) store: ArtifactStore,
    pub(super) snapshot_pins: Arc<SnapshotPins>,
}

pub(super) struct PreparedPlatform {
    pub(super) base: ObjectPlatform,
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
}

pub(super) async fn prepare(
    loaded: LoadedConfig,
    opts: RunInner,
) -> Result<PreparedPlatform, PlatformError> {
    let stored = open_storage(initialize(loaded, opts)?).await?;
    let runtime = verify_runtime(stored).await?;
    let objects = connect_objects(runtime).await?;
    let cached = open_cache(objects).await?;
    bind_private_runtime(cached).await
}

struct InitialPlatform {
    loaded: LoadedConfig,
    opts: RunInner,
    metrics: Arc<MetricsRegistry>,
    health: HealthCoordinator,
}

fn initialize(loaded: LoadedConfig, opts: RunInner) -> Result<InitialPlatform, PlatformError> {
    let metrics = Arc::new(MetricsRegistry::new(
        &loaded.config.metrics,
        env!("CARGO_PKG_VERSION"),
        "unknown",
    )?);
    metrics.set_object_backend(loaded.config.object_storage.kind());
    record(&opts, "config");
    #[cfg(any(test, feature = "test-support"))]
    fail_after(&opts, FailAfter::Config, &metrics, StartStage::Config)?;
    metrics.inc_start(StartResult::Success, StartStage::Config);
    if let (Ok(capabilities), Ok(release_metadata)) = (
        platform_capabilities(&loaded.config),
        platform_release_metadata(&loaded),
    ) {
        metrics.set_release_identity(
            &capabilities.release.workerd_lock_sha256,
            &release_metadata.conformance_result,
        )?;
    }
    require_current_serving_schema(&loaded)?;
    let health = HealthCoordinator::new();
    health.set_component(
        ComponentName::Process,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;
    Ok(InitialPlatform {
        loaded,
        opts,
        metrics,
        health,
    })
}

async fn open_storage(initial: InitialPlatform) -> Result<StoredPlatform, PlatformError> {
    let storage_started = Instant::now();
    let result = tokio::task::spawn_blocking({
        let config = initial.loaded.config.clone();
        move || storage_bootstrap::bootstrap(&config)
    })
    .await;
    let (storage, scheduler_store) = match result {
        Ok(Ok(storage)) => storage,
        Ok(Err(error)) => return storage_failure(&initial.metrics, error),
        Err(_) => {
            return storage_failure(
                &initial.metrics,
                PlatformError::new(ErrorCode::MigrationFailed, "storage bootstrap task failed"),
            );
        }
    };
    let observability = open_observability(&initial, &storage);
    inspect_storage(&initial, &storage, storage_started)?;
    mark_storage_healthy(&initial.health)?;
    Ok(StoredPlatform {
        loaded: initial.loaded,
        opts: initial.opts,
        metrics: initial.metrics,
        health: initial.health,
        storage,
        scheduler_store,
        observability,
    })
}

fn storage_failure<T>(metrics: &MetricsRegistry, error: PlatformError) -> Result<T, PlatformError> {
    metrics.inc_start(StartResult::Failure, StartStage::Storage);
    Err(error)
}

fn open_observability(
    initial: &InitialPlatform,
    storage: &Arc<PlatformStorage>,
) -> Arc<ObservabilityService> {
    let store = storage
        .data_dir()
        .ensure_observability_db()
        .and_then(|path| {
            ObservabilityStore::open(
                &path,
                initial.loaded.config.data.sqlite_busy_timeout_ms,
                initial.loaded.config.observability.retention_ms,
                initial.loaded.config.observability.max_database_bytes,
            )
        });
    let store = match store {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
            tracing::warn!(
                code = error.code().as_str(),
                "Workers Logs database is unavailable; tenant execution remains available"
            );
            None
        }
    };
    ObservabilityService::new(
        storage.clone(),
        store,
        initial.loaded.config.observability.clone(),
        initial.metrics.clone(),
    )
}

fn inspect_storage(
    initial: &InitialPlatform,
    storage: &Arc<PlatformStorage>,
    started: Instant,
) -> Result<(), PlatformError> {
    refresh_p1_metrics(
        storage,
        &initial.metrics,
        initial.loaded.config.hardening.emergency_reserve_bytes,
    )?;
    initial.metrics.set_schema_version(
        u64::try_from(open_compute_storage::migrations::current_schema_version()).unwrap_or(0),
    );
    load_offline_metrics_receipts(storage.data_dir(), &initial.metrics);
    update_operations_health(
        storage.data_dir(),
        initial.loaded.config.hardening.snapshot_stale_after_ms,
        &initial.health,
    )?;
    initial
        .metrics
        .observe_sqlite(SqliteOp::Open, started.elapsed());
    initial
        .metrics
        .observe_sqlite(SqliteOp::Migrate, started.elapsed());
    record(&initial.opts, "storage");
    #[cfg(any(test, feature = "test-support"))]
    fail_after(
        &initial.opts,
        FailAfter::Storage,
        &initial.metrics,
        StartStage::Storage,
    )?;
    initial
        .metrics
        .inc_start(StartResult::Success, StartStage::Storage);
    Ok(())
}

fn mark_storage_healthy(health: &HealthCoordinator) -> Result<(), PlatformError> {
    for component in [
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::Scheduler,
        ComponentName::VectorizeStorage,
        ComponentName::VectorizeMutations,
        ComponentName::AiSearchStorage,
        ComponentName::AiSearchIndexing,
        ComponentName::AiModels,
    ] {
        health.set_component(
            component,
            ComponentState::Healthy,
            Some(ReadinessReason::Ready),
        )?;
    }
    Ok(())
}

async fn verify_runtime(base: StoredPlatform) -> Result<RuntimePlatform, PlatformError> {
    let redactor = Redactor::new();
    let runtime_lease_path = base.storage.data_dir().runtime_dir().join("child.lease");
    let runtime_dir = base.storage.data_dir().runtime_dir();
    let package = tokio::task::spawn_blocking(move || {
        open_compute_runtime::materialize_embedded_runtime(&runtime_dir)
    })
    .await
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "embedded runtime materialization task failed",
        )
    })??;
    let result = package
        .verify(
            Duration::from_millis(base.loaded.config.runtime.startup_timeout_ms),
            &redactor,
            &runtime_lease_path,
        )
        .await;
    let runtime = match result {
        Ok(runtime) => runtime,
        Err(error) => {
            base.metrics
                .inc_start(StartResult::Failure, StartStage::RuntimeVerify);
            return Err(error);
        }
    };
    base.metrics.set_workerd_version(runtime.version_output())?;
    let durable_object_storage = base.storage.data_dir().prepare_durable_object_storage(
        &base.storage.identity().platform_id.to_string(),
        runtime.version_output(),
    )?;
    update_do_storage_health(
        &base.storage,
        &base.loaded.config.durable_objects,
        &base.health,
        &base.metrics,
    )?;
    record(&base.opts, "runtime_verify");
    #[cfg(any(test, feature = "test-support"))]
    fail_after(
        &base.opts,
        FailAfter::RuntimeVerify,
        &base.metrics,
        StartStage::RuntimeVerify,
    )?;
    base.metrics
        .inc_start(StartResult::Success, StartStage::RuntimeVerify);
    Ok(RuntimePlatform {
        base,
        redactor,
        package,
        runtime,
        runtime_lease_path,
        durable_object_storage,
    })
}

async fn connect_objects(mut base: RuntimePlatform) -> Result<ObjectPlatform, PlatformError> {
    let connected = connect_object_backend(&base.base.loaded.config, base.base.storage.identity())
        .map_err(|error| object_failure(&base.base.metrics, error))?;
    if let Some(credentials) = &connected.credentials {
        base.redactor
            .register_secret_string(credentials.access_key_id());
        base.redactor
            .register_secret_string(credentials.secret_access_key());
    }
    let backend = connected.backend;
    preflight_objects(&base, &backend).await?;
    set_object_health(&base, &backend)?;
    let snapshot_pins = Arc::new(load_pins(&base, &backend).await);
    let maintenance_backend = backend.clone();
    let maintenance_object_storage = base.base.loaded.config.object_storage.clone();
    let r2_objects = R2ObjectStore::new(backend.clone());
    let ai_search_objects = AiSearchObjectStore::new(backend.clone());
    let store = ArtifactStore::new(backend);
    Ok(ObjectPlatform {
        base,
        maintenance_backend,
        maintenance_object_storage,
        r2_objects,
        ai_search_objects,
        store,
        snapshot_pins,
    })
}

fn object_failure(metrics: &MetricsRegistry, error: PlatformError) -> PlatformError {
    metrics.inc_start(StartResult::Failure, StartStage::ObjectStorage);
    error
}

async fn preflight_objects(
    base: &RuntimePlatform,
    backend: &open_compute_artifacts::ObjectBackend,
) -> Result<(), PlatformError> {
    backend
        .recover()
        .await
        .map_err(PlatformError::from)
        .map_err(|error| object_failure(&base.base.metrics, error))?;
    let outcome = preflight_object_storage(
        backend,
        base.base.storage.identity().platform_id,
        StartupId::generate(),
    )
    .await
    .map_err(|error| object_failure(&base.base.metrics, error))?;
    base.base.metrics.observe_preflight_success(&outcome);
    preflight_r2(
        backend,
        base.base.storage.identity().platform_id,
        StartupId::generate(),
    )
    .await
    .map_err(|error| object_failure(&base.base.metrics, error))?;
    base.base
        .storage
        .bind_object_authority(backend.kind(), &backend.authority_sha256())
        .map_err(|error| object_failure(&base.base.metrics, error))?;
    record(&base.base.opts, "object_storage");
    #[cfg(any(test, feature = "test-support"))]
    fail_after(
        &base.base.opts,
        FailAfter::ObjectStorage,
        &base.base.metrics,
        StartStage::ObjectStorage,
    )?;
    base.base
        .metrics
        .inc_start(StartResult::Success, StartStage::ObjectStorage);
    Ok(())
}

fn set_object_health(
    base: &RuntimePlatform,
    backend: &open_compute_artifacts::ObjectBackend,
) -> Result<(), PlatformError> {
    let state = match &base.base.loaded.config.object_storage {
        open_compute_core::ObjectStorageConfig::Local(local) => match backend.available_bytes() {
            Ok(Some(available)) if available < local.free_space_hard_bytes => {
                return Err(PlatformError::new(
                    ErrorCode::ObjectStorageCapacity,
                    "local object authority free space is below the hard limit",
                ));
            }
            Ok(Some(available)) if available < local.free_space_soft_bytes => {
                (ComponentState::Degraded, ReadinessReason::DiskSoftLimit)
            }
            Ok(Some(_)) => (ComponentState::Healthy, ReadinessReason::Ready),
            Ok(None) | Err(_) => (
                ComponentState::Degraded,
                ReadinessReason::ObjectStorageDegraded,
            ),
        },
        open_compute_core::ObjectStorageConfig::S3(_) => {
            (ComponentState::Healthy, ReadinessReason::Ready)
        }
    };
    base.base
        .health
        .set_component(ComponentName::ObjectStorage, state.0, Some(state.1))
}

async fn load_pins(
    base: &RuntimePlatform,
    backend: &open_compute_artifacts::ObjectBackend,
) -> SnapshotPins {
    match load_snapshot_pins(
        &base.base.loaded,
        base.base.storage.identity().platform_id,
        backend.clone(),
    )
    .await
    {
        Ok(pins) => pins,
        Err(error) => {
            base.base.metrics.inc_snapshot_inspect_failure();
            tracing::warn!(
                code = error.code().as_str(),
                "Snapshot pin inventory is unavailable; immutable object GC is disabled"
            );
            SnapshotPins::Unavailable
        }
    }
}

async fn open_cache(base: ObjectPlatform) -> Result<PreparedCache, PlatformError> {
    let cache = ArtifactCache::open(
        base.base.base.storage.data_dir().artifact_cache_dir(),
        base.base.base.loaded.config.cache.clone(),
        StartupId::generate(),
    )
    .map(Arc::new)
    .inspect_err(|_| {
        base.base
            .base
            .metrics
            .inc_start(StartResult::Failure, StartStage::Cache);
    })?;
    let response_cache = Arc::new(
        CacheBindingService::new(
            base.base.base.storage.clone(),
            base.store.clone(),
            cache.clone(),
            base.base.base.loaded.config.response_cache.clone(),
        )?
        .with_metrics(base.base.base.metrics.clone()),
    );
    let response_cache_manager = response_cache.manager();
    let images = Arc::new(
        ImageBindingService::new(
            base.base.base.storage.clone(),
            base.base.base.loaded.config.images.clone(),
        )
        .with_metrics(base.base.base.metrics.clone()),
    );
    let document_parser = Arc::new(DocumentParserBindingService::new(
        base.base.base.storage.clone(),
        base.base.base.loaded.config.document_parser.clone(),
        &base.base.base.loaded.config.ai,
    )?);
    base.base
        .base
        .metrics
        .set_cache(cache.total_bytes().await, cache.entry_count(), 0, 0);
    record(&base.base.base.opts, "cache");
    #[cfg(any(test, feature = "test-support"))]
    fail_after(
        &base.base.base.opts,
        FailAfter::Cache,
        &base.base.base.metrics,
        StartStage::Cache,
    )?;
    base.base
        .base
        .metrics
        .inc_start(StartResult::Success, StartStage::Cache);
    base.base.base.health.set_component(
        ComponentName::Cache,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;
    Ok(PreparedCache {
        base,
        cache,
        response_cache,
        response_cache_manager,
        images,
        document_parser,
    })
}

struct PreparedCache {
    base: ObjectPlatform,
    cache: Arc<ArtifactCache>,
    response_cache: Arc<CacheBindingService>,
    response_cache_manager: Arc<CacheManager>,
    images: Arc<ImageBindingService>,
    document_parser: Arc<DocumentParserBindingService>,
}

async fn bind_private_runtime(base: PreparedCache) -> Result<PreparedPlatform, PlatformError> {
    let generation_auth = GenerationAuthRegistry::new();
    let binding_generation_auth = GenerationAuthRegistry::new();
    let observability_generation_auth = GenerationAuthRegistry::new();
    let runtime_source_listener = bind_runtime_source().await?;
    let runtime_source_addr = private_addr(&runtime_source_listener)?;
    let binding_backend_listener = bind_binding_backend().await?;
    let binding_backend_addr = private_addr(&binding_backend_listener)?;
    let observability_backend_listener = bind_observability_backend().await?;
    let observability_backend_addr = private_addr(&observability_backend_listener)?;
    let compiler = StaticConfigCompiler::new(
        base.base.base.runtime.clone(),
        base.base.base.package.lock_path(),
        base.base.base.package.assets_dir(),
        base.base.base.base.storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Duration::from_millis(base.base.base.base.loaded.config.runtime.startup_timeout_ms),
        base.base.base.redactor.clone(),
    )
    .with_generation_auth(generation_auth.clone())
    .with_binding_generation_auth(binding_generation_auth.clone())
    .with_observability_generation_auth(observability_generation_auth.clone())
    .with_durable_objects_config(base.base.base.base.loaded.config.durable_objects.clone());
    record(&base.base.base.base.opts, "compile");
    #[cfg(any(test, feature = "test-support"))]
    fail_after(
        &base.base.base.base.opts,
        FailAfter::Compile,
        &base.base.base.base.metrics,
        StartStage::Compile,
    )?;
    base.base
        .base
        .base
        .metrics
        .inc_start(StartResult::Success, StartStage::Compile);
    Ok(PreparedPlatform {
        base: base.base,
        cache: base.cache,
        response_cache: base.response_cache,
        response_cache_manager: base.response_cache_manager,
        images: base.images,
        document_parser: base.document_parser,
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
    })
}

fn private_addr(listener: &tokio::net::TcpListener) -> Result<SocketAddr, PlatformError> {
    listener.local_addr().map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "failed to inspect private runtime listener",
        )
    })
}
