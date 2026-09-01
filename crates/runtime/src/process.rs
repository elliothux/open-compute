//! Bounded argv process execution with TERM/KILL/reap of the process group.

use open_compute_core::{ErrorCode, PlatformError, Redactor};
use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
use rustix::process::{
    Pid, Signal, getpgid, kill_process, kill_process_group, test_kill_process,
    test_kill_process_group,
};
use std::ffi::OsStr;
use std::fs::{self, File};
#[cfg(target_os = "macos")]
use std::io::Seek;
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::fd::{AsFd, OwnedFd};
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

const MAX_STDERR: usize = 64 * 1024;
const KILL_GRACE: Duration = Duration::from_millis(200);

/// Result of a bounded child execution. Stderr is already redacted.
#[derive(Debug)]
pub struct BoundedOutput {
    /// Process exit status, if the child exited.
    pub status: Option<std::process::ExitStatus>,
    /// Bounded stdout bytes.
    pub stdout: Vec<u8>,
    /// Bounded, redacted stderr.
    pub stderr: Vec<u8>,
    /// True if the deadline fired before exit.
    pub timed_out: bool,
    /// True if stdout exceeded the configured bound.
    pub stdout_overflow: bool,
    /// Child PID that was waited, if spawn succeeded.
    pub pid: Option<i32>,
}

/// Keep-alive executable identity used for fd-based spawn.
#[derive(Debug)]
pub(crate) struct ExecImage {
    _keep: File,
    _staging: Option<PathBuf>,
    _staging_journal: Option<PathBuf>,
    pub(crate) program: PathBuf,
}

/// Duplicate `file` without `CLOEXEC` and map it to a kernel fd path.
pub(crate) fn exec_image(file: &File) -> Result<ExecImage, PlatformError> {
    exec_image_inner(file, None)
}

/// Materialize an executable while journaling macOS staging next to `lease_path`.
pub(crate) fn exec_image_with_lease(
    file: &File,
    lease_path: &Path,
    binary_sha256: &str,
) -> Result<ExecImage, PlatformError> {
    exec_image_inner(file, Some((lease_path, binary_sha256)))
}

fn exec_image_inner(
    file: &File,
    staging_lease: Option<(&Path, &str)>,
) -> Result<ExecImage, PlatformError> {
    let owned: OwnedFd = rustix::io::dup(file.as_fd()).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to duplicate verified executable fd",
        )
    })?;
    let mut flags = fcntl_getfd(&owned).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read verified executable fd flags",
        )
    })?;
    flags.remove(FdFlags::CLOEXEC);
    fcntl_setfd(&owned, flags).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to clear CLOEXEC on verified executable fd",
        )
    })?;
    let mut keep = File::from(owned);
    let (program, staging, staging_journal) = exec_path_for(&mut keep, staging_lease)?;
    Ok(ExecImage {
        _keep: keep,
        _staging: staging,
        _staging_journal: staging_journal,
        program,
    })
}

impl Drop for ExecImage {
    fn drop(&mut self) {
        if let Some(dir) = self._staging.take() {
            cleanup_staging_dir(&dir);
        }
        if let Some(journal) = self._staging_journal.take() {
            let _ = fs::remove_file(journal);
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(clippy::unnecessary_wraps))]
fn exec_path_for(
    file: &mut File,
    staging_lease: Option<(&Path, &str)>,
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>), PlatformError> {
    #[cfg(target_os = "linux")]
    {
        let raw = file.as_raw_fd();
        let _ = staging_lease;
        Ok((PathBuf::from(format!("/proc/self/fd/{raw}")), None, None))
    }
    #[cfg(target_os = "macos")]
    {
        // posix_spawn CLOEXEC_DEFAULT drops extra fds, so /dev/fd/N cannot be
        // exec'd. Copy the already-opened vnode into a private exclusive file
        // and execute that path. This never reopens the caller pathname.
        let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
        let staging_journal = staging_lease
            .map(|(lease_path, digest)| write_staging_journal(lease_path, &staging, digest))
            .transpose()?;
        let materialized = (|| {
            fs::create_dir(&staging).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to create verified executable staging directory",
                )
            })?;
            let mut perms = fs::metadata(&staging)
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to create verified executable staging directory",
                    )
                })?
                .permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&staging, perms).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to create verified executable staging directory",
                )
            })?;
            let dest = staging.join("workerd");
            {
                let mut out = crate::fsutil::open_nofollow(&dest, true, true).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to materialize verified executable",
                    )
                })?;
                file.rewind().map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to rewind verified executable",
                    )
                })?;
                let mut buf = [0u8; 8192];
                loop {
                    let n = file.read(&mut buf).map_err(|_| {
                        PlatformError::new(
                            ErrorCode::RuntimeInvalid,
                            "failed to read verified executable",
                        )
                    })?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n]).map_err(|_| {
                        PlatformError::new(
                            ErrorCode::RuntimeInvalid,
                            "failed to materialize verified executable",
                        )
                    })?;
                }
                out.sync_all().map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to fsync verified executable",
                    )
                })?;
            }
            let mut dest_perms = fs::metadata(&dest)
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to materialize verified executable",
                    )
                })?
                .permissions();
            dest_perms.set_mode(0o700);
            fs::set_permissions(&dest, dest_perms).map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to materialize verified executable",
                )
            })?;
            Ok::<_, PlatformError>(dest)
        })();
        let dest = match materialized {
            Ok(dest) => dest,
            Err(error) => {
                if cleanup_staging_dir_strict(&staging).is_ok()
                    && let Some(journal) = &staging_journal
                {
                    let _ = fs::remove_file(journal);
                }
                return Err(error);
            }
        };
        Ok((dest, Some(staging), staging_journal))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (file, staging_lease);
        Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "fd execution is not supported on this OS",
        ))
    }
}

