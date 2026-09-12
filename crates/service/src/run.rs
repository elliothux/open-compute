//! Production `run` composition and shutdown.

use crate::D1ApiState;
use crate::ai_search_backend::AiSearchBindingService;
use crate::asset_backend::AssetBindingService;
use crate::binding_backend::{
    bind_binding_backend, serve_binding_backend_with_ai_search_and_snapshot_pins,
};
use crate::cache_backend::CacheBindingService;
use crate::cache_images_http::CacheImagesApiState;
use crate::capabilities::{platform_capabilities, platform_release_metadata};
use crate::config_load::LoadedConfig;
use crate::d1_backend::D1BindingService;
use crate::dashboard::bootstrap_dashboard;
use crate::do_lifecycle::DurableObjectLifecycleService;
use crate::document_parser_backend::DocumentParserBindingService;
use crate::health::HealthCoordinator;
use crate::http::{self, HttpState};
use crate::images_backend::ImageBindingService;
use crate::kv_api::KvApiState;
use crate::kv_backend::SqliteKvBindingExecutor;
use crate::metrics::{
    DoFacetReloadReason, KvMaintenance, MetricsRegistry, SqliteOp, StartResult, StartStage,
};
use crate::object_storage::connect_object_backend;
use crate::observability::ObservabilityService;
use crate::observability_backend::{bind_observability_backend, serve_observability_backend};
use crate::p2_3_promotion::P23PromotionCoordinator;
use crate::queue_api::QueueApiState;
use crate::r2_api::R2ApiState;
use crate::r2_backend::R2BindingService;
use crate::r2_maintenance::R2Maintenance;
use crate::runtime_bridge::{WorkerdTransport, bind_runtime_source, serve_runtime_source};
use crate::runtime_generation::RuntimeGenerationResources;
use crate::scheduler::SchedulerService;
use crate::search_api::SearchApiState;
use crate::service_invocations::ServiceInvocationRegistry;
use crate::snapshot_pins::{SnapshotPins, load_snapshot_pins};
use crate::workers_http::WorkerApiState;
pub(super) mod p1;
mod storage_bootstrap;
use open_compute_artifacts::{
    ARTIFACT_KEY_VERSION, AiSearchObjectStore, ArtifactCache, ArtifactRef, ArtifactStore,
    R2ObjectStore, preflight_object_storage, preflight_r2,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, PlatformError, ReadinessReason, Redactor, RequestId,
    StartupId, SystemSchedulerClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions,
};
use open_compute_storage::{
    CacheManager, DurableObjectRepository, ObservabilityStore, PlatformStorage, WorkerRepository,
};
use open_compute_workers::{BundleLimits, ResourcePins, RuntimeSource, VersionPins};
use p1::{
    load_offline_metrics_receipts, refresh_metrics as refresh_p1_metrics,
    require_current_serving_schema, update_operations_health,
};
mod execution;
mod startup;
use execution::run_prepared;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{RwLock, mpsc, watch};

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
    /// After object-storage preflight.
    ObjectStorage,
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
    /// Explicit registry authority used by isolated test processes.
    pub instance_registry: Option<crate::instance_registry::InstanceRegistry>,
}

#[derive(Clone, Debug, Default)]
struct RunInner {
    #[cfg(any(test, feature = "test-support"))]
    fail_after: Option<FailAfter>,
    #[cfg(any(test, feature = "test-support"))]
    stages: Arc<Mutex<Vec<&'static str>>>,
    #[cfg(any(test, feature = "test-support"))]
    last_public_addr: Arc<Mutex<Option<SocketAddr>>>,
    instance_registry: Option<crate::instance_registry::InstanceRegistry>,
}

/// Run the platform until SIGINT/SIGTERM.
pub async fn run_platform(loaded: LoadedConfig) -> Result<(), PlatformError> {
    Box::pin(run_inner(loaded, RunInner::default())).await
}

/// Run with explicit test-support options.
#[cfg(any(test, feature = "test-support"))]
pub async fn run_platform_with(
    loaded: LoadedConfig,
    opts: RunOptions,
) -> Result<(), PlatformError> {
    Box::pin(run_inner(
        loaded,
        RunInner {
            fail_after: opts.fail_after,
            stages: opts.stages,
            last_public_addr: opts.last_public_addr,
            instance_registry: opts.instance_registry,
        },
    ))
    .await
}

async fn run_inner(loaded: LoadedConfig, opts: RunInner) -> Result<(), PlatformError> {
    let prepared = startup::prepare(loaded, opts).await?;
    Box::pin(run_prepared(prepared)).await
}

fn control_identity(
    config_path: &std::path::Path,
    test_registry: Option<&crate::instance_registry::InstanceRegistry>,
) -> Result<
    (
        open_compute_core::InstanceId,
        crate::instance_registry::ServiceScope,
    ),
    PlatformError,
