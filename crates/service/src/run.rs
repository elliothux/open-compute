//! Production `run` composition and shutdown.

use crate::config_load::LoadedConfig;
use crate::health::HealthCoordinator;
use crate::http::{self, HttpState, SanitizedSupervisor};
use crate::metrics::{MetricsRegistry, SqliteOp, StartResult, StartStage};
use crate::runtime_bridge::{WorkerdTransport, bind_runtime_source, serve_runtime_source};
use crate::workers_http::WorkerApiState;
use open_compute_artifacts::{
    ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore, S3ArtifactClient,
    preflight_s3, resolve_s3_credentials,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, PlatformError, ReadinessReason, Redactor, RequestId,
    StartupId,
};
use open_compute_runtime::{
    ExternalServiceAddress, GenerationAuthRegistry, OsJitter, PlatformReleaseMeta,
    StaticConfigCompiler, WorkerdSupervisor, WorkerdSupervisorOptions,
    verify_runtime_binary_with_staging_lease,
};
use open_compute_storage::PlatformStorage;
use open_compute_storage::WorkerRepository;
use open_compute_workers::{BundleLimits, DeploymentPins, RuntimeSource};
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
        move || PlatformStorage::bootstrap(&cfg, &clock)
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
    let maintenance_now = unix_ms();
    WorkerRepository::new(storage.db())
        .prune_expired_idempotency(maintenance_now, loaded.config.workers.delete_recovery_batch)?;
    WorkerRepository::new(storage.db()).recover_deleting_deployments(
        RequestId::generate(),
        maintenance_now,
        loaded.config.workers.delete_recovery_batch,
    )?;
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
        loaded.config.cache.max_artifact_bytes,
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
    let runtime_source_listener = bind_runtime_source().await?;
    let runtime_source_addr = runtime_source_listener.local_addr().map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "failed to inspect private RuntimeSource listener",
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
    .with_generation_auth(generation_auth.clone());
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
    let bundle_limits = BundleLimits {
        max_artifact_bytes: usize::try_from(loaded.config.workers.max_bundle_bytes).map_err(
            |_| PlatformError::new(ErrorCode::LimitInvalid, "Worker bundle limit is invalid"),
        )?,
        ..BundleLimits::default()
    };
    let deployment_pins = DeploymentPins::new();
    let supervisor_for_http = supervisor_handle.clone();
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
    .with_worker_api(WorkerApiState::new(
        storage.clone(),
        store.clone(),
        transport.clone(),
        deployment_pins.clone(),
        bundle_limits,
        Duration::from_millis(loaded.config.workers.delete_drain_timeout_ms),
    ));

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
    let mut shutdown_maintenance = shutdown_rx.clone();
    let maintenance_storage = storage.clone();
    let maintenance_store = store.clone();
    let maintenance_cache = cache.clone();
    let maintenance_config = loaded.config.workers.clone();
    let maintenance_pins = deployment_pins;
    let maintenance_task = tokio::spawn(async move {
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
                    ).await;
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

    let supervisor = Arc::new(WorkerdSupervisor::new_with_external_services(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: loaded.config.runtime.clone(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor,
            lease_path: Some(runtime_lease_path),
        },
        vec![ExternalServiceAddress::loopback(
            "runtime-source",
            runtime_source_addr,
        )?],
        Some(generation_auth),
    ));
    *supervisor_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(supervisor.clone());
    supervisor.start();
    record(&opts, "supervisor");
    metrics.inc_start(StartResult::Success, StartStage::Supervisor);

    let mut watch_rx = supervisor.subscribe();
    let health_watch = health.clone();
    let metrics_watch = metrics.clone();
    tokio::spawn(async move {
        loop {
            let snap = watch_rx.borrow().clone();
            metrics_watch.observe_supervisor(&snap);
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
        public_task,
        admin_task,
        runtime_source_task,
        maintenance_task,
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

async fn wait_signals_and_servers(
    health: &HealthCoordinator,
    supervisor: &WorkerdSupervisor,
    shutdown_tx: watch::Sender<bool>,
    public_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    admin_task: Option<tokio::task::JoinHandle<Result<(), PlatformError>>>,
    runtime_source_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    maintenance_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
) -> Option<PlatformError> {
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();
    let mut public_task = public_task;
    let mut admin_task = admin_task;
    let mut runtime_source_task = runtime_source_task;
    let mut maintenance_task = maintenance_task;
    let mut listener_error = None;
    tokio::select! {
        _ = async {
            match sigterm.as_mut() {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        } => {}
        _ = async {
            match sigint.as_mut() {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        } => {}
        res = &mut public_task => {
            listener_error = Some(join_listener(res));
        }
        res = async {
            match admin_task.as_mut() {
                Some(task) => task.await,
                None => std::future::pending().await,
            }
        } => {
            listener_error = Some(join_listener(res));
        }
        res = &mut runtime_source_task => {
            listener_error = Some(join_runtime_source(res));
        }
        res = &mut maintenance_task => {
            listener_error = Some(join_runtime_source(res));
        }
    }
    let _ = health.begin_drain();
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
    if !maintenance_task.is_finished() {
        let _ = maintenance_task.await;
    }
    listener_error
}

async fn run_worker_maintenance(
    storage: &Arc<PlatformStorage>,
    store: &ArtifactStore,
    cache: &Arc<ArtifactCache>,
    pins: &DeploymentPins,
    config: &open_compute_core::WorkersConfig,
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
    let grace = SystemTime::now()
        .checked_sub(Duration::from_millis(config.artifact_gc_grace_ms))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if let Err(error) = store.gc_unreferenced(&retained, grace).await {
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
