//! Production `run` composition and shutdown.

use crate::binding_backend::{bind_binding_backend, serve_binding_backend_with_scheduler};
use crate::capabilities::{platform_capabilities, platform_release_metadata};
use crate::config_load::LoadedConfig;
use crate::d1_backend::D1BindingService;
use crate::d1_http::D1ApiState;
use crate::do_http::DoApiState;
use crate::health::HealthCoordinator;
use crate::http::{self, HttpState, SanitizedSupervisor};
use crate::kv_backend::SqliteKvBindingExecutor;
use crate::kv_http::KvApiState;
use crate::metrics::{
    DoFacetReloadReason, KvMaintenance, MetricsRegistry, SqliteOp, StartResult, StartStage,
};
use crate::p2_3_promotion::P23PromotionCoordinator;
use crate::queue_http::QueueApiState;
use crate::r2_backend::R2BindingService;
use crate::r2_http::R2ApiState;
use crate::r2_maintenance::R2Maintenance;
use crate::runtime_bridge::{WorkerdTransport, bind_runtime_source, serve_runtime_source};
use crate::scheduler::SchedulerService;
use crate::snapshot_pins::{SnapshotPins, load_snapshot_pins};
use crate::workers_http::WorkerApiState;
#[path = "run_p1.rs"]
pub(super) mod p1;
use open_compute_artifacts::{
    ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore, R2ObjectStore,
    S3ArtifactClient, preflight_r2, preflight_s3, resolve_s3_credentials,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, PlatformError, ReadinessReason, Redactor, RequestId,
    StartupId, SystemSchedulerClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, WorkerdSupervisor, WorkerdSupervisorOptions,
    verify_runtime_binary_with_staging_lease,
};
use open_compute_storage::{
    DurableObjectRepository, PlatformStorage, SchedulerStore, WorkerRepository,
};
use open_compute_workers::{BundleLimits, DeploymentPins, ResourcePins, RuntimeSource};
use p1::{
    load_offline_metrics_receipts, refresh_metrics as refresh_p1_metrics,
    require_current_serving_schema, update_operations_health,
};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

/// Injected failure after a named stage.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailAfter {
    /// After config.
    Config,
    /// After storage bootstrap.
    Storage,
    /// After runtime verify.
    RuntimeVerify,
    /// After S3 preflight.
    S3,
    /// After cache open.
    Cache,
    /// After compile construction.
    Compile,
    /// After listeners bind.
    Listen,
}

/// Options for [`run_platform_with`]. Survives async task migration.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Fail after this stage.
    pub fail_after: Option<FailAfter>,
    /// Recorded stage names.
    pub stages: Arc<Mutex<Vec<&'static str>>>,
    /// Last bound public address, if listeners were acquired.
    pub last_public_addr: Arc<Mutex<Option<SocketAddr>>>,
}

#[derive(Clone, Debug, Default)]
struct RunInner {
    #[cfg(any(test, feature = "test-support"))]
    fail_after: Option<FailAfter>,
    #[cfg(any(test, feature = "test-support"))]
    stages: Arc<Mutex<Vec<&'static str>>>,
    #[cfg(any(test, feature = "test-support"))]
    last_public_addr: Arc<Mutex<Option<SocketAddr>>>,
}

/// Run the platform until SIGINT/SIGTERM.
pub async fn run_platform(loaded: LoadedConfig) -> Result<(), PlatformError> {
    run_inner(loaded, RunInner::default()).await
}

/// Run with explicit test-support options.
#[cfg(any(test, feature = "test-support"))]
pub async fn run_platform_with(
    loaded: LoadedConfig,
    opts: RunOptions,
) -> Result<(), PlatformError> {
    run_inner(
        loaded,
        RunInner {
            fail_after: opts.fail_after,
            stages: opts.stages,
            last_public_addr: opts.last_public_addr,
        },
    )
    .await
}

