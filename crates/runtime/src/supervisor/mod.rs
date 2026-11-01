//! workerd process supervisor: spawn, ready, restart, drain, reap.

mod backoff;
mod control;
mod host_extensions;
pub(crate) mod logs;
pub(crate) mod owner;
mod probe;
mod python_preparation;
mod rotation;
mod spawn;
mod state;
mod watchdog;

use crate::compile::{CompileRequest, CompiledConfig, PlatformReleaseMeta, compile_static_config};
use crate::lease::{clear_lease, recover_orphans, write_lease};
use crate::process::{assert_reaped, wait_reaped};
use crate::verify::VerifiedRuntime;
use backoff::{RestartBudget, backoff_delay};
use open_compute_core::clock::Clock;
use open_compute_core::config::{DurableObjectsConfig, RuntimeConfig};
use open_compute_core::error::ReadinessReason;
use open_compute_core::ids::StartupId;
use open_compute_core::{ErrorCode, PlatformError, Redactor, SystemClock};
use owner::{OwnerCompletion, OwnerRegistry};
use spawn::{LiveRuntime, SpawnFailure, SpawnRequest, spawn_child, wait_ready};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot, watch};

#[cfg(any(test, feature = "test-support"))]
pub use backoff::SequenceJitter;
pub use backoff::{JitterRng, OsJitter};
pub use host_extensions::HostExtensionBrokerRegistry;
#[cfg(any(test, feature = "test-support"))]
pub use logs::set_reader_fail_point;
#[cfg(any(test, feature = "test-support"))]
pub use owner::{take_owner_wait_count, take_reader_join_errors};
pub use probe::{READY_PATH, probe_ready};
pub use python_preparation::{PythonPreparationOptions, PythonPreparationProcess};
pub use spawn::serve_argv;
#[cfg(any(test, feature = "test-support"))]
pub use spawn::{
    blocking_spawn_is_waiting, clear_blocking_spawn_hold, hold_blocking_spawn, last_spawned_pid,
    release_blocking_spawn, set_spawn_fail_point,
};
pub use state::{SanitizedExit, SupervisorSnapshot, SupervisorState};
pub use watchdog::{LIVE_PATH, RuntimeFailureEvidence, WatchdogConfig};

/// Bounded redacted child diagnostics. Not part of ordinary snapshot/status/Debug.
#[derive(Clone, Debug, Default)]
pub struct ProcessDiagnostics {
    /// Redacted stdout tail for trusted helpers; empty for workerd tenant output.
    pub stdout_tail: String,
    /// Redacted stderr tail for trusted helpers; empty for workerd native tracebacks.
    pub stderr_tail: String,
    /// Child exit code if it exited.
    pub exit_code: Option<i32>,
    /// POSIX signal if terminated by signal.
    pub signal: Option<i32>,
    /// True if a stdout/stderr reader failed or panicked.
    pub reader_failed: bool,
}

/// Unix address injected into one named workerd external service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalServiceAddress {
    name: String,
    address: PathBuf,
}

/// Absolute local directory mapped to one named workerd disk service.
#[derive(Clone, Eq, PartialEq)]
pub struct DirectoryServicePath {
    name: String,
    path: PathBuf,
}

impl Debug for DirectoryServicePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryServicePath")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl DirectoryServicePath {
    /// Validate an already-created absolute local directory mapping.
    pub fn local(name: &str, path: &Path) -> Result<Self, PlatformError> {
        validate_service_name(name)?;
        if !path.is_absolute() {
            return Err(directory_invalid());
        }
        let metadata = std::fs::metadata(path).map_err(|_| directory_invalid())?;
        if !metadata.is_dir() {
            return Err(directory_invalid());
        }
        let canonical = std::fs::canonicalize(path).map_err(|_| directory_invalid())?;
        if canonical.to_str().is_none() {
            return Err(directory_invalid());
        }
        Ok(Self {
            name: name.to_owned(),
            path: canonical,
        })
    }
}

impl ExternalServiceAddress {
    /// Validate a service name and absolute named Unix socket path.
    pub fn unix(name: &str, address: &Path) -> Result<Self, PlatformError> {
        validate_service_name(name)?;
        crate::digest::validate_socket_path(address)?;
        Ok(Self {
            name: name.to_owned(),
            address: address.to_owned(),
        })
    }
}

fn validate_service_name(name: &str) -> Result<(), PlatformError> {
    if name.is_empty()
        || name.len() > 64
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "workerd service name is invalid",
        ));
    }
    Ok(())
}

fn directory_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoStorageUnavailable,
        "workerd Durable Object directory mapping is invalid",
    )
}