> {
    if let Some(registry) = test_registry {
        return control_identity_from_records(config_path, registry.list()?);
    }
    let system_registry = crate::instance_registry::InstanceRegistry::with_roots(
        std::path::PathBuf::from(crate::instance_registry::SYSTEM_REGISTRY_ROOT),
        std::path::PathBuf::new(),
    );
    let mut records = system_registry.list_scope(crate::instance_registry::ServiceScope::System)?;
    if let Ok(registry) = crate::instance_registry::InstanceRegistry::production() {
        records.extend(registry.list_scope(crate::instance_registry::ServiceScope::User)?);
    }
    control_identity_from_records(config_path, records)
}

fn control_identity_from_records(
    config_path: &std::path::Path,
    records: Vec<crate::instance_registry::InstanceRecord>,
) -> Result<
    (
        open_compute_core::InstanceId,
        crate::instance_registry::ServiceScope,
    ),
    PlatformError,
> {
    let mut matching = records
        .into_iter()
        .filter(|record| record.config_path() == config_path);
    if let Some(record) = matching.next() {
        if matching.next().is_some() {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "multiple instance registrations reference the active configuration",
            ));
        }
        return Ok((record.instance_id()?, record.service_scope));
    }
    let id = open_compute_core::InstanceId::from_canonical_config_path(config_path)?;
    let scope = if config_path.starts_with("/etc/open-compute/") {
        crate::instance_registry::ServiceScope::System
    } else {
        crate::instance_registry::ServiceScope::User
    };
    Ok((id, scope))
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
    metrics.set_do_storage_watermark(watermark);
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

pub(crate) fn update_local_object_storage_health(
    backend: &open_compute_artifacts::ObjectBackend,
    config: &open_compute_core::ObjectStorageConfig,
    health: &HealthCoordinator,
) -> Result<(), PlatformError> {
    let open_compute_core::ObjectStorageConfig::Local(local) = config else {
        return Ok(());
    };
    let (state, reason) = match backend.available_bytes() {
        Ok(Some(available)) if available < local.free_space_hard_bytes => {
            (ComponentState::Degraded, ReadinessReason::DiskHardLimit)
        }
        Ok(Some(available)) if available < local.free_space_soft_bytes => {
            (ComponentState::Degraded, ReadinessReason::DiskSoftLimit)
        }
        Ok(Some(_)) => (ComponentState::Healthy, ReadinessReason::Ready),
        Ok(None) | Err(_) => (
            ComponentState::Degraded,
            ReadinessReason::ObjectStorageDegraded,
        ),
    };
    health.set_component(ComponentName::ObjectStorage, state, Some(reason))
}

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
async fn wait_signals_and_servers(
    health: &HealthCoordinator,
    supervisor: &WorkerdSupervisor,
    shutdown_tx: watch::Sender<bool>,
    scheduler_shutdown_tx: watch::Sender<bool>,
    public_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    admin_task: Option<tokio::task::JoinHandle<Result<(), PlatformError>>>,
    runtime_source_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    binding_backend_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    observability_backend_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    control_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    maintenance_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    scheduler_task: Option<tokio::task::JoinHandle<Result<(), PlatformError>>>,
) -> Option<PlatformError> {
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();
    let mut public_task = public_task;
    let mut admin_task = admin_task;
    let mut runtime_source_task = runtime_source_task;
    let mut binding_backend_task = binding_backend_task;
    let mut observability_backend_task = observability_backend_task;
    let mut control_task = control_task;
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
            res = &mut observability_backend_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut control_task => {
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
    if !control_task.is_finished() {
        let _ = control_task.await;
    }
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
    if !observability_backend_task.is_finished() {
        let _ = observability_backend_task.await;
    }
    if !maintenance_task.is_finished() {
        let _ = maintenance_task.await;
    }
    listener_error
}

pub(crate) fn join_scheduler(
    res: Result<Result<(), PlatformError>, tokio::task::JoinError>,
) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(
            ErrorCode::SchedulerUnavailable,
            "scheduler task stopped unexpectedly",
        ),
        Ok(Err(error)) => error,
        Err(_) => PlatformError::new(ErrorCode::SchedulerUnavailable, "scheduler task failed"),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
async fn run_worker_maintenance(
    storage: &Arc<PlatformStorage>,
    store: &ArtifactStore,
    cache: &Arc<ArtifactCache>,
    response_cache: &Arc<CacheManager>,
    pins: &VersionPins,
    config: &open_compute_core::WorkersConfig,
    snapshot_pins: &SnapshotPins,
    metrics: &Arc<MetricsRegistry>,
) {
    let now = open_compute_core::wall_time_ms();
    let storage_for_db = storage.clone();
    let batch = config.delete_recovery_batch;
    let policy = config.clone();
    let pass = tokio::task::spawn_blocking(move || {
        let repo = WorkerRepository::new(storage_for_db.db());
        let _ = repo.prune_expired_idempotency(now, batch)?;
        let candidates = repo.retention_candidates(
            now,
            policy.version_min_retention_ms,
            policy.retain_ready_versions,
            policy.retain_rejected_versions,
            batch,
        )?;
        Ok::<_, PlatformError>(candidates)
    })
    .await;
    let candidates = match pass {
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
            WorkerRepository::new(storage_for_begin.db()).begin_version_delete(
                candidate.account_id,
                candidate.worker_id,
                candidate.version_id,
            )
        })
        .await;
        if !matches!(begin, Ok(Ok(()))) {
            pins.unfence(candidate.version_id);
            continue;
        }
        if pins
            .fence_and_wait(
                candidate.version_id,
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
            WorkerRepository::new(storage_for_finish.db()).finalize_version_delete(
                candidate.account_id,
                candidate.worker_id,
                candidate.version_id,
                RequestId::generate(),
                now,
            )
        })
        .await;
        if matches!(finish, Ok(Ok(()))) {
            pins.retire_fence(candidate.version_id);
        } else {
            tracing::warn!("Worker retention finalization failed");
        }
    }
    if let Err(error) = gc_worker_artifacts(
        storage,
        store,
        config,
        snapshot_pins,
        Some(response_cache.clone()),
    )
    .await
    {
        tracing::warn!(
            code = error.code().as_str(),
            "Worker artifact GC pass failed"
        );
    }
    if let Err(error) = cache.evict_if_needed().await {
        tracing::warn!(
            code = error.code().as_str(),
            "Worker cache eviction pass failed"
        );
    }
    match response_cache.stats(open_compute_core::wall_time_ms()) {
        Ok(stats) => metrics.set_response_cache_stats(stats),
        Err(error) => tracing::warn!(
            code = error.code().as_str(),
            "Response cache metrics inspection failed"
        ),
    }
}

