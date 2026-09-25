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
use crate::host_extension_broker::HostExtensionBroker;
use crate::http::{self, HttpState};
use crate::images_backend::ImageBindingService;
use crate::instance_registry::{
    DaemonArtifactsConfig, DaemonMetricsConfig, InstanceRecord, InstanceRegistry, ServiceScope,
};
use crate::kv_api::KvApiState;
use crate::kv_backend::SqliteKvBindingExecutor;
use crate::local_extensions::LocalExtensionRegistry;
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
    ComponentName, ComponentState, DaemonGatewayConfig, DaemonServerConfig, ErrorCode, InstanceId,
    PlatformError, ReadinessReason, Redactor, RequestId, StartupId, SystemSchedulerClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry,
    HostExtensionBrokerRegistry, OsJitter, PlatformReleaseMeta, StaticConfigCompiler,
    SupervisorState, WorkerdSupervisor, WorkerdSupervisorOptions,
};
use open_compute_storage::{
    CacheManager, DurableObjectRepository, ObservabilityStore, PlatformStorage, WorkerRepository,
};
use open_compute_workers::{BundleLimits, ResourcePins, RuntimeSource, VersionPins};
use p1::{
    load_offline_metrics_receipts, refresh_metrics as refresh_p1_metrics, update_operations_health,
};
mod cache_clean;
pub(crate) mod daemon_control;
mod daemon_lifecycle;
mod daemon_lock;
pub(crate) use daemon_lock::DaemonLock;
mod execution;
mod gateway;
mod maintenance;
mod startup;
mod wait;
pub(crate) use cache_clean::clean_global_cache;
pub(crate) use daemon_lifecycle::OfflineInstanceOwner;
pub(crate) use daemon_lifecycle::validate_registered_tokens;
use daemon_lifecycle::{DaemonPlan, handle_daemon_command, spawn_runtime};
use execution::run_prepared;
use maintenance::run_worker_maintenance;
pub(crate) use maintenance::{gc_worker_artifacts, run_kv_maintenance};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{RwLock, Semaphore, mpsc, watch};
pub(crate) use wait::join_listener;
#[cfg(test)]
pub(crate) use wait::{join_runtime_source, join_scheduler};
use wait::{wait_for_supervisor_running, wait_instance_and_servers};

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
    /// Shared listener addresses supplied by the isolated test OCD scope.
    pub daemon_server: DaemonServerConfig,
    /// Shared Gateway settings supplied by the isolated test OCD scope.
    pub daemon_gateway: Option<DaemonGatewayConfig>,
    /// Fail after this stage.
    pub fail_after: Option<FailAfter>,
    /// Recorded stage names.
    pub stages: Arc<Mutex<Vec<&'static str>>>,
    /// Last bound public address, if listeners were acquired.
    pub last_public_addr: Arc<Mutex<Option<SocketAddr>>>,
    /// Explicit registry authority used by isolated test processes.
    pub instance_registry: Option<InstanceRegistry>,
}

#[derive(Clone, Default)]
struct RunInner {
    daemon_server: DaemonServerConfig,
    daemon_gateway: Option<DaemonGatewayConfig>,
    daemon_artifacts: DaemonArtifactsConfig,
    daemon_metrics: DaemonMetricsConfig,
    shared_package: Option<open_compute_runtime::RuntimePackage>,
    artifact_requests: Option<Arc<Semaphore>>,
    scope: Option<ServiceScope>,
    shutdown: Option<watch::Receiver<bool>>,
    daemon_api: Option<daemon_control::DaemonApi>,
    dashboard_auth: Option<Arc<crate::dashboard_auth::DashboardAuth>>,
    shared_routes: Option<http::SharedRoutes>,
    shared_public_addr: Option<SocketAddr>,
    shared_admin_addr: Option<SocketAddr>,
    gateway_pids: Option<(
        Arc<std::sync::atomic::AtomicI32>,
        Arc<std::sync::atomic::AtomicI32>,
    )>,
    #[cfg(any(test, feature = "test-support"))]
    fail_after: Option<FailAfter>,
    #[cfg(any(test, feature = "test-support"))]
    stages: Arc<Mutex<Vec<&'static str>>>,
    #[cfg(any(test, feature = "test-support"))]
    last_public_addr: Arc<Mutex<Option<SocketAddr>>>,
    #[cfg(any(test, feature = "test-support"))]
    stop_on_instance_error: bool,
    instance_registry: Option<InstanceRegistry>,
}