/// Compiles a private binary config for each spawn attempt.
pub trait ConfigCompiler: Send + Sync + 'static {
    /// Compile or reuse the static config for this spawn attempt.
    fn compile(
        &self,
        startup_id: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send + '_>>;
}

/// Production compiler using task-D `compile_static_config`.
#[derive(Clone)]
pub struct StaticConfigCompiler {
    runtime: VerifiedRuntime,
    lock_path: PathBuf,
    assets_dir: PathBuf,
    runtime_data_dir: PathBuf,
    ingress_socket: PathBuf,
    platform: PlatformReleaseMeta,
    deadline: Duration,
    redactor: Redactor,
    durable_objects: DurableObjectsConfig,
    container_provider: Option<crate::ContainerProviderConfig>,
}

impl Debug for StaticConfigCompiler {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticConfigCompiler")
            .field("runtime", &self.runtime)
            .field("platform", &self.platform)
            .finish_non_exhaustive()
    }
}

impl StaticConfigCompiler {
    /// Bind compiler inputs. Paths must already be absolute and verified.
    #[must_use]
    pub fn new(
        runtime: VerifiedRuntime,
        lock_path: PathBuf,
        assets_dir: PathBuf,
        runtime_data_dir: PathBuf,
        ingress_socket: PathBuf,
        platform: PlatformReleaseMeta,
        deadline: Duration,
        redactor: Redactor,
    ) -> Self {
        Self {
            runtime,
            lock_path,
            assets_dir,
            runtime_data_dir,
            ingress_socket,
            platform,
            deadline,
            redactor,
            durable_objects: DurableObjectsConfig::default(),
            container_provider: None,
        }
    }

    /// Enable the qualified native Container provider only on the private `DoHost` Worker.
    #[must_use]
    pub fn with_container_provider(mut self, config: crate::ContainerProviderConfig) -> Self {
        self.container_provider = Some(config);
        self
    }

    /// Render validated Durable Object limits into private system-Worker bindings.
    #[must_use]
    pub fn with_durable_objects_config(mut self, config: DurableObjectsConfig) -> Self {
        self.durable_objects = config;
        self
    }
}

impl ConfigCompiler for StaticConfigCompiler {
    fn compile(
        &self,
        _startup_id: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send + '_>> {
        Box::pin(async move {
            compile_static_config(CompileRequest {
                role: crate::ConfigRole::Runtime,
                runtime: &self.runtime,
                lock_path: &self.lock_path,
                assets_dir: &self.assets_dir,
                runtime_data_dir: &self.runtime_data_dir,
                platform: &self.platform,
                ingress_socket: &self.ingress_socket,
                durable_objects: self.durable_objects.clone(),
                container_provider: self.container_provider.as_ref(),
                deadline: self.deadline,
                redactor: &self.redactor,
            })
            .await
        })
    }
}

/// Function-backed compiler for tests.
#[cfg(any(test, feature = "test-support"))]
pub struct FnCompiler<F>(pub F);

#[cfg(any(test, feature = "test-support"))]
impl<F> Debug for FnCompiler<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FnCompiler").finish()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<F> ConfigCompiler for FnCompiler<F>
where
    F: Send
        + Sync
        + 'static
        + Fn(StartupId) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send>>,
{
    fn compile(
        &self,
        startup_id: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send + '_>> {
        (self.0)(startup_id)
    }
}

enum Command {
    Start,
    /// Bridge-level suspicion for one generation. The actor verifies the [`StartupId`]
    /// against the current Running child and answers with one functional probe; it never
    /// tears down on suspicion alone.
    SuspectUnhealthy {
        startup_id: StartupId,
        evidence: RuntimeFailureEvidence,
    },
    /// Result of one functional liveness probe, fenced to the generation it probed.
    FunctionalProbeResult {
        startup_id: StartupId,
        healthy: bool,
    },
    /// Explicit operator-requested generation rotation.
    RotateGeneration {
        ack: oneshot::Sender<Result<(), PlatformError>>,
    },
    /// Immediate teardown and restart. Test-support only: it simulates a runtime crash and
    /// must not be reachable from production suspicion paths.
    #[cfg(any(test, feature = "test-support"))]
    ForceRestartForTest,
    BeginDrain,
    Shutdown {
        ack: Option<oneshot::Sender<()>>,
    },
}

enum AttemptOutcome {
    Ready(Box<LiveRuntime>),
    Failed(SpawnFailure),
    Cancelled,
}

struct InFlight {
    task: tokio::task::JoinHandle<AttemptOutcome>,
    cancel: Option<oneshot::Sender<()>>,
}