#[cfg(target_os = "macos")]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StagingJournal {
    schema_version: u32,
    directory: PathBuf,
    binary_sha256: String,
}

#[cfg(target_os = "macos")]
fn write_staging_journal(
    lease_path: &Path,
    directory: &Path,
    binary_sha256: &str,
) -> Result<PathBuf, PlatformError> {
    let path = staging_journal_path(lease_path);
    let bytes = serde_json::to_vec(&StagingJournal {
        schema_version: 1,
        directory: directory.to_owned(),
        binary_sha256: binary_sha256.to_owned(),
    })
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to encode runtime staging journal",
        )
    })?;
    crate::fsutil::write_atomic_replace(&path, &bytes, 0o600)?;
    Ok(path)
}

pub(crate) fn staging_journal_path(lease_path: &Path) -> PathBuf {
    lease_path.with_extension("staging")
}

pub(crate) fn clear_staging_journal(lease_path: &Path) -> Result<(), PlatformError> {
    let path = staging_journal_path(lease_path);
    crate::fsutil::remove_file_nofollow(&path)
}

#[cfg_attr(not(target_os = "macos"), allow(clippy::unnecessary_wraps))]
pub(crate) fn recover_unleased_staging(
    lease_path: &Path,
    expected_digest: &str,
) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let journal_path = staging_journal_path(lease_path);
        let Some(mut file) = crate::fsutil::open_optional_nofollow(&journal_path)? else {
            return Ok(());
        };
        let metadata = file.metadata().map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to stat runtime staging journal",
            )
        })?;
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime staging journal is not a bounded regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|_| {
            PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to read runtime staging journal",
            )
        })?;
        let journal: StagingJournal = serde_json::from_slice(&bytes).map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime staging journal is malformed",
            )
        })?;
        if journal.schema_version != 1
            || journal.binary_sha256 != expected_digest
            || !private_staging_dir(&journal.directory)
        {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "runtime staging journal does not match the verified runtime",
            ));
        }
        let executable = journal.directory.join("workerd");
        let executable = executable.canonicalize().unwrap_or(executable);
        let mut user = None;
        for _ in 0..10 {
            user = executable_user(&executable)?;
            if user.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if user.is_some() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "unleased runtime staging executable is still in use",
            ));
        }
        cleanup_staging_dir_strict(&journal.directory)?;
        clear_staging_journal(lease_path)?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (lease_path, expected_digest);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn private_staging_dir(directory: &Path) -> bool {
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(uuid) = name.strip_prefix("oc-exec-") else {
        return false;
    };
    let Some(parent) = directory.parent() else {
        return false;
    };
    uuid::Uuid::parse_str(uuid).is_ok()
        && match (parent.canonicalize(), std::env::temp_dir().canonicalize()) {
            (Ok(parent), Ok(temp)) => parent == temp,
            _ => false,
        }
}

#[cfg(target_os = "macos")]
fn executable_user(path: &Path) -> Result<Option<i32>, PlatformError> {
    if !path.exists() {
        return Ok(None);
    }
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-d", "txt", "-Fn"])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to inspect runtime staging executable ownership",
            )
        })?;
    match output.status.code() {
        Some(0) => {
            let target = path.canonicalize().map_err(|_| {
                PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to canonicalize runtime staging executable",
                )
            })?;
            let mut current_pid = None;
            let mut pids = Vec::new();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(pid) = line.strip_prefix('p') {
                    current_pid = Some(pid.parse::<i32>().map_err(|_| {
                        PlatformError::new(
                            ErrorCode::RuntimeInvalid,
                            "runtime staging ownership output was malformed",
                        )
                    })?);
                } else if let Some(name) = line.strip_prefix('n')
                    && Path::new(name).file_name() == target.file_name()
                    && Path::new(name).canonicalize().ok().as_ref() == Some(&target)
                    && let Some(pid) = current_pid
                {
                    pids.push(pid);
                }
            }
            pids.sort_unstable();
            pids.dedup();
            match pids.as_slice() {
                [] => Ok(None),
                [pid] => Ok(Some(*pid)),
                _ => Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "runtime staging executable has ambiguous ownership",
                )),
            }
        }
        Some(1) => Ok(None),
        _ => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to inspect runtime staging executable ownership",
        )),
    }
}