async fn run_inner(loaded: LoadedConfig, opts: RunInner) -> Result<(), PlatformError> {
    let metrics =
        match MetricsRegistry::new(&loaded.config.metrics, env!("CARGO_PKG_VERSION"), "unknown") {
            Ok(m) => Arc::new(m),
            Err(err) => return Err(err),
        };
    record(&opts, "config");
    fail_after(&opts, FailAfterDummy::Config, &metrics, StartStage::Config)?;
    metrics.inc_start(StartResult::Success, StartStage::Config);
    if let (Ok(capabilities), Ok(release_metadata)) = (
        platform_capabilities(&loaded),
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

    let clock = SystemClock;
    let storage_started = Instant::now();
    let storage = match tokio::task::spawn_blocking({
        let cfg = loaded.config.storage.clone();
        let hardening = loaded.config.hardening.clone();
        let recovery_batch = loaded.config.workers.delete_recovery_batch;
        move || {
            let storage = PlatformStorage::bootstrap_with_hardening(&cfg, &hardening, &clock)?;
            open_compute_storage::KvPaths::open(storage.data_dir().root())?
                .cleanup_write_staging()?;
            open_compute_storage::R2Staging::open(storage.data_dir().root())?.cleanup()?;
            let maintenance_now = unix_ms();
            WorkerRepository::new(storage.db())
                .prune_expired_idempotency(maintenance_now, recovery_batch)?;
            WorkerRepository::new(storage.db()).recover_deleting_deployments(
                RequestId::generate(),
                maintenance_now,
                recovery_batch,
            )?;
            let scheduler_path = storage.data_dir().ensure_scheduler_db()?;
            drop(SchedulerStore::open(
                &scheduler_path,
                cfg.sqlite_busy_timeout_ms,
                maintenance_now,
            )?);
            let schemas = open_compute_storage::inspect_owned_schema(
                storage.data_dir(),
                storage.db(),
                cfg.sqlite_busy_timeout_ms,
                maintenance_now,
            )?;
            if schemas.control
                != u32::try_from(open_compute_storage::migrations::current_schema_version())
                    .unwrap_or(u32::MAX)
                || schemas.scheduler
                    != u32::try_from(open_compute_storage::current_scheduler_schema_version())
                        .unwrap_or(u32::MAX)
                || schemas.kv_min != open_compute_storage::KV_SCHEMA_VERSION
                || schemas.kv_max != open_compute_storage::KV_SCHEMA_VERSION
                || schemas.d1_min != open_compute_storage::D1_DATABASE_SCHEMA_VERSION
                || schemas.d1_max != open_compute_storage::D1_DATABASE_SCHEMA_VERSION
            {
                return Err(PlatformError::new(
                    ErrorCode::UpgradeRequired,
                    "platformd run refuses a mixed project-owned schema tuple",
                ));
            }
            Ok::<_, PlatformError>(storage)
        }
    })
    .await
    {
        Ok(Ok(storage)) => storage,
        Ok(Err(err)) => {
            metrics.inc_start(StartResult::Failure, StartStage::Storage);
            return Err(err);
        }
        Err(_) => {
            metrics.inc_start(StartResult::Failure, StartStage::Storage);
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "storage bootstrap task failed",
            ));
        }
    };
    let storage = Arc::new(storage);
    refresh_p1_metrics(
        &storage,
        &metrics,
        loaded.config.hardening.emergency_reserve_bytes,
    )?;
    metrics.set_schema_state(
        u64::try_from(open_compute_storage::migrations::current_schema_version()).unwrap_or(0),
        u64::try_from(open_compute_storage::migrations::current_schema_version()).unwrap_or(0),
    );
    load_offline_metrics_receipts(storage.data_dir(), &metrics);
    update_operations_health(
        storage.data_dir(),
        loaded.config.hardening.snapshot_stale_after_ms,
        &health,
    )?;
    let scheduler_store = match storage.data_dir().ensure_scheduler_db().and_then(|path| {
        SchedulerStore::open(
            &path,
            loaded.config.storage.sqlite_busy_timeout_ms,
            unix_ms(),
        )
    }) {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
            tracing::warn!(
                code = error.code().as_str(),
                "scheduler unavailable; ordinary Worker and Durable Object traffic remains enabled"
            );
            None
        }
    };
    metrics.observe_sqlite(SqliteOp::Open, storage_started.elapsed());
    metrics.observe_sqlite(SqliteOp::Migrate, storage_started.elapsed());
    record(&opts, "storage");
    if let Err(err) = fail_after(
        &opts,
        FailAfterDummy::Storage,
        &metrics,
        StartStage::Storage,
    ) {
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::Storage);
    health.set_component(
        ComponentName::DataDir,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;
    health.set_component(
        ComponentName::ControlDb,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;
    health.set_component(
        ComponentName::MasterKey,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;
    health.set_component(
        ComponentName::Scheduler,
        if scheduler_store.is_some() {
            ComponentState::Healthy
        } else {
            ComponentState::Degraded
        },
        Some(if scheduler_store.is_some() {
            ReadinessReason::Ready
        } else {
            ReadinessReason::SchedulerUnavailable
        }),
    )?;

    let mut redactor = Redactor::new();
    let runtime_lease_path = storage.data_dir().runtime_dir().join("child.lease");
    let runtime = match verify_runtime_binary_with_staging_lease(
        &loaded.config.runtime.lock_file,
        &loaded.config.runtime.binary,
        Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
        &redactor,
        &runtime_lease_path,
    )
    .await
    {
        Ok(rt) => rt,
        Err(err) => {
            metrics.inc_start(StartResult::Failure, StartStage::RuntimeVerify);
            drop(storage);
            return Err(err);
        }
    };
    metrics.set_workerd_version(runtime.version_output())?;
    let durable_object_storage = storage.data_dir().prepare_durable_object_storage(
        &storage.identity().platform_id.to_string(),
        runtime.version_output(),
    )?;
    update_do_storage_health(&storage, &loaded.config.durable_objects, &health, &metrics)?;
    record(&opts, "runtime_verify");
    if let Err(err) = fail_after(
        &opts,
        FailAfterDummy::RuntimeVerify,
        &metrics,
        StartStage::RuntimeVerify,
    ) {
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::RuntimeVerify);

    let creds = match resolve_s3_credentials(&loaded.config.s3) {
        Ok(c) => c,
        Err(err) => {
            metrics.inc_start(StartResult::Failure, StartStage::S3);
            drop(storage);
            return Err(err);
        }
    };
    redactor.register_secret_string(creds.access_key_id());
    redactor.register_secret_string(creds.secret_access_key());
    let client = match S3ArtifactClient::connect(
        &loaded.config.s3,
        &creds,
        loaded
            .config
            .cache
            .max_artifact_bytes
            .max(loaded.config.kv.namespace_quota_bytes),
    ) {
        Ok(c) => c,
        Err(err) => {
            metrics.inc_start(StartResult::Failure, StartStage::S3);
            drop(storage);
            return Err(err);
        }
    };
    match preflight_s3(
        &client,
        storage.identity().platform_id,
        StartupId::generate(),
    )
    .await
    {
        Ok(outcome) => metrics.observe_preflight_success(&outcome),
        Err(err) => {
            metrics.inc_start(StartResult::Failure, StartStage::S3);
            drop(storage);
            return Err(err);
        }
    }
    if let Err(err) = preflight_r2(
        &client,
        storage.identity().platform_id,
        StartupId::generate(),
    )
    .await
    {
        metrics.inc_start(StartResult::Failure, StartStage::S3);
        drop(storage);
        return Err(err);
    }
    record(&opts, "s3");
    if let Err(err) = fail_after(&opts, FailAfterDummy::S3, &metrics, StartStage::S3) {
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::S3);
    health.set_component(
        ComponentName::S3,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;

    let snapshot_pins = Arc::new(
        match load_snapshot_pins(&loaded, storage.identity().platform_id, client.clone()).await {
            Ok(pins) => pins,
            Err(error) => {
                metrics.inc_snapshot_inspect_failure();
                tracing::warn!(
                    code = error.code().as_str(),
                    "Snapshot pin inventory is unavailable; immutable object GC is disabled"
                );
                SnapshotPins::Unavailable
            }
        },
    );

    let r2_objects = R2ObjectStore::new(client.clone());
    let store = ArtifactStore::new(client);
    let cache = match ArtifactCache::open(
        storage.data_dir().artifact_cache_dir(),
        loaded.config.cache.clone(),
        StartupId::generate(),
    ) {
        Ok(c) => Arc::new(c),
        Err(err) => {
            metrics.inc_start(StartResult::Failure, StartStage::Cache);
            drop(store);
            drop(storage);
            return Err(err);
        }
    };
    metrics.set_cache(cache.total_bytes().await, cache.entry_count(), 0, 0);
    record(&opts, "cache");
    if let Err(err) = fail_after(&opts, FailAfterDummy::Cache, &metrics, StartStage::Cache) {
        drop(cache);
        drop(store);
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::Cache);
    health.set_component(
        ComponentName::Cache,
        ComponentState::Healthy,
        Some(ReadinessReason::Ready),
    )?;

    let generation_auth = GenerationAuthRegistry::new();
    let binding_generation_auth = GenerationAuthRegistry::new();
    let runtime_source_listener = bind_runtime_source().await?;
    let runtime_source_addr = runtime_source_listener.local_addr().map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "failed to inspect private RuntimeSource listener",
        )
    })?;
    let binding_backend_listener = bind_binding_backend().await?;
    let binding_backend_addr = binding_backend_listener.local_addr().map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "failed to inspect private binding backend listener",
        )
    })?;
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        loaded.config.runtime.lock_file.clone(),
        loaded.config.runtime.assets_dir.clone(),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
        redactor.clone(),
    )
    .with_generation_auth(generation_auth.clone())
    .with_binding_generation_auth(binding_generation_auth.clone())
    .with_durable_objects_config(loaded.config.durable_objects.clone());
    record(&opts, "compile");
    if let Err(err) = fail_after(
        &opts,
        FailAfterDummy::Compile,
        &metrics,
        StartStage::Compile,
    ) {
        drop(cache);
        drop(store);
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::Compile);

    let public_addr = loaded.config.server.public_addr()?;
    let admin_addr = loaded.config.server.admin_addr()?;
    let merged = !matches!(admin_addr, Some(admin) if admin != public_addr);

    let supervisor_handle: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>> = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(generation_auth.clone(), supervisor_handle.clone())
        .with_max_request_body(
            usize::try_from(loaded.config.workers.max_request_body_bytes).map_err(|_| {
                PlatformError::new(ErrorCode::LimitInvalid, "Worker body limit is invalid")
            })?,
        );
    let scheduler_service = scheduler_store.as_ref().map(|store| {
        Arc::new(
            SchedulerService::new(
                store.clone(),
                storage.clone(),
                transport.clone(),
                loaded.config.scheduler.clone(),
                Arc::new(SystemSchedulerClock),
            )
            .with_metrics(metrics.clone())
            .with_health(health.clone()),
        )
    });
    if let Some(scheduler) = &scheduler_service {
        scheduler.repair_products(1_000)?;
    }
    let bundle_limits = BundleLimits {
        max_artifact_bytes: usize::try_from(loaded.config.workers.max_bundle_bytes).map_err(
            |_| PlatformError::new(ErrorCode::LimitInvalid, "Worker bundle limit is invalid"),
        )?,
        ..BundleLimits::default()
    };
    let deployment_pins = DeploymentPins::new();
    let resource_pins = ResourcePins::new();
    let r2_api = R2ApiState::new(
        storage.clone(),
        r2_objects.clone(),
        resource_pins.clone(),
        loaded.config.r2.clone(),
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    )
    .with_metrics(metrics.clone());
    r2_api.reconcile_pending().await?;
    let r2_backend = Arc::new(
        R2BindingService::new(
            storage.clone(),
            resource_pins.clone(),
            r2_objects.clone(),
            loaded.config.r2.clone(),
        )?
        .with_metrics(metrics.clone()),
    );
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
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    );
    d1_api.reconcile_pending().await?;
    let do_api = DoApiState::new(
        storage.clone(),
        resource_pins.clone(),
        transport.clone(),
        loaded.config.durable_objects.clone(),
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    )
    .with_metrics(metrics.clone())
    .with_scheduler(scheduler_store.clone());
    let queue_api = scheduler_store.as_ref().map(|scheduler| {
        QueueApiState::new(storage.clone(), scheduler.clone())
            .with_metrics(metrics.clone())
            .with_default_max_backlog_bytes(loaded.config.queues.default_max_backlog_bytes)
    });
    if let Some(api) = &queue_api {
        api.reconcile_pending().await?;
    }
    metrics.set_do_runtime_gauges(0, 0, 0);
    let maintenance_do_api = do_api.clone();
    let supervisor_for_http = supervisor_handle.clone();
    let mut worker_api = WorkerApiState::new(
        storage.clone(),
        store.clone(),
        transport.clone(),
        deployment_pins.clone(),
        bundle_limits,
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    )
    .with_queue_consumer_limit(loaded.config.queues.max_consumer_concurrency);
    if let Some(scheduler) = &scheduler_store {
        worker_api = worker_api.with_product_promoter(Arc::new(P23PromotionCoordinator::new(
            storage.clone(),
            scheduler.clone(),
            Duration::from_millis(loaded.config.scheduler.shutdown_drain_ms),
        )));
    }
    let state = HttpState::new(
        health.clone(),
        metrics.clone(),
        loaded.config.metrics.enabled,
        &loaded.config.server,
        Arc::new(move || {
            supervisor_for_http
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|s| SanitizedSupervisor::from(&s.snapshot())))
        }),
    )?
    .with_worker_api(worker_api)
    .with_kv_api(
        KvApiState::new(
            storage.clone(),
            store.clone(),
            resource_pins.clone(),
            loaded.config.kv.clone(),
            Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
        )
        .with_snapshot_pins(snapshot_pins.clone()),
    )
    .with_r2_api(r2_api)
    .with_d1_api(d1_api)
    .with_do_api(do_api)
    .with_queue_api(queue_api)
    .with_scheduler(scheduler_service.clone());

    let public_listener = match http::bind(public_addr).await {
        Ok(l) => l,
        Err(err) => {
            metrics.inc_start(StartResult::Failure, StartStage::Listen);
            drop(cache);
            drop(store);
            drop(storage);
            return Err(err);
        }
    };
    remember_bind(&opts, public_listener.local_addr().ok());
    let admin_listener = if merged {
        None
    } else {
        match http::bind(admin_addr.expect("distinct admin")).await {
            Ok(l) => Some(l),
            Err(err) => {
                metrics.inc_start(StartResult::Failure, StartStage::Listen);
                drop(public_listener);
                drop(cache);
                drop(store);
                drop(storage);
                return Err(err);
            }
        }
    };
    record(&opts, "listen");
    if let Err(err) = fail_after(&opts, FailAfterDummy::Listen, &metrics, StartStage::Listen) {
        drop(admin_listener);
        drop(public_listener);
        drop(cache);
        drop(store);
        drop(storage);
        return Err(err);
    }
    metrics.inc_start(StartResult::Success, StartStage::Listen);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = watch::channel(false);
    let mut shutdown_maintenance = shutdown_rx.clone();
    let maintenance_storage = storage.clone();
    let maintenance_store = store.clone();
    let maintenance_cache = cache.clone();
    let maintenance_config = loaded.config.workers.clone();
    let maintenance_kv_config = loaded.config.kv.clone();
    let maintenance_r2_config = loaded.config.r2.clone();
    let maintenance_do_config = loaded.config.durable_objects.clone();
    let maintenance_snapshot_stale_after_ms = loaded.config.hardening.snapshot_stale_after_ms;
    let maintenance_emergency_reserve_bytes = loaded.config.hardening.emergency_reserve_bytes;
    let maintenance_r2_objects = r2_objects;
    let maintenance_health = health.clone();
    let maintenance_pins = deployment_pins;
    let maintenance_resource_pins = resource_pins.clone();
    let maintenance_metrics = metrics.clone();
    let maintenance_snapshot_pins = snapshot_pins;
    let maintenance_task = tokio::spawn(async move {
        let mut r2_maintenance = R2Maintenance::default();
        let mut interval = tokio::time::interval(Duration::from_millis(
            maintenance_config.artifact_gc_interval_ms,
        ));
        loop {
            tokio::select! {
                _ = shutdown_maintenance.changed() => return Ok(()),
                _ = interval.tick() => {
                    run_worker_maintenance(
                        &maintenance_storage,
                        &maintenance_store,
                        &maintenance_cache,
                        &maintenance_pins,
                        &maintenance_config,
                        &maintenance_snapshot_pins,
                    ).await;
                    run_kv_maintenance(
                        &maintenance_storage,
                        &maintenance_resource_pins,
                        &maintenance_kv_config,
                        &maintenance_metrics,
                    ).await;
                    r2_maintenance.run(
                        &maintenance_storage,
                        &maintenance_r2_objects,
                        &maintenance_r2_config,
                        &maintenance_health,
                    ).await;
                    let _ = update_do_storage_health(
                        &maintenance_storage,
                        &maintenance_do_config,
                        &maintenance_health,
                        &maintenance_metrics,
                    );
                    let _ = update_operations_health(
                        maintenance_storage.data_dir(),
                        maintenance_snapshot_stale_after_ms,
                        &maintenance_health,
                    );
                    if let Err(error) = refresh_p1_metrics(
                        &maintenance_storage,
                        &maintenance_metrics,
                        maintenance_emergency_reserve_bytes,
                    ) {
                        tracing::warn!(
                            code = error.code().as_str(),
                            "P1 disk and resource metrics refresh failed"
                        );
                    }
                    let _ = maintenance_do_api.reconcile_pending().await;
                }
            }
        }
    });
    let runtime_source =
        RuntimeSource::new(storage.clone(), store.clone(), bundle_limits).with_cache(cache.clone());
    let mut shutdown_source = shutdown_rx.clone();
    let source_auth = generation_auth.clone();
    let runtime_source_task = tokio::spawn(async move {
        serve_runtime_source(
            runtime_source_listener,
            runtime_source,
            source_auth,
            async move {
                let _ = shutdown_source.changed().await;
            },
        )
        .await
    });
    let mut shutdown_binding = shutdown_rx.clone();
    let binding_storage = storage.clone();
    let binding_executor = Arc::new(
        SqliteKvBindingExecutor::with_config(
            storage.clone(),
            Arc::new(SystemClock),
            &loaded.config.kv,
        )
        .with_metrics(metrics.clone()),
    );
    let binding_auth = binding_generation_auth.clone();
    let binding_metrics = metrics.clone();
    let binding_do_config = loaded.config.durable_objects.clone();
    let binding_queue_config = loaded.config.queues.clone();
    let binding_backend_task = tokio::spawn(async move {
        serve_binding_backend_with_scheduler(
            binding_backend_listener,
            binding_storage,
            binding_auth,
            resource_pins,
            binding_executor,
            Some(binding_metrics),
            Some(r2_backend),
            Some(d1_backend),
            binding_do_config,
            binding_queue_config,
            scheduler_store,
            async move {
                let _ = shutdown_binding.changed().await;
            },
        )
        .await
    });
    let public_router = if merged {
        http::merged_router(state.clone())
    } else {
        http::public_router(state.clone())
    };
    let mut shutdown_public = shutdown_rx.clone();
    let public_task = tokio::spawn(async move {
        http::serve_until(public_listener, public_router, async move {
            let _ = shutdown_public.changed().await;
        })
        .await
    });
    let admin_task = if let Some(listener) = admin_listener {
        let router = http::admin_router(state.clone());
        let mut rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            http::serve_until(listener, router, async move {
                let _ = rx.changed().await;
            })
            .await
        }))
    } else {
        None
    };

    let supervisor = Arc::new(WorkerdSupervisor::new_with_services_and_auth(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: loaded.config.runtime.clone(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor,
            lease_path: Some(runtime_lease_path),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", runtime_source_addr)?,
            ExternalServiceAddress::loopback("binding-backend", binding_backend_addr)?,
        ],
        vec![DirectoryServicePath::local(
            "do-storage",
            &durable_object_storage,
        )?],
        vec![generation_auth, binding_generation_auth],
    ));
    *supervisor_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(supervisor.clone());
    supervisor.start();
    record(&opts, "supervisor");
    metrics.inc_start(StartResult::Success, StartStage::Supervisor);
    let scheduler_task = scheduler_service
        .map(|service| tokio::spawn(async move { service.run(scheduler_shutdown_rx).await }));

    let mut watch_rx = supervisor.subscribe();
    let health_watch = health.clone();
    let metrics_watch = metrics.clone();
    let storage_watch = storage.clone();
    tokio::spawn(async move {
        let mut running_pid = None;
        loop {
            let snap = watch_rx.borrow().clone();
            metrics_watch.observe_supervisor(&snap);
            if snap.state == open_compute_runtime::SupervisorState::Running {
                if running_pid.is_some()
                    && running_pid != snap.pid
                    && DurableObjectRepository::new(&storage_watch)
                        .count_live_objects()
                        .is_ok_and(|count| count > 0)
                {
                    metrics_watch.inc_do_facet_reload(DoFacetReloadReason::Restart);
                }
                running_pid = snap.pid;
            }
            if let Err(err) = health_watch.apply_supervisor(&snap) {
                tracing::error!(
                    code = err.code().as_str(),
                    "runtime health transition failed"
                );
            }
            if watch_rx.changed().await.is_err() {
                break;
            }
        }
    });

    let run_err = wait_signals_and_servers(
        &health,
        &supervisor,
        shutdown_tx,
        scheduler_shutdown_tx,
        public_task,
        admin_task,
        runtime_source_task,
        binding_backend_task,
        maintenance_task,
        scheduler_task,
    )
    .await;

    drop(cache);
    drop(store);
    drop(storage);
    match run_err {
        None => Ok(()),
        Some(err) => Err(err),
    }
}