/// Construction options for [`WorkerdSupervisor`].
pub struct WorkerdSupervisorOptions<C, K, J> {
    /// Verified workerd identity used as the only executable.
    pub runtime: VerifiedRuntime,
    /// Config compiler invoked on every spawn attempt.
    pub compiler: C,
    /// Runtime timeouts and restart budget.
    pub config: RuntimeConfig,
    /// Clock for snapshot timestamps and backoff.
    pub clock: Arc<K>,
    /// Jitter source for backoff.
    pub jitter: Arc<J>,
    /// Optional absolute path for the secret-free child lease.
    pub lease_path: Option<PathBuf>,
}

impl<C, K, J> Debug for WorkerdSupervisorOptions<C, K, J>
where
    C: Debug,
    K: Debug,
    J: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerdSupervisorOptions")
            .field("runtime", &self.runtime)
            .field("compiler", &self.compiler)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Owns the workerd child, restart policy, and shutdown.
pub struct WorkerdSupervisor {
    tx: mpsc::UnboundedSender<Command>,
    rx: watch::Receiver<SupervisorSnapshot>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    owners: OwnerRegistry,
    diagnostics: Arc<std::sync::Mutex<Option<ProcessDiagnostics>>>,
    #[cfg(any(test, feature = "test-support"))]
    watchdog: Arc<std::sync::RwLock<WatchdogConfig>>,
}

impl Debug for WorkerdSupervisor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerdSupervisor")
            .field("snapshot", &*self.rx.borrow())
            .finish()
    }
}

impl WorkerdSupervisor {
    /// Create a supervisor with the current private service composition.
    pub fn new<C, K, J>(
        opts: WorkerdSupervisorOptions<C, K, J>,
        external_services: Vec<ExternalServiceAddress>,
        directory_services: Vec<DirectoryServicePath>,
    ) -> Self
    where
        C: ConfigCompiler,
        K: Clock + 'static,
        J: JitterRng + 'static,
    {
        Self::new_inner(
            opts,
            external_services,
            directory_services,
            Some(HostExtensionBrokerRegistry::new()),
        )
    }

    /// Create a supervisor that gives each generation a private host-extension broker socket.
    pub fn new_with_host_extension_broker<C, K, J>(
        opts: WorkerdSupervisorOptions<C, K, J>,
        external_services: Vec<ExternalServiceAddress>,
        directory_services: Vec<DirectoryServicePath>,
        host_extension_broker: HostExtensionBrokerRegistry,
    ) -> Self
    where
        C: ConfigCompiler,
        K: Clock + 'static,
        J: JitterRng + 'static,
    {
        Self::new_inner(
            opts,
            external_services,
            directory_services,
            Some(host_extension_broker),
        )
    }

    fn new_inner<C, K, J>(
        opts: WorkerdSupervisorOptions<C, K, J>,
        external_services: Vec<ExternalServiceAddress>,
        directory_services: Vec<DirectoryServicePath>,
        host_extension_broker: Option<HostExtensionBrokerRegistry>,
    ) -> Self
    where
        C: ConfigCompiler,
        K: Clock + 'static,
        J: JitterRng + 'static,
    {
        let now = opts.clock.now();
        let snap = SupervisorSnapshot::initial(now, opts.runtime.binary_sha256().to_owned());
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (watch_tx, watch_rx) = watch::channel(snap.clone());
        let clock: Arc<dyn Clock> = opts.clock;
        let jitter: Arc<dyn JitterRng> = opts.jitter;
        let owners = OwnerRegistry::default();
        let diagnostics = Arc::new(std::sync::Mutex::new(None));
        #[cfg(any(test, feature = "test-support"))]
        let watchdog = Arc::new(std::sync::RwLock::new(WatchdogConfig::default()));
        #[cfg(not(any(test, feature = "test-support")))]
        let watchdog = Arc::new(std::sync::RwLock::new(WatchdogConfig::default()));
        let actor = Actor {
            cmd_tx: cmd_tx.clone(),
            runtime: opts.runtime,
            compiler: Arc::new(opts.compiler) as Arc<dyn ConfigCompiler>,
            owners: owners.clone(),
            config: opts.config,
            clock,
            jitter,
            cmd_rx,
            watch_tx,
            snap,
            child: None,
            in_flight: None,
            budget: RestartBudget::new(),
            consecutive_failures: 0,
            shutting_down: false,
            pending_shutdown_acks: Vec::new(),
            diagnostics: diagnostics.clone(),
            last_report: None,
            lease_path: opts.lease_path,
            host_extension_broker,
            lease_active: false,
            recovery_failed: false,
            external_services: Arc::from(external_services),
            directory_services: Arc::from(directory_services),
            watchdog: watchdog.clone(),
            probe_flight: None,
            probe_failures: 0,
            next_periodic_probe_at: None,
        };
        let task = tokio::spawn(actor.run());
        Self {
            tx: cmd_tx,
            rx: watch_rx,
            task: std::sync::Mutex::new(Some(task)),
            owners,
            diagnostics,
            #[cfg(any(test, feature = "test-support"))]
            watchdog,
        }
    }