fn cleanup_staging_dir(directory: &Path) {
    let _ = fs::remove_file(directory.join("workerd"));
    let _ = fs::remove_dir(directory);
}

#[cfg(any(test, target_os = "macos"))]
fn cleanup_staging_dir_strict(directory: &Path) -> Result<(), PlatformError> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(_) => {}
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "failed to inspect runtime staging directory",
            ));
        }
    }
    crate::fsutil::remove_file_nofollow(&directory.join("workerd"))?;
    crate::fsutil::remove_empty_dir_nofollow(directory)
}

/// Execute the already-opened verified file. Never reopens a caller pathname.
pub(crate) async fn run_verified_fd(
    file: &File,
    args: &[&str],
    deadline: Duration,
    max_stdout: usize,
    redactor: &Redactor,
    stdout_file: Option<File>,
) -> Result<BoundedOutput, PlatformError> {
    run_exec_hook();
    let image = exec_image(file)?;
    run_image(&image, args, deadline, max_stdout, redactor, stdout_file).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_verified_fd_with_lease(
    file: &File,
    lease_path: &Path,
    binary_sha256: &str,
    args: &[&str],
    deadline: Duration,
    max_stdout: usize,
    redactor: &Redactor,
    stdout_file: Option<File>,
) -> Result<BoundedOutput, PlatformError> {
    run_exec_hook();
    let image = exec_image_with_lease(file, lease_path, binary_sha256)?;
    run_image(&image, args, deadline, max_stdout, redactor, stdout_file).await
}

async fn run_image(
    image: &ExecImage,
    args: &[&str],
    deadline: Duration,
    max_stdout: usize,
    redactor: &Redactor,
    stdout_file: Option<File>,
) -> Result<BoundedOutput, PlatformError> {
    let mut std_cmd = std::process::Command::new(&image.program);
    std::os::unix::process::CommandExt::process_group(&mut std_cmd, 0);
    std_cmd
        .args(args.iter().copied().map(OsStr::new))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = std_cmd.spawn().map_err(|_| {
        PlatformError::new(ErrorCode::RuntimeInvalid, "failed to spawn runtime process")
    })?;
    let pid = child.id() as i32;
    let mut owned = OwnedChild::new(child, pid);
    // `--version` can exit before getpgid. spawn already used process_group(0).
    // Skip only the *read* failure when the pid is already gone; leader mismatch still fails.
    match verify_self_pgid(pid) {
        Ok(()) => {}
        Err(err) => {
            if err.message() != "failed to read runtime process group" || !pid_already_gone(pid) {
                return Err(err);
            }
        }
    }
    let stdout = owned.take_stdout();
    let stderr = owned.take_stderr();

    let cancel = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = oneshot::channel();
    let started = std::time::Instant::now();
    let deadline_at = started.checked_add(deadline).ok_or_else(|| {
        PlatformError::new(ErrorCode::RuntimeInvalid, "process deadline overflow")
    })?;
    let hard_deadline = deadline_at.checked_add(KILL_GRACE).unwrap_or(deadline_at);

    let owner_cancel = cancel.clone();
    if owner_spawn_should_fail() {
        drop(owned);
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to start process owner thread",
        ));
    }
    let Ok(owner) = std::thread::Builder::new()
        .name("oc-os-owner".into())
        .spawn(move || {
            let output = owner_wait(OwnerWait {
                owned,
                stdout,
                stderr,
                stdout_file,
                max_stdout,
                cancel: owner_cancel,
                deadline_at,
                hard_deadline,
            });
            run_owner_reaped_hook();
            let _ = done_tx.send(output);
        })
    else {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to start process owner thread",
        ));
    };

    let mut guard = ProcessGuard {
        cancel: Some(cancel),
        owner: Some(owner),
    };
    let result = done_rx.await.map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime process owner task ended without a result",
        )
    })?;
    guard.disarm();
    match result {
        Ok(mut output) => {
            output.stderr = redactor.redact_bytes(output.stderr.as_slice());
            Ok(output)
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn verify_self_pgid(pid: i32) -> Result<(), PlatformError> {
    if pgid_verify_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read runtime process group",
        ));
    }
    let Some(raw) = Pid::from_raw(pid) else {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime process pid is invalid",
        ));
    };
    let pgid = getpgid(Some(raw)).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "failed to read runtime process group",
        )
    })?;
    if pgid.as_raw_nonzero().get() != pid {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime process is not its own process group leader",
        ));
    }
    Ok(())
}

