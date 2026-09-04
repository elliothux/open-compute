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

struct Actor {
    runtime: VerifiedRuntime,
    compiler: Arc<dyn ConfigCompiler>,
    config: RuntimeConfig,
    clock: Arc<dyn Clock>,
    jitter: Arc<dyn JitterRng>,
    redactor: Redactor,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    watch_tx: watch::Sender<SupervisorSnapshot>,
    snap: SupervisorSnapshot,
    child: Option<LiveRuntime>,
    in_flight: Option<InFlight>,
    budget: RestartBudget,
    consecutive_failures: u32,
    shutting_down: bool,
    owners: OwnerRegistry,
    pending_shutdown_acks: Vec<oneshot::Sender<()>>,
    diagnostics: Arc<std::sync::Mutex<Option<ProcessDiagnostics>>>,
    last_report: Option<OwnerCompletion>,
    lease_path: Option<PathBuf>,
    lease_active: bool,
    recovery_failed: bool,
    external_services: Arc<[ExternalServiceAddress]>,
    directory_services: Arc<[DirectoryServicePath]>,
    generation_auths: Arc<[GenerationAuthRegistry]>,
}

impl Actor {
    async fn run(mut self) {
        if let Some(path) = &self.lease_path {
            let digest = self.runtime.binary_sha256().to_owned();
            let path = path.clone();
            match tokio::task::spawn_blocking(move || recover_orphans(&path, &digest)).await {
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => {
                    self.recovery_failed = true;
                    self.permanent_fail(ErrorCode::RuntimeInvalid);
                }
            }
        }
        loop {
            if let Some(mut flight) = self.in_flight.take() {
                tokio::select! {
                    cmd = self.cmd_rx.recv() => {
                        self.in_flight = Some(flight);
                        let Some(cmd) = cmd else { break; };
                        self.handle(cmd).await;
                        if self.shutdown_is_terminal() {
                            break;
                        }
                    }
                    result = &mut flight.task => {
                        if let Ok(outcome) = result {
                            self.on_attempt_outcome(outcome).await;
                        }
                        if self.shutdown_is_terminal() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        self.in_flight = Some(flight);
                        self.on_tick().await;
                    }
                }
            } else {
                tokio::select! {
                    cmd = self.cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break; };
                        self.handle(cmd).await;
                        if self.shutdown_is_terminal() {
                            break;
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        self.on_tick().await;
                    }
                }
            }
        }
        self.force_stop().await;
        for ack in self.pending_shutdown_acks.drain(..) {
            let _ = ack.send(());
        }
        self.ack_pending_shutdowns();
    }

    fn shutdown_is_terminal(&self) -> bool {
        self.shutting_down
            && matches!(
                self.snap.state,
                SupervisorState::Stopped | SupervisorState::Failed
            )
    }

    fn fail_closed_after_teardown(&mut self) {
        self.clear_generation_auths();
        self.recovery_failed = true;
        self.permanent_fail(ErrorCode::RuntimeInvalid);
    }

    fn clear_generation_auths(&self) {
        for auth in self.generation_auths.iter() {
            auth.clear();
        }
    }

    async fn on_tick(&mut self) {
        self.poll_running().await;
        if self.snap.state == SupervisorState::BackingOff
            && let Some(at) = self.snap.next_retry_at
            && self.clock.now() >= at
            && !self.shutting_down
            && !self.recovery_failed
            && self.in_flight.is_none()
        {
            self.begin_attempt();
        }
    }

    fn ack_pending_shutdowns(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            if let Command::Shutdown { ack: Some(ack) } = cmd {
                let _ = ack.send(());
            }
        }
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Start => {
                if self.recovery_failed {
                    return;
                }
                if matches!(
                    self.snap.state,
                    SupervisorState::Stopped | SupervisorState::Failed
                ) && !self.shutting_down
                    && self.in_flight.is_none()
                {
                    self.budget = RestartBudget::new();
                    self.consecutive_failures = 0;
                    self.begin_attempt();
                }
            }
            Command::ReportUnhealthy => {
                if self.snap.state == SupervisorState::Running {
                    match self.teardown_child().await {
                        Ok(report) => {
                            self.fail_or_backoff(
                                ErrorCode::RuntimeExitedInFlight,
                                true,
                                report.as_ref(),
                            )
                            .await;
                        }
                        Err(_) => self.fail_closed_after_teardown(),
                    }
                }
            }
            Command::BeginDrain => {
                self.shutting_down = true;
                if self.cancel_attempt().await.is_err() {
                    self.fail_closed_after_teardown();
                } else {
                    self.graceful_stop().await;
                }
            }
            Command::Shutdown { ack } => {
                self.shutting_down = true;
                if let Some(ack) = ack {
                    self.pending_shutdown_acks.push(ack);
                }
                if self.cancel_attempt().await.is_err() {
                    self.fail_closed_after_teardown();
                } else {
                    self.graceful_stop().await;
                }
            }
        }
    }

    fn begin_attempt(&mut self) {
        self.last_report = None;
        self.transition(
            SupervisorState::Starting,
            ReadinessReason::RuntimeStarting,
            None,
            None,
        );
        self.snap.attempt = self.snap.attempt.saturating_add(1);
        self.publish();

        let token = match generate_internal_token() {
            Ok(t) => t,
            Err(err) => {
                self.permanent_fail(err.code());
                return;
            }
        };
        let mut redactor = self.redactor.clone();
        redactor.register_secret_string(&token);
        let fingerprint = token_fingerprint(&token);
        let startup_id = StartupId::generate();
        self.snap.startup_id = Some(startup_id);
        self.snap.token_fingerprint = Some(fingerprint);
        self.publish();

        let (cancel_tx, cancel_rx) = oneshot::channel();
        let compiler = self.compiler.clone();
        let runtime = self.runtime.clone();
        let startup = Duration::from_millis(self.config.startup_timeout_ms);
        let task = tokio::spawn(run_attempt(
            AttemptArgs {
                compiler,
                runtime,
                token,
                redactor,
                startup_id,
                startup,
                owners: self.owners.clone(),
                external_services: self.external_services.clone(),
                directory_services: self.directory_services.clone(),
                lease_path: self.lease_path.clone(),
            },
            cancel_rx,
        ));
        self.in_flight = Some(InFlight {
            task,
            cancel: Some(cancel_tx),
        });
    }

    async fn cancel_attempt(&mut self) -> Result<(), PlatformError> {
        let Some(mut flight) = self.in_flight.take() else {
            return Ok(());
        };
        if let Some(cancel) = flight.cancel.take() {
            let _ = cancel.send(());
        }
        match flight.task.await {
            Ok(AttemptOutcome::Ready(live)) => {
                self.child = Some(*live);
                self.teardown_child().await?;
            }
            Ok(AttemptOutcome::Failed(fail)) => {
                self.clear_generation_auths();
                if let Some(report) = fail.completion {
                    self.record_completion(report);
                }
                if let Some(pid) = fail.pid {
                    assert_reaped(Some(pid))?;
                }
            }
            Ok(AttemptOutcome::Cancelled) | Err(_) => {
                self.clear_generation_auths();
            }
        }
        Ok(())
    }

    async fn on_attempt_outcome(&mut self, outcome: AttemptOutcome) {
        match outcome {
            AttemptOutcome::Ready(live) => {
                if self.shutting_down {
                    self.child = Some(*live);
                    if self.teardown_child().await.is_err() {
                        self.fail_closed_after_teardown();
                    }
                    return;
                }
                self.consecutive_failures = 0;
                self.snap.config_digest = live.config_digest.clone();
                self.snap.listen_port = Some(live.port);
                let pid = live.pid();
                let pgid = live.pgid();
                self.child = Some(*live);
                if self.persist_lease(pid, pgid).is_err() {
                    let _ = self.teardown_child().await;
                    self.fail_closed_after_teardown();
                    return;
                }
                self.transition(
                    SupervisorState::Running,
                    ReadinessReason::Ready,
                    Some(pid),
                    Some(pgid),
                );
            }
            AttemptOutcome::Failed(fail) => {
                self.clear_generation_auths();
                let report = fail.completion.clone();
                if let Some(report) = fail.completion {
                    self.record_completion(report);
                }
                if let Some(pid) = fail.pid
                    && assert_reaped(Some(pid)).is_err()
                {
                    self.fail_closed_after_teardown();
                    return;
                }
                self.snap.pid = None;
                self.snap.pgid = None;
                self.snap.listen_port = None;
                if self.shutting_down {
                    self.transition(
                        SupervisorState::Stopped,
                        ReadinessReason::Draining,
                        None,
                        None,
                    );
                    return;
                }
                let retryable = !is_permanent(fail.error.code());
                self.fail_or_backoff(fail.error.code(), retryable, report.as_ref())
                    .await;
            }
            AttemptOutcome::Cancelled => {
                self.clear_generation_auths();
                if self.shutting_down {
                    self.transition(
                        SupervisorState::Stopped,
                        ReadinessReason::Draining,
                        None,
                        None,
                    );
                }
            }
        }
    }

    async fn poll_running(&mut self) {
        if self.snap.state != SupervisorState::Running {
            return;
        }
        let mut exited = false;
        let mut unhealthy = false;
        if let Some(live) = self.child.as_mut() {
            if !live.handle.leader_alive() {
                exited = true;
            } else {
                let mut buf = [0u8; 1024];
                let read = if let Ok(control) = live.ensure_control() {
                    tokio::time::timeout(Duration::from_millis(1), control.read(&mut buf)).await
                } else {
                    unhealthy = true;
                    Ok(Ok(0))
                };
                if !unhealthy {
                    match read {
                        Ok(Ok(0)) => {
                            if !live.parser.accepted() {
                                unhealthy = true;
                            }
                        }
                        Ok(Ok(n)) => {
                            if live.parser.push(&buf[..n]).is_err() {
                                unhealthy = true;
                            }
                        }
                        Ok(Err(_)) => unhealthy = true,
                        Err(_) => {}
                    }
                }
            }
        }
        if exited {
            self.on_child_exit().await;
        } else if unhealthy {
            self.control_unhealthy().await;
        }
    }

    async fn control_unhealthy(&mut self) {
        if self.shutting_down {
            return;
        }
        match self.teardown_child().await {
            Ok(report) => {
                self.fail_or_backoff(ErrorCode::RuntimeExitedInFlight, true, report.as_ref())
                    .await;
            }
            Err(_) => self.fail_closed_after_teardown(),
        }
    }

    async fn on_child_exit(&mut self) {
        let Ok(report) = self.teardown_child().await else {
            self.fail_closed_after_teardown();
            return;
        };
        if self.shutting_down {
            self.transition(
                SupervisorState::Stopped,
                ReadinessReason::Draining,
                None,
                None,
            );
            return;
        }
        self.fail_or_backoff(ErrorCode::RuntimeExitedInFlight, true, report.as_ref())
            .await;
    }

    async fn fail_or_backoff(
        &mut self,
        code: ErrorCode,
        consume_budget: bool,
        report: Option<&OwnerCompletion>,
    ) {
        let now = self.clock.now();
        let window = Duration::from_millis(self.config.restart_window_ms);
        if consume_budget {
            self.budget.record(now, window);
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let (exit_code, signal) = report.map_or((None, None), |r| (r.exit_code(), r.signal()));
        self.snap.last_exit = Some(SanitizedExit {
            code: exit_code,
            signal,
            retryable: consume_budget && !is_permanent(code),
            code_name: code.as_str().to_owned(),
        });
        if is_permanent(code) {
            self.permanent_fail(code);
            return;
        }
        if consume_budget && self.budget.exceeded(self.config.restart_budget) {
            self.permanent_fail(ErrorCode::RuntimeInvalid);
            self.snap.reason = ReadinessReason::RuntimeInvalid;
            self.publish();
            return;
        }
        let delay = backoff_delay(
            &self.config,
            self.consecutive_failures,
            self.jitter.as_ref(),
        );
        let next = now + delay;
        self.snap.next_retry_at = Some(next);
        self.transition(
            SupervisorState::BackingOff,
            ReadinessReason::RuntimeRestartBackoff,
            None,
            None,
        );
    }

    fn permanent_fail(&mut self, code: ErrorCode) {
        let reason = match code {
            ErrorCode::ConfigCompileFailed | ErrorCode::ConfigInvalid => {
                ReadinessReason::ConfigInvalid
            }
            _ => ReadinessReason::RuntimeInvalid,
        };
        self.snap.next_retry_at = None;
        self.snap.pid = None;
        self.snap.pgid = None;
        self.transition(SupervisorState::Failed, reason, None, None);
    }

    async fn graceful_stop(&mut self) {
        if self.snap.state == SupervisorState::Stopped {
            return;
        }
        self.transition(
            SupervisorState::Draining,
            ReadinessReason::Draining,
            self.snap.pid,
            self.snap.pgid,
        );
        tokio::time::sleep(Duration::from_millis(self.config.drain_timeout_ms)).await;
        self.transition(
            SupervisorState::Stopping,
            ReadinessReason::Draining,
            self.snap.pid,
            self.snap.pgid,
        );
        match self.teardown_child().await {
            Ok(_) => self.transition(
                SupervisorState::Stopped,
                ReadinessReason::Draining,
                None,
                None,
            ),
            Err(_) => self.fail_closed_after_teardown(),
        }
    }

    fn persist_lease(&mut self, pid: i32, pgid: i32) -> Result<(), PlatformError> {
        let Some(path) = &self.lease_path else {
            return Ok(());
        };
        let Some(lease) = capture_lease(pid, pgid, self.runtime.binary_sha256()) else {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to capture runtime child lease identity",
            ));
        };
        write_lease(path, &lease)?;
        self.lease_active = true;
        Ok(())
    }

    async fn teardown_child(&mut self) -> Result<Option<OwnerCompletion>, PlatformError> {
        self.clear_generation_auths();
        let report = if let Some(live) = self.child.take() {
            let pid = live.pid();
            let pgid = live.pgid();
            let report = live
                .shutdown(
                    Duration::from_millis(self.config.shutdown_grace_ms),
                    Duration::from_millis(self.config.kill_timeout_ms),
                )
                .await;
            self.record_completion(report.clone());
            if pid != pgid {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "runtime child is not its process group leader",
                ));
            }
            wait_reaped(pid, Duration::from_secs(2))?;
            Some(report)
        } else {
            None
        };
        if report.is_none() && self.lease_active {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "active child lease cannot be cleared without a reap proof",
            ));
        }
        if self.lease_active {
            let path = self.lease_path.as_ref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "active child lease path is missing",
                )
            })?;
            clear_lease(path)?;
            self.lease_active = false;
        }
        self.snap.pid = None;
        self.snap.pgid = None;
        self.snap.listen_port = None;
        Ok(report)
    }

    fn record_completion(&mut self, report: OwnerCompletion) {
        let diag = ProcessDiagnostics {
            stdout_tail: report.stdout.as_lossy_str(),
            stderr_tail: report.stderr.as_lossy_str(),
            exit_code: report.exit_code(),
            signal: report.signal(),
            reader_failed: report.reader_failed,
        };
        *self
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(diag);
        self.last_report = Some(report);
    }

    async fn force_stop(&mut self) {
        if self.recovery_failed {
            return;
        }
        if self.cancel_attempt().await.is_err() || self.teardown_child().await.is_err() {
            self.fail_closed_after_teardown();
            return;
        }
        if self.snap.state != SupervisorState::Stopped {
            self.transition(
                SupervisorState::Stopped,
                ReadinessReason::Draining,
                None,
                None,
            );
        }
    }

    fn transition(
        &mut self,
        state: SupervisorState,
        reason: ReadinessReason,
        pid: Option<i32>,
        pgid: Option<i32>,
    ) {
        self.snap.state = state;
        self.snap.reason = reason;
        self.snap.last_transition_at = self.clock.now();
        self.snap.pid = pid;
        self.snap.pgid = pgid;
        if !matches!(
            state,
            SupervisorState::Starting
                | SupervisorState::Running
                | SupervisorState::Draining
                | SupervisorState::Stopping
        ) {
            self.snap.pid = None;
            self.snap.pgid = None;
        }
        self.publish();
    }

    fn publish(&self) {
        let _ = self.watch_tx.send(self.snap.clone());
    }
}