    /// Start the runtime if stopped.
    pub fn start(&self) {
        let _ = self.tx.send(Command::Start);
    }

    /// Subscribe to snapshot updates.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<SupervisorSnapshot> {
        self.rx.clone()
    }

    /// Current snapshot.
    #[must_use]
    pub fn snapshot(&self) -> SupervisorSnapshot {
        self.rx.borrow().clone()
    }

    /// Report bridge-level suspicion for one generation. The supervisor confirms it with a
    /// functional liveness probe before any restart; a stale or superseded generation's
    /// suspicion is dropped.
    pub fn suspect_unhealthy(&self, startup_id: StartupId, evidence: RuntimeFailureEvidence) {
        let _ = self.tx.send(Command::SuspectUnhealthy {
            startup_id,
            evidence,
        });
    }

    /// Replace the running workerd generation and wait until its successor is ready.
    pub async fn rotate_generation(&self, timeout: Duration) -> Result<(), PlatformError> {
        let previous = self.snapshot().startup_id;
        let mut snapshots = self.subscribe();
        let (ack, received) = oneshot::channel();
        self.tx
            .send(Command::RotateGeneration { ack })
            .map_err(|_| runtime_rotation_failed())?;
        received.await.map_err(|_| runtime_rotation_failed())??;
        tokio::time::timeout(timeout, async move {
            loop {
                let snapshot = snapshots.borrow().clone();
                if snapshot.state == SupervisorState::Running
                    && snapshot.startup_id.is_some()
                    && snapshot.startup_id != previous
                {
                    return Ok(());
                }
                if snapshot.state == SupervisorState::Failed {
                    return Err(runtime_rotation_failed());
                }
                snapshots
                    .changed()
                    .await
                    .map_err(|_| runtime_rotation_failed())?;
            }
        })
        .await
        .map_err(|_| runtime_rotation_failed())?
    }

    /// Immediate teardown and restart for crash-simulation fixtures. Test-support only.
    #[cfg(any(test, feature = "test-support"))]
    pub fn force_restart_for_test(&self) {
        let _ = self.tx.send(Command::ForceRestartForTest);
    }

    /// Override the functional watchdog timing for fixtures. Test-support only; production
    /// always runs the fixed product constants.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_watchdog_for_test(&self, config: WatchdogConfig) {
        *self
            .watchdog
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    }

    /// Enter DRAINING then stop. Idempotent.
    pub fn begin_drain(&self) {
        let _ = self.tx.send(Command::BeginDrain);
    }

    /// Drain and stop. Idempotent. Returns after the actor is terminal.
    pub async fn shutdown(&self) {
        let (ack, rx) = oneshot::channel();
        let sent = self.tx.send(Command::Shutdown { ack: Some(ack) }).is_ok();
        let task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
            return;
        }
        if sent {
            let _ = rx.await;
        }
    }

    /// Last retained owner completion diagnostics.
    #[must_use]
    pub fn last_diagnostics(&self) -> Option<ProcessDiagnostics> {
        self.diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Number of live owner registrations.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn owner_registry_len(&self) -> usize {
        self.owners.active_count()
    }
}

fn runtime_rotation_failed() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeUnavailable,
        "workerd generation rotation did not complete",
    )
}

impl Drop for WorkerdSupervisor {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown { ack: None });
        self.owners.kill_all();
        // Detach the actor; do not abort it and never signal a snapshot PID.
        if let Ok(mut task) = self.task.lock() {
            let _ = task.take();
        }
    }
}

mod actor;
mod attempt;

use actor::Actor;
use attempt::{ChildStartOptions, run_attempt};

impl WorkerdSupervisor {
    /// Construct with the system clock and OS jitter.
    pub fn with_defaults<C: ConfigCompiler>(
        runtime: VerifiedRuntime,
        compiler: C,
        config: RuntimeConfig,
    ) -> Self {
        Self::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler,
                config,
                clock: Arc::new(SystemClock),
                jitter: Arc::new(OsJitter),
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
        )
    }
}