/// RAII owner of a spawned process group. Drop KILL/waits unless disarmed.
struct OwnedChild {
    child: Option<std::process::Child>,
    pid: i32,
    disarmed: bool,
}

impl OwnedChild {
    fn new(child: std::process::Child, pid: i32) -> Self {
        Self {
            child: Some(child),
            pid,
            disarmed: false,
        }
    }

    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|child| child.stdout.take())
    }

    fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.as_mut().and_then(|child| child.stderr.take())
    }

    fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, PlatformError> {
        if wait_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wait for child",
            ));
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        child
            .try_wait()
            .map_err(|_| PlatformError::new(ErrorCode::RuntimeInvalid, "failed to wait for child"))
    }

    fn wait(&mut self) -> Result<Option<std::process::ExitStatus>, PlatformError> {
        if wait_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wait for child",
            ));
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        match child.wait() {
            Ok(status) => {
                self.child.take();
                Ok(Some(status))
            }
            Err(_) => Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to wait for child",
            )),
        }
    }

    fn fail_safe_kill(&mut self) {
        terminate_group_kill(Some(self.pid));
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        self.fail_safe_kill();
    }
}

struct OwnerWait {
    owned: OwnedChild,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
    stdout_file: Option<File>,
    max_stdout: usize,
    cancel: Arc<AtomicBool>,
    deadline_at: std::time::Instant,
    hard_deadline: std::time::Instant,
}