fn update_do_storage_health(
    storage: &PlatformStorage,
    config: &open_compute_core::DurableObjectsConfig,
    health: &HealthCoordinator,
    metrics: &MetricsRegistry,
) -> Result<(), PlatformError> {
    let used = storage.filesystem_used_percent()?;
    let watermark = if used >= config.disk_stop_writes_percent {
        2
    } else if used >= config.disk_high_watermark_percent {
        1
    } else {
        0
    };
    metrics.set_do_runtime_gauges(0, 0, watermark);
    let state = if watermark == 0 {
        ComponentState::Healthy
    } else {
        ComponentState::Degraded
    };
    let reason = match watermark {
        0 => ReadinessReason::Ready,
        1 => ReadinessReason::DiskSoftLimit,
        _ => ReadinessReason::DiskHardLimit,
    };
    health.set_component(ComponentName::DataDir, state, Some(reason))
}

#[allow(clippy::too_many_arguments)]
async fn wait_signals_and_servers(
    health: &HealthCoordinator,
    supervisor: &WorkerdSupervisor,
    shutdown_tx: watch::Sender<bool>,
    scheduler_shutdown_tx: watch::Sender<bool>,
    public_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    admin_task: Option<tokio::task::JoinHandle<Result<(), PlatformError>>>,
    runtime_source_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    binding_backend_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    maintenance_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    scheduler_task: Option<tokio::task::JoinHandle<Result<(), PlatformError>>>,
) -> Option<PlatformError> {
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();
    let mut public_task = public_task;
    let mut admin_task = admin_task;
    let mut runtime_source_task = runtime_source_task;
    let mut binding_backend_task = binding_backend_task;
    let mut maintenance_task = maintenance_task;
    let mut scheduler_task = scheduler_task;
    let mut listener_error = None;
    'wait: loop {
        tokio::select! {
            _ = async {
                match sigterm.as_mut() {
                    Some(s) => {
                        s.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => break 'wait,
            _ = async {
                match sigint.as_mut() {
                    Some(s) => {
                        s.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => break 'wait,
            res = &mut public_task => {
                listener_error = Some(join_listener(res));
                break 'wait;
            }
            res = async {
                match admin_task.as_mut() {
                    Some(task) => task.await,
                    None => std::future::pending().await,
                }
            } => {
                listener_error = Some(join_listener(res));
                break 'wait;
            }
            res = &mut runtime_source_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut binding_backend_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut maintenance_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = async {
                match scheduler_task.as_mut() {
                    Some(task) => task.await,
                    None => std::future::pending().await,
                }
            } => {
                let error = join_scheduler(res);
                tracing::error!(code = error.code().as_str(), "scheduler task stopped");
                let _ = health.set_component(
                    ComponentName::Scheduler,
                    ComponentState::Failed,
                    Some(ReadinessReason::SchedulerUnavailable),
                );
                scheduler_task = None;
            }
        }
    }
    let _ = health.begin_drain();
    let _ = scheduler_shutdown_tx.send(true);
    if let Some(task) = scheduler_task
        && !task.is_finished()
    {
        let _ = task.await;
    }
    supervisor.begin_drain();
    let _ = shutdown_tx.send(true);
    supervisor.shutdown().await;
    if !public_task.is_finished() {
        let _ = public_task.await;
    }
    if let Some(task) = admin_task
        && !task.is_finished()
    {
        let _ = task.await;
    }
    if !runtime_source_task.is_finished() {
        let _ = runtime_source_task.await;
    }
    if !binding_backend_task.is_finished() {
        let _ = binding_backend_task.await;
    }
    if !maintenance_task.is_finished() {
        let _ = maintenance_task.await;
    }
    listener_error
}

fn join_scheduler(res: Result<Result<(), PlatformError>, tokio::task::JoinError>) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(
            ErrorCode::SchedulerUnavailable,
            "scheduler task stopped unexpectedly",
        ),
        Ok(Err(error)) => error,
        Err(_) => PlatformError::new(ErrorCode::SchedulerUnavailable, "scheduler task failed"),
    }
}

async fn run_worker_maintenance(
    storage: &Arc<PlatformStorage>,
    store: &ArtifactStore,
    cache: &Arc<ArtifactCache>,
    pins: &DeploymentPins,
    config: &open_compute_core::WorkersConfig,
    snapshot_pins: &SnapshotPins,
) {
    let now = unix_ms();
    let storage_for_db = storage.clone();
    let batch = config.delete_recovery_batch;
    let policy = config.clone();
    let pass = tokio::task::spawn_blocking(move || {
        let repo = WorkerRepository::new(storage_for_db.db());
        let _ = repo.prune_expired_idempotency(now, batch)?;
        let candidates = repo.retention_candidates(
            now,
            policy.deployment_min_retention_ms,
            policy.retain_ready_deployments,
            policy.retain_rejected_deployments,
            batch,
        )?;
        let references = repo.referenced_artifacts()?;
        Ok::<_, PlatformError>((references, candidates))
    })
    .await;
    let (fallback_references, candidates) = match pass {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            tracing::warn!(
                code = error.code().as_str(),
                "Worker maintenance DB pass failed"
            );
            return;
        }
        Err(_) => {
            tracing::warn!("Worker maintenance DB task failed");
            return;
        }
    };
    for candidate in candidates {
        let storage_for_begin = storage.clone();
        let begin = tokio::task::spawn_blocking(move || {
            WorkerRepository::new(storage_for_begin.db()).begin_deployment_delete(
                candidate.account_id,
                candidate.worker_id,
                candidate.deployment_id,
            )
        })
        .await;
        if !matches!(begin, Ok(Ok(()))) {
            pins.unfence(candidate.deployment_id);
            continue;
        }
        if pins
            .fence_and_wait(
                candidate.deployment_id,
                Duration::from_millis(config.delete_drain_timeout_ms),
            )
            .await
            .is_err()
        {
            // Keep both the SQLite deleting state and memory fence. A future
            // process restart has no surviving in-flight pins and recovers it.
            continue;
        }
        let storage_for_finish = storage.clone();
        let finish = tokio::task::spawn_blocking(move || {
            WorkerRepository::new(storage_for_finish.db()).finalize_deployment_delete(
                candidate.account_id,
                candidate.worker_id,
                candidate.deployment_id,
                RequestId::generate(),
                now,
            )
        })
        .await;
        if matches!(finish, Ok(Ok(()))) {
            pins.retire_fence(candidate.deployment_id);
        } else {
            tracing::warn!("Worker retention finalization failed");
        }
    }
    let storage_for_refs = storage.clone();
    let references = match tokio::task::spawn_blocking(move || {
        WorkerRepository::new(storage_for_refs.db()).referenced_artifacts()
    })
    .await
    {
        Ok(Ok(references)) => references,
        _ => fallback_references,
    };
    let mut retained = HashSet::new();
    for (digest, size) in references {
        if let Ok(reference) = ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size) {
            retained.insert(reference);
        }
    }
    match snapshot_pins.extend_artifacts(&mut retained) {
        Ok(()) => {
            let grace = SystemTime::now()
                .checked_sub(Duration::from_millis(config.artifact_gc_grace_ms))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if let Err(error) = store.gc_unreferenced(&retained, grace).await {
                tracing::warn!(
                    code = error.code().as_str(),
                    "Worker artifact GC pass failed"
                );
            }
        }
        Err(error) => tracing::warn!(
            code = error.code().as_str(),
            "Worker artifact GC skipped because snapshot pins are unavailable"
        ),
    }
    if let Err(error) = cache.evict_if_needed().await {
        tracing::warn!(
            code = error.code().as_str(),
            "Worker cache eviction pass failed"
        );
    }
}

pub(crate) async fn run_kv_maintenance(
    storage: &Arc<PlatformStorage>,
    pins: &ResourcePins,
    config: &open_compute_core::KvConfig,
    metrics: &Arc<MetricsRegistry>,
) {
    let storage = storage.clone();
    let pins = pins.clone();
    let metrics = metrics.clone();
    let batch = usize::try_from(config.max_connections.min(64)).unwrap_or(64);
    let pass = tokio::task::spawn_blocking(move || {
        let account = storage.identity().default_account_id;
        let catalog = open_compute_storage::KvNamespaceRepository::new(storage.db());
        let resources = open_compute_storage::ResourceRepository::new(storage.db());
        let paths = open_compute_storage::KvPaths::open(storage.data_dir().root())?;
        let now = unix_ms();
        for record in catalog.list(account)?.into_iter().take(batch) {
            if record.resource.state != open_compute_core::ResourceState::Ready
                || pins.count(record.resource.id) != 0
            {
                continue;
            }
            let path = paths.resolve_storage_key(
                &record.storage_key,
                record.resource.account_id,
                record.resource.id,
            )?;
            let engine = match open_compute_storage::KvEngine::from_record(path, &record) {
                Ok(engine) => engine,
                Err(error) => {
                    metrics.inc_kv_corruption(2);
                    let code = if error.code() == ErrorCode::KvCorrupt {
                        "KV_CORRUPT"
                    } else {
                        "KV_UNAVAILABLE"
                    };
                    let _ = resources.set_availability(
                        record.resource.account_id,
                        record.resource.id,
                        open_compute_core::ResourceAvailability::Unavailable,
                        Some(code),
                        now,
                    );
                    continue;
                }
            };
            if let Ok(wal_bytes) = engine.wal_bytes() {
                metrics.observe_kv_wal_bytes(wal_bytes);
            }
            metrics.inc_kv_maintenance(KvMaintenance::Gc, engine.gc_expired(now, 256).is_ok());
            if record
                .last_quick_check_ms
                .is_none_or(|last| now.saturating_sub(last) >= 60 * 60 * 1000)
            {
                match engine.quick_check() {
                    Ok(()) => {
                        let _ = catalog.record_quick_check(record.resource.id, now);
                    }
                    Err(error) => {
                        metrics.inc_sqlite_check_failure();
                        metrics.inc_kv_corruption(2);
                        let code = if error.code() == ErrorCode::KvCorrupt {
                            "KV_CORRUPT"
                        } else {
                            "KV_UNAVAILABLE"
                        };
                        let _ = resources.set_availability(
                            record.resource.account_id,
                            record.resource.id,
                            open_compute_core::ResourceAvailability::Unavailable,
                            Some(code),
                            now,
                        );
                    }
                }
            }
            metrics.inc_kv_maintenance(KvMaintenance::Checkpoint, engine.checkpoint(false).is_ok());
        }
        Ok::<_, PlatformError>(())
    })
    .await;
    match pass {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(code = error.code().as_str(), "KV maintenance pass failed");
        }
        Err(_) => tracing::warn!("KV maintenance task failed"),
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

pub(crate) fn join_listener(
    res: Result<Result<(), PlatformError>, tokio::task::JoinError>,
) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(ErrorCode::ConfigInvalid, "health listener failed"),
        Ok(Err(err)) => err,
        Err(_) => PlatformError::new(ErrorCode::ConfigInvalid, "health listener failed"),
    }
}