impl RunInner {
    fn artifact_requests(&self) -> Result<Arc<Semaphore>, PlatformError> {
        self.artifact_requests.clone().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "shared Artifacts request limit is missing",
            )
        })
    }
}

/// Run the selected OCD scope until SIGINT/SIGTERM.
pub async fn run_platform(
    scope: ServiceScope,
    registry: InstanceRegistry,
) -> Result<(), PlatformError> {
    validate_scope_runtime_owner(registry.root_for(scope))?;
    let _daemon_lock = DaemonLock::acquire(registry.root_for(scope))?;
    crate::setup::recover_setup_staging(registry.root_for(scope))?;
    let records = registry.list_scope(scope)?;
    let daemon_server = registry.server_config(scope)?;
    let daemon_gateway = registry.gateway_config(scope)?;
    let daemon_artifacts = registry.artifacts_config(scope)?;
    let daemon_metrics = registry.metrics_config(scope)?;
    let credentials = validate_registered_tokens(&records, &daemon_server)?;
    let loaded_instances = records
        .iter()
        .filter(|record| record.autostart)
        .map(|record| {
            crate::config_load::load_platform_config_from(
                record.config_path(),
                std::path::Path::new("/"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = registry.root_for(scope).to_path_buf();
    let cache_dir = root.join("cache");
    open_compute_storage::ensure_dir_secure(&cache_dir)?;
    let shared_package = tokio::task::spawn_blocking(move || {
        open_compute_runtime::materialize_embedded_runtime(&cache_dir)
    })
    .await
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "shared runtime materialization failed",
        )
    })??;
    let (_, caddy_digest) = shared_package.caddy()?;
    crate::task_workspace::recover(
        &root.join("tmp"),
        &["caddy-tool-", "caddy-validate-"],
        "tool.lease",
        caddy_digest,
    )?;
    let plan = DaemonPlan {
        manifest_digest: crate::instance_registry::manifest_digest(&root)?,
        root,
        scope,
        registry: registry.clone(),
        records,
        credentials,
    };
    let opts = RunInner {
        daemon_server,
        daemon_gateway,
        daemon_artifacts,
        daemon_metrics,
        shared_package: Some(shared_package),
        scope: Some(scope),
        instance_registry: Some(registry),
        ..RunInner::default()
    };
    run_until_signal(loaded_instances, opts, Some(plan)).await
}

fn validate_scope_runtime_owner(root: &std::path::Path) -> Result<(), PlatformError> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::getuid().as_raw();
    let owner = std::fs::symlink_metadata(root).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "OCD_DIR is unavailable; run scoped setup before starting the daemon",
        )
    })?;
    if uid == 0 || !owner.is_dir() || owner.file_type().is_symlink() || owner.uid() != uid {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "OCD daemon must run as the non-root owner of OCD_DIR",
        ));
    }
    Ok(())
}

/// Run with explicit test-support options.
#[cfg(any(test, feature = "test-support"))]
pub async fn run_platform_with(
    loaded: LoadedConfig,
    opts: RunOptions,
) -> Result<(), PlatformError> {
    let scope = opts
        .instance_registry
        .as_ref()
        .map(|registry| registry.scope_for_config(&loaded.path))
        .transpose()?
        .unwrap_or(ServiceScope::User);
    let _daemon_lock = opts
        .instance_registry
        .as_ref()
        .map(|registry| DaemonLock::acquire(registry.root_for(scope)))
        .transpose()?;
    let daemon_artifacts = opts
        .instance_registry
        .as_ref()
        .map(|registry| registry.artifacts_config(scope))
        .transpose()?
        .unwrap_or_default();
    let daemon_gateway = opts
        .instance_registry
        .as_ref()
        .map(|registry| registry.gateway_config(scope))
        .transpose()?
        .flatten()
        .or(opts.daemon_gateway);
    let daemon_metrics = opts
        .instance_registry
        .as_ref()
        .map(|registry| registry.metrics_config(scope))
        .transpose()?
        .unwrap_or_default();
    run_until_signal(
        vec![loaded],
        RunInner {
            daemon_server: opts.daemon_server,
            daemon_artifacts,
            daemon_gateway,
            daemon_metrics,
            scope: Some(scope),
            fail_after: opts.fail_after,
            stages: opts.stages,
            last_public_addr: opts.last_public_addr,
            instance_registry: opts.instance_registry,
            stop_on_instance_error: true,
            ..RunInner::default()
        },
        None,
    )
    .await
}

