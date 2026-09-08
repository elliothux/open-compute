//! workerd process supervisor: spawn, ready, restart, drain, reap.

mod backoff;
mod control;
mod logs;
mod owner;
mod probe;
mod spawn;
mod state;
mod token;

use crate::compile::{CompileRequest, CompiledConfig, PlatformReleaseMeta, compile_static_config};
use crate::lease::{capture_lease, clear_lease, recover_orphans, write_lease};
use crate::process::{assert_reaped, wait_reaped};
use crate::verify::VerifiedRuntime;
use backoff::{RestartBudget, backoff_delay};
use open_compute_core::clock::Clock;
use open_compute_core::config::{DurableObjectsConfig, RuntimeConfig};
use open_compute_core::error::ReadinessReason;
use open_compute_core::ids::StartupId;
use open_compute_core::{ErrorCode, PlatformError, Redactor, SecretString, SystemClock};
use owner::{OwnerCompletion, OwnerRegistry};
use spawn::{LiveRuntime, SpawnFailure, SpawnRequest, spawn_child, wait_ready};
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot, watch};

#[cfg(any(test, feature = "test-support"))]
pub use backoff::SequenceJitter;
pub use backoff::{JitterRng, OsJitter};
#[cfg(any(test, feature = "test-support"))]
pub use logs::set_reader_fail_point;
#[cfg(any(test, feature = "test-support"))]
pub use owner::{take_owner_wait_count, take_reader_join_errors};
pub use probe::{READY_PATH, TOKEN_HEADER, probe_ready_with_raw_token};
pub use spawn::serve_argv;
#[cfg(any(test, feature = "test-support"))]
pub use spawn::{
    blocking_spawn_is_waiting, clear_blocking_spawn_hold, hold_blocking_spawn, last_spawned_pid,
    release_blocking_spawn, set_spawn_fail_point,
};
pub use state::{SanitizedExit, SupervisorSnapshot, SupervisorState};
pub use token::{
    GenerationAuthRegistry, GenerationCredential, generate_internal_token, token_fingerprint,
};

/// Bounded redacted child diagnostics. Not part of ordinary snapshot/status/Debug.
#[derive(Clone, Debug, Default)]
pub struct ProcessDiagnostics {
    /// Redacted stdout tail.
    pub stdout_tail: String,
    /// Redacted stderr tail.
    pub stderr_tail: String,
    /// Child exit code if it exited.
    pub exit_code: Option<i32>,
    /// POSIX signal if terminated by signal.
    pub signal: Option<i32>,
    /// True if a stdout/stderr reader failed or panicked.
    pub reader_failed: bool,
}