pub(crate) fn join_runtime_source(
    res: Result<Result<(), PlatformError>, tokio::task::JoinError>,
) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "private RuntimeSource listener stopped unexpectedly",
        ),
        Ok(Err(err)) => err,
        Err(_) => PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "private RuntimeSource listener task failed",
        ),
    }
}

fn record(opts: &RunInner, stage: &'static str) {
    #[cfg(any(test, feature = "test-support"))]
    {
        opts.stages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(stage);
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = (opts, stage);
    }
}

fn remember_bind(opts: &RunInner, addr: Option<SocketAddr>) {
    #[cfg(any(test, feature = "test-support"))]
    {
        *opts
            .last_public_addr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = addr;
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = (opts, addr);
    }
}

fn fail_after(
    opts: &RunInner,
    stage: FailAfterDummy,
    metrics: &MetricsRegistry,
    metric_stage: StartStage,
) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    {
        if opts.fail_after == Some(stage) {
            metrics.inc_start(StartResult::Failure, metric_stage);
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "injected startup failure",
            ));
        }
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = (opts, stage, metrics, metric_stage);
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
type FailAfterDummy = FailAfter;

#[cfg(not(any(test, feature = "test-support")))]
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum FailAfterDummy {
    Config,
    Storage,
    RuntimeVerify,
    S3,
    Cache,
    Compile,
    Listen,
}

/// Bind addresses used after config validation.
pub fn listener_plan(
    server: &open_compute_core::config::ServerConfig,
) -> Result<(SocketAddr, Option<SocketAddr>), PlatformError> {
    let public = server.public_addr()?;
    let admin = server.admin_addr()?;
    match admin {
        Some(addr) if addr != public => Ok((public, Some(addr))),
        _ => Ok((public, None)),
    }
}