struct AttemptArgs {
    compiler: Arc<dyn ConfigCompiler>,
    runtime: VerifiedRuntime,
    token: SecretString,
    redactor: Redactor,
    startup_id: StartupId,
    startup: Duration,
    owners: OwnerRegistry,
    external_services: Arc<[ExternalServiceAddress]>,
    directory_services: Arc<[DirectoryServicePath]>,
    lease_path: Option<PathBuf>,
}

async fn run_attempt(args: AttemptArgs, mut cancel: oneshot::Receiver<()>) -> AttemptOutcome {
    let AttemptArgs {
        compiler,
        runtime,
        token,
        redactor,
        startup_id,
        startup,
        owners,
        external_services,
        directory_services,
        lease_path,
    } = args;
    let compiled = tokio::select! {
        biased;
        _ = &mut cancel => return AttemptOutcome::Cancelled,
        compiled = compiler.compile(token.clone(), startup_id) => compiled,
    };
    let compiled = match compiled {
        Ok(c) => c,
        Err(err) => {
            return AttemptOutcome::Failed(SpawnFailure {
                error: err,
                pid: None,
                pgid: None,
                completion: None,
            });
        }
    };

    let runtime_spawn = runtime.clone();
    let token_spawn = token.clone();
    let redactor_spawn = redactor.clone();
    let owners_spawn = owners.clone();
    let spawn_lease_path = lease_path.clone();
    let spawn_task = tokio::task::spawn_blocking(move || {
        spawn_child(&SpawnRequest {
            runtime: &runtime_spawn,
            compiled: &compiled,
            token: &token_spawn,
            redactor: &redactor_spawn,
            owners: &owners_spawn,
            external_services: &external_services,
            directory_services: &directory_services,
            lease_path: spawn_lease_path.as_deref(),
        })
    });
    tokio::pin!(spawn_task);
    let mut cancel_pending = false;
    let spawned = loop {
        tokio::select! {
            biased;
            _ = &mut cancel, if !cancel_pending => {
                cancel_pending = true;
            }
            spawned = &mut spawn_task => break spawned,
        }
    };
    let mut live = match spawned {
        Ok(Ok(live)) => live,
        Ok(Err(fail)) => return AttemptOutcome::Failed(fail),
        Err(_) => {
            return AttemptOutcome::Failed(SpawnFailure {
                error: PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "runtime spawn task ended without a result",
                ),
                pid: None,
                pgid: None,
                completion: None,
            });
        }
    };
    if cancel_pending {
        let pid = live.pid();
        live.shutdown(Duration::from_millis(0), Duration::from_secs(2))
            .await;
        clear_attempt_lease_if_reaped(lease_path.as_deref(), pid);
        return AttemptOutcome::Cancelled;
    }

    tokio::select! {
        biased;
        _ = &mut cancel => {
            let pid = live.pid();
            live.shutdown(Duration::from_millis(0), Duration::from_secs(2)).await;
            clear_attempt_lease_if_reaped(lease_path.as_deref(), pid);
            AttemptOutcome::Cancelled
        }
        ready = wait_ready(&mut live, &token, startup) => {
            match ready {
                Ok(port) => {
                    live.port = port;
                    AttemptOutcome::Ready(Box::new(live))
                }
                Err(error) => {
                    let pid = live.pid();
                    let pgid = live.pgid();
                    let completion = live.shutdown(Duration::from_millis(0), Duration::from_secs(2)).await;
                    clear_attempt_lease_if_reaped(lease_path.as_deref(), pid);
                    AttemptOutcome::Failed(SpawnFailure {
                        error,
                        pid: Some(pid),
                        pgid: Some(pgid),
                        completion: Some(completion),
                    })
                }
            }
        }
    }
}

fn clear_attempt_lease_if_reaped(lease_path: Option<&Path>, pid: i32) {
    if wait_reaped(pid, Duration::from_secs(2)).is_ok()
        && let Some(path) = lease_path
    {
        let _ = clear_lease(path);
    }
}

fn is_permanent(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::RuntimeInvalid
            | ErrorCode::ConfigCompileFailed
            | ErrorCode::ConfigInvalid
            | ErrorCode::PathInvalid
            | ErrorCode::CacheEntryCorrupt
            | ErrorCode::LimitInvalid
            | ErrorCode::SchemaTooNew
            | ErrorCode::MasterKeyMismatch
    )
}

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