/// Loopback address injected into one named workerd external service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalServiceAddress {
    name: String,
    address: SocketAddr,
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
        let metadata = std::fs::symlink_metadata(path).map_err(|_| directory_invalid())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
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
    /// Validate a service name and a nonzero loopback address.
    pub fn loopback(name: &str, address: SocketAddr) -> Result<Self, PlatformError> {
        if validate_service_name(name).is_err()
            || !address.ip().is_loopback()
            || address.port() == 0
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "external service must have a bounded name and loopback address",
            ));
        }
        Ok(Self {
            name: name.to_owned(),
            address,
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

/// Compiles a generation-scoped binary config from a fresh token.
pub trait ConfigCompiler: Send + Sync + 'static {
    /// Compile or reuse the static config for this spawn attempt.
    fn compile(
        &self,
        token: SecretString,
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
    platform: PlatformReleaseMeta,
    deadline: Duration,
    redactor: Redactor,
    generation_auth: Option<GenerationAuthRegistry>,
    binding_generation_auth: Option<GenerationAuthRegistry>,
    observability_generation_auth: Option<GenerationAuthRegistry>,
    durable_objects: DurableObjectsConfig,
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
        platform: PlatformReleaseMeta,
        deadline: Duration,
        redactor: Redactor,
    ) -> Self {
        Self {
            runtime,
            lock_path,
            assets_dir,
            runtime_data_dir,
            platform,
            deadline,
            redactor,
            generation_auth: None,
            binding_generation_auth: None,
            observability_generation_auth: None,
            durable_objects: DurableObjectsConfig::default(),
        }
    }

    /// Activate generation-scoped loopback authentication after each successful compile.
    #[must_use]
    pub fn with_generation_auth(mut self, auth: GenerationAuthRegistry) -> Self {
        self.generation_auth = Some(auth);
        self
    }

    /// Activate a distinct generation credential for the private binding backend.
    #[must_use]
    pub fn with_binding_generation_auth(mut self, auth: GenerationAuthRegistry) -> Self {
        self.binding_generation_auth = Some(auth);
        self
    }

    /// Activate a distinct generation credential for Workers Logs ingestion.
    #[must_use]
    pub fn with_observability_generation_auth(mut self, auth: GenerationAuthRegistry) -> Self {
        self.observability_generation_auth = Some(auth);
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
        token: SecretString,
        _startup_id: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send + '_>> {
        Box::pin(async move {
            let binding_token = generate_internal_token()?;
            let observability_token = generate_internal_token()?;
            let mut redactor = self.redactor.clone();
            redactor.register_secret_string(&token);
            redactor.register_secret_string(&binding_token);
            redactor.register_secret_string(&observability_token);
            let compiled = compile_static_config(CompileRequest {
                runtime: &self.runtime,
                lock_path: &self.lock_path,
                assets_dir: &self.assets_dir,
                runtime_data_dir: &self.runtime_data_dir,
                platform: &self.platform,
                token: &token,
                binding_token: &binding_token,
                observability_token: &observability_token,
                durable_objects: self.durable_objects.clone(),
                deadline: self.deadline,
                redactor: &redactor,
            })
            .await;
            if compiled.is_ok()
                && let Some(auth) = &self.generation_auth
            {
                auth.activate(token.clone());
            }
            if compiled.is_ok()
                && let Some(auth) = &self.binding_generation_auth
            {
                auth.activate(binding_token);
            }
            if compiled.is_ok()
                && let Some(auth) = &self.observability_generation_auth
            {
                auth.activate(observability_token);
            }
            compiled
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
        + Fn(
            SecretString,
            StartupId,
        ) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send>>,
{
    fn compile(
        &self,
        token: SecretString,
        startup_id: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<CompiledConfig, PlatformError>> + Send + '_>> {
        (self.0)(token, startup_id)
    }
}

enum Command {
    Start,
    ReportUnhealthy,
    BeginDrain,
    Shutdown { ack: Option<oneshot::Sender<()>> },
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
    /// Redactor that will also receive each generation token.
    pub redactor: Redactor,
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
}

impl Debug for WorkerdSupervisor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerdSupervisor")
            .field("snapshot", &*self.rx.borrow())
            .finish()
    }
}

impl WorkerdSupervisor {
    /// Create a supervisor with the complete current service and auth composition.
    pub fn new<C, K, J>(
        opts: WorkerdSupervisorOptions<C, K, J>,
        external_services: Vec<ExternalServiceAddress>,
        directory_services: Vec<DirectoryServicePath>,
        generation_auths: Vec<GenerationAuthRegistry>,
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
        let actor = Actor {
            runtime: opts.runtime,
            compiler: Arc::new(opts.compiler) as Arc<dyn ConfigCompiler>,
            owners: owners.clone(),
            config: opts.config,
            clock,
            jitter,
            redactor: opts.redactor,
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
            lease_active: false,
            recovery_failed: false,
            external_services: Arc::from(external_services),
            directory_services: Arc::from(directory_services),
            generation_auths: Arc::from(generation_auths),
        };
        let task = tokio::spawn(actor.run());
        Self {
            tx: cmd_tx,
            rx: watch_rx,
            task: std::sync::Mutex::new(Some(task)),
            owners,
            diagnostics,
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

    /// Mark the running runtime unhealthy; consumes restart budget.
    pub fn report_unhealthy(&self) {
        let _ = self.tx.send(Command::ReportUnhealthy);
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

use actor::Actor;

impl WorkerdSupervisor {
    /// Construct with the system clock and OS jitter.
    pub fn with_defaults<C: ConfigCompiler>(
        runtime: VerifiedRuntime,
        compiler: C,
        config: RuntimeConfig,
        redactor: Redactor,
    ) -> Self {
        Self::new(
            WorkerdSupervisorOptions {
                runtime,
                compiler,
                config,
                clock: Arc::new(SystemClock),
                jitter: Arc::new(OsJitter),
                redactor,
                lease_path: None,
            },
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }
}