struct PipeState {
    done: Arc<AtomicBool>,
    error: Arc<Mutex<Option<PlatformError>>>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

struct ReaderDoneGuard {
    done: Arc<AtomicBool>,
}

impl Drop for ReaderDoneGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

impl PipeState {
    fn new() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
            bytes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn take_error(&self) -> Option<PlatformError> {
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn take_bytes(&self) -> Vec<u8> {
        std::mem::take(
            &mut *self
                .bytes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

fn owner_wait(
    OwnerWait {
        mut owned,
        stdout,
        stderr,
        mut stdout_file,
        max_stdout,
        cancel,
        deadline_at,
        hard_deadline,
    }: OwnerWait,
) -> Result<BoundedOutput, PlatformError> {
    let pid = owned.pid;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_state = PipeState::new();
    let stderr_state = PipeState::new();
    let stream_to_file = stdout_file.is_some();

    let stdout_thread = stdout.map(|pipe| {
        let overflow = overflow.clone();
        let state = stdout_state.done.clone();
        let error = stdout_state.error.clone();
        let bytes = stdout_state.bytes.clone();
        let file = stdout_file.take();
        std::thread::spawn(move || {
            let _done = ReaderDoneGuard { done: state };
            reader_panic_hook();
            let result = read_stdout(pipe, file, max_stdout, &overflow);
            match result {
                Ok(out) => {
                    *bytes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = out;
                }
                Err(err) => {
                    *error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(err);
                }
            }
        })
    });
    let stderr_thread = stderr.map(|pipe| {
        let state = stderr_state.done.clone();
        let error = stderr_state.error.clone();
        let bytes = stderr_state.bytes.clone();
        std::thread::spawn(move || {
            let _done = ReaderDoneGuard { done: state };
            let result = read_stderr(pipe);
            match result {
                Ok(out) => {
                    *bytes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = out;
                }
                Err(err) => {
                    *error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(err);
                }
            }
        })
    });

    let mut status = None;
    let mut timed_out = false;
    let mut sent_term = false;
    let mut term_at = None;
    let mut outcome_err = None;

    loop {
        let now = std::time::Instant::now();
        match owned.try_wait() {
            Ok(Some(s)) => status = Some(s),
            Ok(None) => {}
            Err(err) => {
                if outcome_err.is_none() {
                    outcome_err = Some(err);
                }
            }
        }
        if let Some(err) = stdout_state.take_error() {
            outcome_err = Some(err);
        }
        if outcome_err.is_none()
            && let Some(err) = stderr_state.take_error()
        {
            outcome_err = Some(err);
        }

        let overflowed = overflow.load(Ordering::SeqCst);
        let cancelled = cancel.load(Ordering::SeqCst);
        let deadline_hit = now >= deadline_at;
        let group_live = process_group_live(pid);
        let readers_done =
            stdout_state.done.load(Ordering::SeqCst) && stderr_state.done.load(Ordering::SeqCst);
        let leader_exited = status.is_some();
        let stop_live = overflowed || cancelled || deadline_hit || outcome_err.is_some();

        if leader_exited && !group_live && readers_done {
            break;
        }
        if (stop_live || (leader_exited && group_live)) && !sent_term {
            terminate_group_term(Some(pid));
            sent_term = true;
            term_at = Some(now);
            timed_out = cancelled || deadline_hit;
        }
        if sent_term {
            let grace_done = term_at
                .is_some_and(|t| now >= t.checked_add(KILL_GRACE).unwrap_or(hard_deadline))
                || now >= hard_deadline;
            if grace_done {
                terminate_group_kill(Some(pid));
                reap_after_kill(&mut owned, &mut status, &mut outcome_err);
                break;
            }
        }
        if now >= hard_deadline {
            terminate_group_kill(Some(pid));
            reap_after_kill(&mut owned, &mut status, &mut outcome_err);
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    if let Err(err) = join_readers(stdout_thread, stderr_thread, hard_deadline)
        && outcome_err.is_none()
    {
        outcome_err = Some(err);
    }
    if status.is_none() {
        match owned.wait() {
            Ok(s) => status = s,
            Err(err) => {
                if outcome_err.is_none() {
                    outcome_err = Some(err);
                }
            }
        }
    }
    if let Some(err) = stdout_state
        .take_error()
        .or_else(|| stderr_state.take_error())
        && outcome_err.is_none()
    {
        outcome_err = Some(err);
    }
    let stdout_bytes = stdout_state.take_bytes();
    let stderr_bytes = stderr_state.take_bytes();
    if let Some(err) = outcome_err {
        return Err(err);
    }
    if status.is_some() {
        let _ = owned.child.take();
    }
    owned.disarm();

    Ok(BoundedOutput {
        status,
        stdout: if stream_to_file {
            Vec::new()
        } else {
            stdout_bytes
        },
        stderr: stderr_bytes,
        timed_out,
        stdout_overflow: overflow.load(Ordering::SeqCst),
        pid: Some(pid),
    })
}

fn reap_after_kill(
    owned: &mut OwnedChild,
    status: &mut Option<std::process::ExitStatus>,
    outcome_err: &mut Option<PlatformError>,
) {
    if status.is_some() {
        return;
    }
    match owned.wait() {
        Ok(s) => *status = s,
        Err(err) => {
            if outcome_err.is_none() {
                *outcome_err = Some(err);
            }
            owned.fail_safe_kill();
        }
    }
}

fn join_readers(
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
    _hard_deadline: std::time::Instant,
) -> Result<(), PlatformError> {
    let mut panicked = false;
    if let Some(t) = stdout_thread
        && t.join().is_err()
    {
        panicked = true;
    }
    if let Some(t) = stderr_thread
        && t.join().is_err()
    {
        panicked = true;
    }
    if panicked {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime output reader panicked",
        ));
    }
    Ok(())
}

pub(crate) fn process_group_live(pid: i32) -> bool {
    let Some(raw) = Pid::from_raw(pid) else {
        return false;
    };
    test_kill_process_group(raw).is_ok()
}

fn read_stdout(
    mut pipe: std::process::ChildStdout,
    mut file: Option<File>,
    max_stdout: usize,
    overflow: &AtomicBool,
) -> Result<Vec<u8>, PlatformError> {
    let mut mem = Vec::new();
    let mut written = 0usize;
    let mut tmp = [0u8; 8192];
    loop {
        if stdout_read_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to read runtime stdout",
            ));
        }
        match pipe.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if written.saturating_add(n) > max_stdout {
                    overflow.store(true, Ordering::SeqCst);
                    let mut drain = [0u8; 8192];
                    loop {
                        match pipe.read(&mut drain) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => {
                                return Err(PlatformError::new(
                                    ErrorCode::RuntimeInvalid,
                                    "failed to read runtime stdout",
                                ));
                            }
                        }
                    }
                    break;
                }
                if let Some(out) = file.as_mut() {
                    write_compile_stdout(out, &tmp[..n])?;
                } else {
                    mem.extend_from_slice(&tmp[..n]);
                }
                written += n;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to read runtime stdout",
                ));
            }
        }
    }
    if let Some(out) = file.as_mut()
        && !overflow.load(Ordering::SeqCst)
    {
        finish_compile_stdout(out)?;
    }
    Ok(mem)
}

fn write_compile_stdout(file: &mut File, chunk: &[u8]) -> Result<(), PlatformError> {
    if stdout_write_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to write compile output",
        ));
    }
    file.write_all(chunk).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to write compile output",
        )
    })
}

fn finish_compile_stdout(file: &mut File) -> Result<(), PlatformError> {
    if stdout_flush_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to flush compile output",
        ));
    }
    file.flush().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to flush compile output",
        )
    })?;
    if stdout_sync_should_fail() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to fsync compile output",
        ));
    }
    file.sync_all().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "failed to fsync compile output",
        )
    })
}

