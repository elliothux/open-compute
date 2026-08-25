//! Spawn a verified workerd (or fixture) with stdin config and control fd 3.

use super::control::ControlParser;
use super::logs::LogCollector;
use super::owner::{ChildHandle, OwnerCompletion};
use super::probe::probe_ready;
use super::{DirectoryServicePath, ExternalServiceAddress};
use crate::compile::CompiledConfig;
use crate::lease::{capture_lease, clear_lease, write_lease};
use crate::lock::RuntimeLock;
use crate::process::{assert_reaped, exec_image, exec_image_with_lease, verify_self_pgid};
use crate::verify::VerifiedRuntime;
use command_fds::{CommandFdExt, FdMapping};
use open_compute_core::{ErrorCode, PlatformError, Redactor, SecretString};
use rustix::process::{Pid, getpgid};
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// Fixed argv for `workerd serve --binary -`.
#[must_use]
pub fn serve_argv(lock: &RuntimeLock) -> Vec<String> {
    serve_argv_with_external(lock, &[])
}

pub(crate) fn serve_argv_with_external(
    lock: &RuntimeLock,
    external_services: &[ExternalServiceAddress],
) -> Vec<String> {
    serve_argv_with_services(lock, external_services, &[])
}

pub(crate) fn serve_argv_with_services(
    lock: &RuntimeLock,
    external_services: &[ExternalServiceAddress],
    directory_services: &[DirectoryServicePath],
) -> Vec<String> {
    let mut args = vec!["serve".to_owned(), "--binary".to_owned(), "-".to_owned()];
    args.extend(lock.process_flags.iter().cloned());
    args.push("--control-fd=3".to_owned());
    args.push("--socket-addr=http=127.0.0.1:0".to_owned());
    for service in external_services {
        args.push(format!(
            "--external-addr={}={}",
            service.name, service.address
        ));
    }
    for service in directory_services {
        args.push(format!(
            "--directory-path={}={}",
            service.name,
            service.path.display()
        ));
    }
    args
}

/// A spawned runtime whose process group is owned by [`ChildHandle`].
pub(crate) struct LiveRuntime {
    pub handle: ChildHandle,
    pub port: u16,
    control_std: Option<UnixStream>,
    pub control: Option<tokio::net::UnixStream>,
    pub parser: ControlParser,
    #[allow(dead_code)]
    pub stdout: LogCollector,
    #[allow(dead_code)]
    pub stderr: LogCollector,
    pub config_digest: String,
    _image: crate::process::ExecImage,
}

impl LiveRuntime {
    pub(crate) fn ensure_control(&mut self) -> Result<&mut tokio::net::UnixStream, PlatformError> {
        if self.control.is_none() {
            let std = self.control_std.take().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to wrap runtime control socket",
                )
            })?;
            self.control = Some(tokio::net::UnixStream::from_std(std).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to wrap runtime control socket",
                )
            })?);
        }
        self.control.as_mut().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wrap runtime control socket",
            )
        })
    }
}

impl LiveRuntime {
    pub(crate) fn pid(&self) -> i32 {
        self.handle.pid
    }

    pub(crate) fn pgid(&self) -> i32 {
        self.handle.pgid
    }

    pub(crate) async fn shutdown(self, grace: Duration, kill_after: Duration) -> OwnerCompletion {
        let LiveRuntime {
            handle,
            _image: image,
            ..
        } = self;
        let completion = handle.shutdown(grace, kill_after).await;
        drop(image);
        completion
    }
}

pub(crate) struct SpawnRequest<'a> {
    pub runtime: &'a VerifiedRuntime,
    pub compiled: &'a CompiledConfig,
    #[allow(dead_code)]
    pub token: &'a SecretString,
    pub redactor: &'a Redactor,
    pub owners: &'a super::owner::OwnerRegistry,
    pub external_services: &'a [ExternalServiceAddress],
    pub directory_services: &'a [DirectoryServicePath],
    pub lease_path: Option<&'a Path>,
}

pub(crate) struct SpawnFailure {
    pub error: PlatformError,
    pub pid: Option<i32>,
    #[allow(dead_code)]
    pub pgid: Option<i32>,
    pub completion: Option<OwnerCompletion>,
}

impl SpawnFailure {
    fn without_child(error: PlatformError) -> Self {
        Self {
            error,
            pid: None,
            pgid: None,
            completion: None,
        }
    }
}

pub(crate) fn spawn_child(req: &SpawnRequest<'_>) -> Result<LiveRuntime, SpawnFailure> {
    wait_if_spawn_held();
    let argv = serve_argv_with_services(
        req.runtime.lock(),
        req.external_services,
        req.directory_services,
    );
    let config = req
        .compiled
        .read_bytes()
        .map_err(SpawnFailure::without_child)?;
    let digest = req.compiled.digest().to_owned();
    let mut live = spawn_child_inner(
        req.runtime,
        &argv,
        &config,
        req.redactor,
        req.owners,
        req.lease_path,
    )?;
    live.config_digest = digest;
    Ok(live)
}