async fn run_until_signal(
    loaded_instances: Vec<LoadedConfig>,
    mut opts: RunInner,
    mut plan: Option<DaemonPlan>,
) -> Result<(), PlatformError> {
    opts.artifact_requests = Some(Arc::new(Semaphore::new(
        opts.daemon_artifacts.max_concurrent_requests as usize,
    )));
    let (public_addr, admin_addr) = listener_plan(&opts.daemon_server)?;
    let public_listener = http::bind(public_addr).await?;
    let public_bound = public_listener.local_addr().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "failed to inspect public listener",
        )
    })?;
    let admin_listener = match admin_addr {
        Some(address) => Some(http::bind(address).await?),
        None => None,
    };
    let admin_bound = admin_listener
        .as_ref()
        .map(tokio::net::TcpListener::local_addr)
        .transpose()
        .map_err(|_| {
            PlatformError::new(ErrorCode::ConfigInvalid, "failed to inspect admin listener")
        })?;
    let (daemon_api, mut daemon_commands) = match plan.as_mut() {
        Some(plan) => {
            let token = crate::auth::resolve_admin_auth(&opts.daemon_server.admin_auth)?;
            let (api, commands) = daemon_control::DaemonApi::channel(
                &plan.records,
                std::mem::take(&mut plan.credentials),
                token,
            )?;
            (Some(api), Some(commands))
        }
        None => (None, None),
    };
    let daemon_socket = plan
        .as_ref()
        .map(|plan| daemon_control::DaemonSocket::bind(&plan.root))
        .transpose()?;
    let routes = http::SharedRoutes::new(daemon_api.clone(), opts.daemon_metrics.max_series);
    opts.dashboard_auth = Some(routes.dashboard_auth());
    opts.shared_public_addr = Some(public_bound);
    opts.shared_admin_addr = admin_bound;
    opts.daemon_api = daemon_api.clone();
    opts.shared_routes = Some(routes.clone());
    remember_bind(&opts, Some(public_bound));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut listeners = tokio::task::JoinSet::new();
    let gateway = gateway::start(
        &loaded_instances,
        &opts,
        plan.as_ref(),
        &routes,
        shutdown_rx.clone(),
        &mut listeners,
    )
    .await?;
    opts.gateway_pids = gateway.as_ref().map(gateway::GatewayOwner::pids);
    if let (Some(api), Some(owner)) = (daemon_api.as_ref(), gateway.as_ref()) {
        api.set_gateway(owner.control.clone())?;
    }
    if let (Some(socket), Some(api)) = (daemon_socket, daemon_api.clone()) {
        listeners.spawn(socket.serve(api, shutdown_rx.clone()));
    }
    let mut public_shutdown = shutdown_rx.clone();
    listeners.spawn(async move {
        http::serve_until(
            public_listener,
            routes.router(admin_bound.is_none(), public_bound.port()),
            async move {
                let _ = public_shutdown.changed().await;
            },
        )
        .await
    });
    if let Some(listener) = admin_listener {
        let admin_port = listener
            .local_addr()
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to inspect admin listener")
            })?
            .port();
        let routes = opts
            .shared_routes
            .as_ref()
            .ok_or_else(|| {
                PlatformError::new(ErrorCode::ConfigInvalid, "shared routes are missing")
            })?
            .clone();
        let mut admin_shutdown = shutdown_rx;
        listeners.spawn(async move {
            http::serve_until(listener, routes.router(true, admin_port), async move {
                let _ = admin_shutdown.changed().await;
            })
            .await
        });
    }
    #[cfg(any(test, feature = "test-support"))]
    let stop_on_instance_error = opts.stop_on_instance_error;
    #[cfg(not(any(test, feature = "test-support")))]
    let stop_on_instance_error = false;
    let mut instances = tokio::task::JoinSet::new();
    let mut active = HashMap::<InstanceId, watch::Sender<bool>>::new();
    let mut instance_shutdowns = Vec::new();
    for loaded in loaded_instances {
        let id = plan
            .as_ref()
            .and_then(|plan| {
                plan.records
                    .iter()
                    .find(|record| record.config_path() == loaded.path)
            })
            .map(InstanceRecord::instance_id)
            .transpose()?;
        spawn_runtime(
            loaded,
            id,
            &opts,
            &mut instances,
            &mut active,
            &mut instance_shutdowns,
            daemon_api.as_ref(),
        )?;
    }
    let mut sigterm = signal(SignalKind::terminate()).ok();
    let mut sigint = signal(SignalKind::interrupt()).ok();
    let mut result = Ok(());
    loop {
        tokio::select! {
            _ = async {
                match sigterm.as_mut() {
                    Some(signal) => { signal.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => break,
            _ = async {
                match sigint.as_mut() {
                    Some(signal) => { signal.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => break,
            listener = listeners.join_next() => {
                result = Err(listener.map_or_else(
                    || PlatformError::new(ErrorCode::ConfigInvalid, "shared listener stopped"),
                    join_listener,
                ));
                break;
            },
            command = async {
                match daemon_commands.as_mut() {
                    Some(commands) => commands.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(command) = command {
                    let response = match (&command.request, daemon_api.as_ref()) {
                        (daemon_control::ControlRequest::CleanGlobalCache { dry_run }, Some(api)) => {
                            plan.as_ref()
                                .ok_or_else(|| PlatformError::new(
                                    ErrorCode::RuntimeUnavailable,
                                    "daemon lifecycle authority is unavailable",
                                ))
                                .and_then(|plan| cache_clean::clean_global_online(
                                    plan,
                                    &active,
                                    api,
                                    gateway.as_ref(),
                                    *dry_run,
                                ))
                                .map(Some)
                        }
                        (daemon_control::ControlRequest::CleanCache { instance_id, dry_run }, Some(api)) => {
                            if active.contains_key(instance_id) {
                                api.clean_cache(instance_id, *dry_run).await.map(Some)
                            } else if let Some(plan) = plan.as_ref() {
                                daemon_lifecycle::clean_stopped_cache(plan, instance_id, *dry_run)
                                    .await
                                    .map(Some)
                            } else {
                                Err(PlatformError::new(
                                    ErrorCode::RuntimeUnavailable,
                                    "daemon lifecycle authority is unavailable",
                                ))
                            }
                        }
                        _ => handle_daemon_command(
                            &command,
                            plan.as_mut(),
                            gateway.as_ref(),
                            &opts,
                            &mut instances,
                            &mut active,
                            &mut instance_shutdowns,
                            daemon_api.as_ref(),
                        ).map(|()| None),
                    };
                    let _ = command.reply.send(response);
                }
            },
            instance = instances.join_next(), if !instances.is_empty() => {
                let (id, outcome) = match instance {
                    Some(Ok(outcome)) => outcome,
                    Some(Err(_)) | None => (None, Err(PlatformError::new(
                        ErrorCode::RuntimeUnavailable, "instance runtime task failed",
                    ))),
                };
                if let Some(id) = id {
                    active.remove(&id);
                    if let Some(api) = &daemon_api {
                        let _ = api.mark(
                            &id,
                            if outcome.is_ok() { "stopped" } else { "failed" },
                            outcome.as_ref().err().map(PlatformError::code),
                        );
                    }
                }
                if let Err(error) = outcome {
                    tracing::error!(code = error.code().as_str(), "instance runtime stopped");
                    if stop_on_instance_error {
                        result = Err(error);
                        break;
                    }
                } else if stop_on_instance_error {
                    break;
                }
            },
        }
    }
    let _ = shutdown_tx.send(true);
    for instance_shutdown in &instance_shutdowns {
        let _ = instance_shutdown.send(true);
    }
    while let Some(instance) = instances.join_next().await {
        if let Err(error) = instance
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeUnavailable,
                    "instance runtime task failed",
                )
            })
            .and_then(|(_, result)| result)
            && result.is_ok()
            && stop_on_instance_error
        {
            result = Err(error);
        }
    }
    while let Some(listener) = listeners.join_next().await {
        if let Err(error) = listener
            .map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "shared listener task failed")
            })
            .and_then(|result| result)
            && result.is_ok()
        {
            result = Err(error);
        }
    }
    result
}

async fn run_inner(loaded: LoadedConfig, opts: RunInner) -> Result<(), PlatformError> {
    let prepared = startup::prepare(loaded, opts).await?;
    Box::pin(run_prepared(prepared)).await
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

/// Bind addresses used after config validation.
pub fn listener_plan(
    server: &DaemonServerConfig,
) -> Result<(SocketAddr, Option<SocketAddr>), PlatformError> {
    let public = server.public_addr()?;
    let admin = server.admin_addr()?;
    match admin {
        Some(addr) if addr != public => Ok((public, Some(addr))),
        _ => Ok((public, None)),
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