fn read_stderr(mut pipe: std::process::ChildStderr) -> Result<Vec<u8>, PlatformError> {
    let mut err = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if stderr_read_should_fail() {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to read runtime stderr",
            ));
        }
        match pipe.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if err.len() < MAX_STDERR {
                    let take = (MAX_STDERR - err.len()).min(n);
                    err.extend_from_slice(&tmp[..take]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeInvalid,
                    "failed to read runtime stderr",
                ));
            }
        }
    }
    Ok(err)
}

struct ProcessGuard {
    cancel: Option<Arc<AtomicBool>>,
    owner: Option<std::thread::JoinHandle<()>>,
}

impl ProcessGuard {
    fn disarm(&mut self) {
        self.cancel.take();
        let _ = self.owner.take();
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let Some(cancel) = self.cancel.take() else {
            return;
        };
        cancel.store(true, Ordering::SeqCst);
        let _ = self.owner.take();
    }
}

pub(crate) fn terminate_group_term(pid: Option<i32>) {
    if let Some(raw) = pid
        && let Some(pid) = Pid::from_raw(raw)
    {
        record_signal(raw, "TERM");
        let _ = kill_process_group(pid, Signal::TERM);
    }
}

pub(crate) fn terminate_group_kill(pid: Option<i32>) {
    if let Some(raw) = pid
        && let Some(pid) = Pid::from_raw(raw)
    {
        record_signal(raw, "KILL");
        let _ = kill_process_group(pid, Signal::KILL);
        let _ = kill_process(pid, Signal::KILL);
    }
}

#[cfg(any(test, feature = "test-support"))]
static SIGNAL_LOG: Mutex<Vec<(i32, &'static str)>> = Mutex::new(Vec::new());

pub(crate) fn record_kill_target(pid: i32) {
    record_signal(pid, "KILL");
}

fn record_signal(pid: i32, kind: &'static str) {
    #[cfg(any(test, feature = "test-support"))]
    {
        SIGNAL_LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((pid, kind));
    }
    let _ = (pid, kind);
}

/// Recorded TERM/KILL targets. Test-support only.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn take_signal_log() -> Vec<(i32, &'static str)> {
    std::mem::take(
        &mut *SIGNAL_LOG
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Clear recorded TERM/KILL targets.
#[cfg(any(test, feature = "test-support"))]
pub fn clear_signal_log() {
    SIGNAL_LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Assert a PID and its process group are gone.
pub fn assert_reaped(pid: Option<i32>) -> Result<(), PlatformError> {
    #[cfg(any(test, feature = "test-support"))]
    if REAP_PROBE_FAIL.load(Ordering::SeqCst) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "injected child reap probe failure",
        ));
    }
    let Some(raw) = pid else {
        return Ok(());
    };
    let Some(pid) = Pid::from_raw(raw) else {
        return Ok(());
    };
    match test_kill_process(pid) {
        Err(err) if err == rustix::io::Errno::SRCH => {}
        Ok(()) => {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process was not reaped",
            ));
        }
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process state could not be verified",
            ));
        }
    }
    match test_kill_process_group(pid) {
        Err(err) if err == rustix::io::Errno::SRCH => Ok(()),
        Ok(()) => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "child process group was not reaped",
        )),
        Err(_) => Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "child process group state could not be verified",
        )),
    }
}

/// Inject a fail-closed reap probe error. Test-support only.
#[cfg(any(test, feature = "test-support"))]
pub fn set_reap_probe_fail(fail: bool) {
    REAP_PROBE_FAIL.store(fail, Ordering::SeqCst);
}

fn pid_already_gone(pid: i32) -> bool {
    let Some(raw) = Pid::from_raw(pid) else {
        return true;
    };
    matches!(test_kill_process(raw), Err(err) if err == rustix::io::Errno::SRCH)
}