fn spawn_child_inner(
    runtime: &VerifiedRuntime,
    argv: &[String],
    config: &[u8],
    redactor: &Redactor,
    owners: &super::owner::OwnerRegistry,
    lease_path: Option<&Path>,
) -> Result<LiveRuntime, SpawnFailure> {
    let image = match lease_path {
        Some(path) => {
            exec_image_with_lease(runtime.executable_file(), path, runtime.binary_sha256())
        }
        None => exec_image(runtime.executable_file()),
    }
    .map_err(SpawnFailure::without_child)?;
    let (parent_ctl, child_ctl) = UnixStream::pair().map_err(|_| {
        SpawnFailure::without_child(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to create runtime control socket pair",
        ))
    })?;
    parent_ctl.set_nonblocking(true).map_err(|_| {
        SpawnFailure::without_child(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to set control socket non-blocking",
        ))
    })?;

    let mut cmd = std::process::Command::new(&image.program);
    cmd.process_group(0);
    cmd.args(argv);
    cmd.env_clear();
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mapped: OwnedFd = child_ctl.as_fd().try_clone_to_owned().map_err(|_| {
        SpawnFailure::without_child(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to duplicate runtime control file descriptor",
        ))
    })?;
    cmd.fd_mappings(vec![FdMapping {
        parent_fd: mapped,
        child_fd: 3,
    }])
    .map_err(|_| {
        SpawnFailure::without_child(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to map runtime control file descriptor",
        ))
    })?;

    let mut child = cmd.spawn().map_err(|_| {
        SpawnFailure::without_child(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to spawn runtime process",
        ))
    })?;
    drop(child_ctl);

    let pid = child.id() as i32;
    record_last_pid(pid);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_log = LogCollector::new(redactor.clone());
    let stderr_log = LogCollector::new(redactor.clone());

    let handle = match ChildHandle::start(
        child,
        pid,
        pid,
        stdout,
        stderr,
        stdout_log.clone(),
        stderr_log.clone(),
        owners,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            return Err(SpawnFailure {
                error,
                pid: Some(pid),
                pgid: Some(pid),
                completion: None,
            });
        }
    };

    if fail_point() == FailPoint::Pgid || verify_self_pgid(pid).is_err() {
        clear_fail_point();
        let err = PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read runtime process group",
        );
        return Err(reap_fail(handle, err, lease_path));
    }

    let pgid = match read_pgid(pid) {
        Ok(pgid) => pgid,
        Err(error) => return Err(reap_fail(handle, error, lease_path)),
    };
    if pgid != pid {
        return Err(reap_fail(
            handle,
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime process is not its own process group leader",
            ),
            lease_path,
        ));
    }

    if let Some(path) = lease_path {
        let Some(lease) = capture_lease(pid, pgid, runtime.binary_sha256()) else {
            return Err(reap_fail(
                handle,
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to capture runtime child lease identity",
                ),
                lease_path,
            ));
        };
        if let Err(error) = write_lease(path, &lease) {
            return Err(reap_fail(handle, error, lease_path));
        }
    }

    if fail_point() == FailPoint::Stdin {
        clear_fail_point();
        return Err(reap_fail(
            handle,
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to write compiled config to runtime stdin",
            ),
            lease_path,
        ));
    }

    match stdin {
        Some(mut stdin) => {
            if stdin.write_all(config).is_err() {
                return Err(reap_fail(
                    handle,
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to write compiled config to runtime stdin",
                    ),
                    lease_path,
                ));
            }
            drop(stdin);
        }
        None => {
            return Err(reap_fail(
                handle,
                PlatformError::new(ErrorCode::RuntimeInvalid, "runtime stdin was not available"),
                lease_path,
            ));
        }
    }

    if fail_point() == FailPoint::Logs {
        clear_fail_point();
        return Err(reap_fail(
            handle,
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to start runtime log readers",
            ),
            lease_path,
        ));
    }

    if fail_point() == FailPoint::Control {
        clear_fail_point();
        return Err(reap_fail(
            handle,
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wrap runtime control socket",
            ),
            lease_path,
        ));
    }

    Ok(LiveRuntime {
        handle,
        port: 0,
        control_std: Some(parent_ctl),
        control: None,
        parser: ControlParser::new(),
        stdout: stdout_log,
        stderr: stderr_log,
        config_digest: String::new(),
        _image: image,
    })
}

fn reap_fail(handle: ChildHandle, error: PlatformError, lease_path: Option<&Path>) -> SpawnFailure {
    let pid = handle.pid;
    let pgid = handle.pgid;
    let completion = handle.shutdown_blocking(Duration::from_millis(0), Duration::from_secs(2));
    if assert_reaped(Some(pid)).is_ok()
        && let Some(path) = lease_path
    {
        let _ = clear_lease(path);
    }
    SpawnFailure {
        error,
        pid: Some(pid),
        pgid: Some(pgid),
        completion: Some(completion),
    }
}