pub(crate) async fn gc_worker_artifacts(
    storage: &Arc<PlatformStorage>,
    store: &ArtifactStore,
    config: &open_compute_core::WorkersConfig,
    snapshot_pins: &SnapshotPins,
    response_cache: Option<Arc<CacheManager>>,
) -> Result<u64, PlatformError> {
    let gc_fence = store.fence_version_gc().await;
    let storage_for_refs = storage.clone();
    let references = match tokio::task::spawn_blocking(move || {
        WorkerRepository::new(storage_for_refs.db()).referenced_artifacts()
    })
    .await
    {
        Ok(Ok(references)) => references,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::ArtifactUnavailable,
                "artifact reference inspection failed",
            ));
        }
    };
    let mut retained = HashSet::new();
    for (digest, size) in references {
        if let Ok(reference) = ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size) {
            retained.insert(reference);
        }
    }
    if let Some(response_cache) = response_cache {
        let cache_references =
            match tokio::task::spawn_blocking(move || response_cache.referenced_bodies()).await {
                Ok(Ok(references)) => references,
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(PlatformError::new(
                        ErrorCode::CacheUnavailable,
                        "cache reference inspection failed",
                    ));
                }
            };
        for body in cache_references {
            match ArtifactRef::new(ARTIFACT_KEY_VERSION, &body.sha256, body.size) {
                Ok(reference) => {
                    retained.insert(reference);
                }
                Err(error) => return Err(error),
            }
        }
    }
    snapshot_pins.extend_artifacts(&mut retained)?;
    let grace = SystemTime::now()
        .checked_sub(Duration::from_millis(config.artifact_gc_grace_ms))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    store.gc_unreferenced(&gc_fence, &retained, grace).await
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
        let now = open_compute_core::wall_time_ms();
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

#[cfg(any(test, feature = "test-support"))]
fn fail_after(
    opts: &RunInner,
    stage: FailAfter,
    metrics: &MetricsRegistry,
    metric_stage: StartStage,
) -> Result<(), PlatformError> {
    if opts.fail_after == Some(stage) {
        metrics.inc_start(StartResult::Failure, metric_stage);
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "injected startup failure",
        ));
    }
    Ok(())
}

async fn wait_for_supervisor_running(supervisor: &WorkerdSupervisor, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut watch_rx = supervisor.subscribe();
    loop {
        if watch_rx.borrow().state == SupervisorState::Running {
            return true;
        }
        if watch_rx.borrow().state == SupervisorState::Failed {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let wait = remaining.min(Duration::from_millis(250));
        if tokio::time::timeout(wait, watch_rx.changed())
            .await
            .is_err()
        {
            continue;
        }
    }
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