/// Wait until `pid` is gone or `deadline` elapses.
pub fn wait_pid_gone(pid: i32, deadline: Duration) -> Result<(), PlatformError> {
    let started = std::time::Instant::now();
    let Some(raw) = Pid::from_raw(pid) else {
        return Ok(());
    };
    loop {
        match test_kill_process(raw) {
            Err(err) if err == rustix::io::Errno::SRCH => return Ok(()),
            _ => {}
        }
        if started.elapsed() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process was not reaped",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait until `pid` and its process group are gone or `deadline` elapses.
pub fn wait_reaped(pid: i32, deadline: Duration) -> Result<(), PlatformError> {
    let started = std::time::Instant::now();
    loop {
        if assert_reaped(Some(pid)).is_ok() {
            return Ok(());
        }
        if started.elapsed() >= deadline {
            return Err(PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "child process was not reaped",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
static EXEC_HOOK: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

#[cfg(test)]
static OWNER_REAPED_HOOK: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

#[cfg(test)]
static OWNER_SPAWN_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static OWNER_SPAWN_HOOK: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>> = Mutex::new(None);

#[cfg(test)]
static STDOUT_READ_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static STDERR_READ_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static STDOUT_WRITE_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static STDOUT_FLUSH_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static STDOUT_SYNC_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static PGID_VERIFY_HOOK: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>> = Mutex::new(None);

#[cfg(any(test, feature = "test-support"))]
static REAP_PROBE_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static WAIT_FAIL: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static WAIT_FAIL_HOOK: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>> = Mutex::new(None);

#[cfg(test)]
static READER_PANIC: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn set_exec_hook(hook: impl Fn() + Send + Sync + 'static) {
    *EXEC_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_exec_hook() {
    *EXEC_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn run_exec_hook() {
    #[cfg(test)]
    {
        if let Some(hook) = EXEC_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            hook();
        }
    }
}

fn run_owner_reaped_hook() {
    #[cfg(test)]
    {
        if let Some(hook) = OWNER_REAPED_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            hook();
        }
    }
}

fn owner_spawn_should_fail() -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = OWNER_SPAWN_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return hook();
        }
        OWNER_SPAWN_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn stdout_read_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_READ_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn stderr_read_should_fail() -> bool {
    #[cfg(test)]
    {
        STDERR_READ_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn stdout_write_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_WRITE_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn stdout_flush_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_FLUSH_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn stdout_sync_should_fail() -> bool {
    #[cfg(test)]
    {
        STDOUT_SYNC_FAIL.swap(false, Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn pgid_verify_should_fail() -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = PGID_VERIFY_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return hook();
        }
        false
    }
    #[cfg(not(test))]
    false
}

fn wait_should_fail() -> bool {
    #[cfg(test)]
    {
        if let Some(hook) = WAIT_FAIL_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return hook();
        }
        WAIT_FAIL.load(Ordering::SeqCst)
    }
    #[cfg(not(test))]
    false
}

fn reader_panic_hook() {
    #[cfg(test)]
    {
        if READER_PANIC.swap(false, Ordering::SeqCst) {
            panic!("test stdout reader panic");
        }
    }
}

#[cfg(test)]
pub(crate) fn set_owner_reaped_hook(hook: impl Fn() + Send + Sync + 'static) {
    *OWNER_REAPED_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_owner_reaped_hook() {
    *OWNER_REAPED_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
pub(crate) fn set_owner_spawn_fail_hook(hook: impl Fn() -> bool + Send + Sync + 'static) {
    *OWNER_SPAWN_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_owner_spawn_fail_hook() {
    *OWNER_SPAWN_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    OWNER_SPAWN_FAIL.store(false, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn set_stdout_write_fail(fail: bool) {
    STDOUT_WRITE_FAIL.store(fail, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn set_pgid_verify_fail_hook(hook: impl Fn() -> bool + Send + Sync + 'static) {
    *PGID_VERIFY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn clear_pgid_verify_fail_hook() {
    *PGID_VERIFY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
pub(crate) fn set_wait_fail_hook(hook: impl Fn() -> bool + Send + Sync + 'static) {
    *WAIT_FAIL_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
}

#[cfg(test)]
pub(crate) fn set_reader_panic(fail: bool) {
    READER_PANIC.store(fail, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn set_stdout_read_fail(fail: bool) {
    STDOUT_READ_FAIL.store(fail, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn clear_io_fail_hooks() {
    OWNER_SPAWN_FAIL.store(false, Ordering::SeqCst);
    *OWNER_SPAWN_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    STDOUT_READ_FAIL.store(false, Ordering::SeqCst);
    STDERR_READ_FAIL.store(false, Ordering::SeqCst);
    STDOUT_WRITE_FAIL.store(false, Ordering::SeqCst);
    STDOUT_FLUSH_FAIL.store(false, Ordering::SeqCst);
    STDOUT_SYNC_FAIL.store(false, Ordering::SeqCst);
    WAIT_FAIL.store(false, Ordering::SeqCst);
    *WAIT_FAIL_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    READER_PANIC.store(false, Ordering::SeqCst);
    *PGID_VERIFY_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

#[cfg(test)]
#[test]
fn fallback_skips_signal_after_owner_reaps() {
    let mut cmd = std::process::Command::new("/bin/sleep");
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd
        .arg("30")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let pid = child.id() as i32;
    let mut owned = OwnedChild::new(child, pid);
    owned.fail_safe_kill();
    owned.disarm();
    drop(owned);
    wait_reaped(pid, Duration::from_secs(2)).expect("owner reaped without a fallback signal");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_recovers_interrupted_copy_without_child_lease() {
    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).expect("create interrupted staging directory");
    fs::write(staging.join("workerd"), b"partial verified executable copy")
        .expect("write interrupted copy");
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover interrupted staging");

    assert!(!staging.exists(), "interrupted staging directory leaked");
    assert!(!journal.exists(), "staging journal leaked");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_recovers_crash_before_directory_creation() {
    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    assert!(!staging.exists());
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover empty staging journal");

    assert!(!staging.exists());
    assert!(!journal.exists(), "empty staging journal leaked");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn complete_staging_without_child_lease_is_recovered() {
    use sha2::Digest as _;

    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).expect("create complete staging directory");
    let executable = staging.join("workerd");
    let bytes = b"complete verified executable copy";
    fs::write(&executable, bytes).expect("write complete copy");
    let digest = hex::encode(sha2::Sha256::digest(bytes));
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover complete staging");

    assert!(!staging.exists(), "complete unleased staging leaked");
    assert!(
        !journal.exists(),
        "complete unleased staging journal leaked"
    );
}

#[cfg(test)]
#[test]
fn process_helpers_fail_closed_on_absent_and_invalid_processes() {
    let data = tempfile::TempDir::new().expect("temporary data");
    let lease = data.path().join("child.lease");
    assert_eq!(
        staging_journal_path(&lease),
        data.path().join("child.staging")
    );
    clear_staging_journal(&lease).expect("missing journal is already clear");
    cleanup_staging_dir_strict(&data.path().join("missing")).expect("missing staging is clear");

    assert!(!process_group_live(0));
    assert_reaped(None).expect("no pid is reaped");
    assert_reaped(Some(0)).expect("invalid pid is treated as absent");
    wait_pid_gone(0, Duration::ZERO).expect("invalid pid is absent");
    terminate_group_term(None);
    terminate_group_term(Some(0));
    terminate_group_kill(None);
    terminate_group_kill(Some(0));

    let pipe = PipeState::new();
    assert!(pipe.take_error().is_none());
    assert!(pipe.take_bytes().is_empty());
    join_readers(None, None, std::time::Instant::now()).expect("no readers");
    let panicking = std::thread::spawn(|| panic!("reader failure"));
    assert!(join_readers(Some(panicking), None, std::time::Instant::now()).is_err());

    let mut owned = OwnedChild {
        child: None,
        pid: 0,
        disarmed: false,
    };
    assert!(owned.take_stdout().is_none());
    assert!(owned.take_stderr().is_none());
    assert!(owned.try_wait().expect("empty owner").is_none());
    assert!(owned.wait().expect("empty owner").is_none());
    let mut status = Some(
        std::process::Command::new("/usr/bin/true")
            .status()
            .unwrap(),
    );
    let mut error = None;
    reap_after_kill(&mut owned, &mut status, &mut error);
    assert!(error.is_none());
    owned.disarm();

    let mut guard = ProcessGuard {
        cancel: None,
        owner: None,
    };
    guard.disarm();
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_validation_matrix_is_fail_closed() {
    let data = tempfile::TempDir::new().expect("temporary data");
    let lease = data.path().join("child.lease");
    let journal = staging_journal_path(&lease);
    let digest = "ab".repeat(32);

    assert!(!private_staging_dir(Path::new("relative")));
    assert!(!private_staging_dir(Path::new("/")));
    assert!(!private_staging_dir(
        &std::env::temp_dir().join("wrong-prefix")
    ));
    assert!(!private_staging_dir(
        &std::env::temp_dir().join("oc-exec-not-a-uuid")
    ));
    assert_eq!(
        executable_user(&data.path().join("missing")).expect("missing executable"),
        None
    );

    fs::write(&journal, b"not json").unwrap();
    assert!(recover_unleased_staging(&lease, &digest).is_err());
    fs::write(&journal, vec![b'x'; 4097]).unwrap();
    assert!(recover_unleased_staging(&lease, &digest).is_err());

    for body in [
        serde_json::json!({
            "schemaVersion": 2,
            "directory": std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7())),
            "binarySha256": digest,
        }),
        serde_json::json!({
            "schemaVersion": 1,
            "directory": std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7())),
            "binarySha256": "cd".repeat(32),
        }),
        serde_json::json!({
            "schemaVersion": 1,
            "directory": data.path().join("not-private"),
            "binarySha256": digest,
        }),
    ] {
        fs::write(&journal, serde_json::to_vec(&body).unwrap()).unwrap();
        assert!(recover_unleased_staging(&lease, &digest).is_err());
    }
    fs::remove_file(&journal).unwrap();

    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("unexpected"), b"keep").unwrap();
    assert!(cleanup_staging_dir_strict(&staging).is_err());
    fs::remove_file(staging.join("unexpected")).unwrap();
    fs::remove_dir(&staging).unwrap();
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod coverage_tests;