fn read_pgid(pid: i32) -> Result<i32, PlatformError> {
    let raw = Pid::from_raw(pid).ok_or_else(|| {
        PlatformError::new(ErrorCode::RuntimeInvalid, "runtime process pid is invalid")
    })?;
    Ok(getpgid(Some(raw))
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to read runtime process group",
            )
        })?
        .as_raw_nonzero()
        .get())
}

pub(crate) async fn wait_ready(
    live: &mut LiveRuntime,
    token: &SecretString,
    startup: Duration,
) -> Result<u16, PlatformError> {
    let deadline = Instant::now() + startup;
    let mut buf = [0u8; 1024];
    loop {
        if Instant::now() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::RuntimeExitedBeforeReady,
                "runtime startup deadline expired",
            ));
        }
        if !live.handle.leader_alive() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeExitedBeforeReady,
                "runtime exited before becoming ready",
            ));
        }
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeExitedBeforeReady,
                "runtime startup deadline expired",
            ));
        }
        let control = live.ensure_control()?;
        match tokio::time::timeout(Duration::from_millis(20), control.read(&mut buf)).await {
            Ok(Ok(0)) => {
                if live.parser.listen().is_none() {
                    return Err(PlatformError::new(
                        ErrorCode::RuntimeExitedBeforeReady,
                        "runtime control-fd closed before listen",
                    ));
                }
            }
            Ok(Ok(n)) => live.parser.push(&buf[..n])?,
            Ok(Err(_)) => {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeExitedBeforeReady,
                    "failed to read runtime control-fd",
                ));
            }
            Err(_) => {}
        }
        if let Some(listen) = live.parser.listen() {
            let remain = deadline.saturating_duration_since(Instant::now());
            probe_ready(listen.port, token, remain).await?;
            live.port = listen.port;
            return Ok(listen.port);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FailPoint {
    None,
    Pgid,
    Stdin,
    Control,
    Logs,
}

static FAIL_POINT: AtomicU8 = AtomicU8::new(0);

fn fail_point() -> FailPoint {
    match FAIL_POINT.load(Ordering::SeqCst) {
        1 => FailPoint::Pgid,
        2 => FailPoint::Stdin,
        3 => FailPoint::Control,
        4 => FailPoint::Logs,
        _ => FailPoint::None,
    }
}

fn clear_fail_point() {
    FAIL_POINT.store(0, Ordering::SeqCst);
}

/// Inject a post-spawn failure for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn set_spawn_fail_point(point: &'static str) {
    let v = match point {
        "pgid" => 1,
        "stdin" => 2,
        "control" => 3,
        "logs" => 4,
        _ => 0,
    };
    FAIL_POINT.store(v, Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-support"))]
static LAST_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

fn record_last_pid(pid: i32) {
    #[cfg(any(test, feature = "test-support"))]
    LAST_PID.store(pid, Ordering::SeqCst);
    let _ = pid;
}

/// Last PID claimed by the owner after `Command::spawn`.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn last_spawned_pid() -> Option<i32> {
    let pid = LAST_PID.load(Ordering::SeqCst);
    (pid > 0).then_some(pid)
}

struct SpawnHold {
    blocked: Mutex<bool>,
    waiting: AtomicBool,
    cv: Condvar,
}

static SPAWN_HOLD: Mutex<Option<Arc<SpawnHold>>> = Mutex::new(None);

fn wait_if_spawn_held() {
    let hold = SPAWN_HOLD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(hold) = hold else {
        return;
    };
    let mut blocked = hold
        .blocked
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    hold.waiting.store(true, Ordering::SeqCst);
    while *blocked {
        blocked = hold
            .cv
            .wait(blocked)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    hold.waiting.store(false, Ordering::SeqCst);
}

/// Hold the next blocking `spawn_child` until [`release_blocking_spawn`].
#[cfg(any(test, feature = "test-support"))]
pub fn hold_blocking_spawn() {
    let hold = Arc::new(SpawnHold {
        blocked: Mutex::new(true),
        waiting: AtomicBool::new(false),
        cv: Condvar::new(),
    });
    *SPAWN_HOLD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hold);
}

/// Release a held blocking spawn.
#[cfg(any(test, feature = "test-support"))]
pub fn release_blocking_spawn() {
    if let Some(hold) = SPAWN_HOLD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        *hold
            .blocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        hold.cv.notify_all();
    }
}

/// True while `spawn_child` is parked in the test hold.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn blocking_spawn_is_waiting() -> bool {
    SPAWN_HOLD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|h| h.waiting.load(Ordering::SeqCst))
}

/// Drop any spawn hold so later tests are not blocked.
#[cfg(any(test, feature = "test-support"))]
pub fn clear_blocking_spawn_hold() {
    if let Some(hold) = SPAWN_HOLD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        *hold
            .blocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        hold.cv.notify_all();
    }
}

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod coverage_tests;
