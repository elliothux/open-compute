//! Bounded argv process execution with TERM/KILL/reap of the process group.

use open_compute_core::{ErrorCode, PlatformError, Redactor};
use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
use rustix::process::{
    Pid, Signal, getpgid, kill_process_group, test_kill_process, test_kill_process_group,
};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::fd::{AsFd, OwnedFd};
#[cfg(target_os = "macos")]
use std::os::unix::fs::{FileExt, PermissionsExt};
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
    /// True if stderr exceeded the configured bound.
    pub stderr_overflow: bool,
    /// True if writing the complete stdin payload failed.
    pub stdin_error: bool,
    /// Child PID that was waited, if spawn succeeded.
    pub pid: Option<i32>,
}

/// An already-opened executable whose identity was verified by its owning domain.
#[derive(Debug)]
pub struct VerifiedLaunchImage {
    pub(crate) file: File,
    pub(crate) resources: Vec<(String, File)>,
}

impl VerifiedLaunchImage {
    /// Transfer an already-opened, verified executable into the process runtime.
    #[must_use]
    pub const fn from_verified_file(file: File) -> Self {
        Self {
            file,
            resources: Vec::new(),
        }
    }
}

/// Exact launch inputs for one bounded host child execution.
#[derive(Debug)]
pub struct HostProcessSpec {
    /// Argument vector excluding argv[0].
    pub args: Vec<OsString>,
    /// Complete environment; the parent environment is always cleared.
    pub environment: Vec<(OsString, OsString)>,
    /// Private working directory selected by the owning domain.
    pub working_directory: PathBuf,
    /// Complete stdin payload written before the pipe is closed.
    pub stdin: Vec<u8>,
    /// Maximum lifetime before TERM/KILL/reap.
    pub deadline: Duration,
    /// Maximum captured stdout bytes.
    pub max_stdout: usize,
    /// Maximum captured stderr bytes.
    pub max_stderr: usize,
    /// Exact-value redactor applied to captured stderr.
    pub redactor: Redactor,
    /// Optional private lease for verified recovery after owner crash.
    pub lease: Option<PathBuf>,
}

/// Run one verified executable with explicit environment, cwd, stdio and lifecycle bounds.
pub async fn run_host_process(
    image: &VerifiedLaunchImage,
    spec: HostProcessSpec,
) -> Result<BoundedOutput, PlatformError> {
    run_exec_hook();
    if let Some(lease) = &spec.lease
        && (lease.parent() != Some(spec.working_directory.as_path()) || !lease.is_absolute())
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "bounded child lease must be inside its private working directory",
        ));
    }
    let launch = if let Some(lease) = &spec.lease {
        exec_image_with_lease(&image.file, lease)?
    } else {
        exec_image_for_task(&image.file, &spec.working_directory)?
    };
    launch.stage_resources(&image.resources)?;
    run_image(
        &launch,
        RunImageSpec {
            args: &spec.args,
            deadline: spec.deadline,
            max_stdout: spec.max_stdout,
            max_stderr: spec.max_stderr,
            redactor: &spec.redactor,
            stdout_file: None,
            working_directory: Some(&spec.working_directory),
            environment: &spec.environment,
            stdin_bytes: spec.stdin,
            lease: spec.lease.as_deref(),
        },
    )
    .await
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
    exec_image_inner(file, None, None)
}

fn exec_image_for_task(file: &File, working_directory: &Path) -> Result<ExecImage, PlatformError> {
    exec_image_inner(file, None, Some(working_directory))
}

/// Materialize an executable while journaling macOS staging next to `lease_path`.
pub(crate) fn exec_image_with_lease(
    file: &File,
    lease_path: &Path,
) -> Result<ExecImage, PlatformError> {
    exec_image_inner(file, Some(lease_path), None)
}

fn exec_image_inner(
    file: &File,
    staging_lease: Option<&Path>,
    working_directory: Option<&Path>,
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
    let (program, staging, staging_journal) =
        exec_path_for(&mut keep, staging_lease, working_directory)?;
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

fn exec_path_for(
    file: &mut File,
    staging_lease: Option<&Path>,
    working_directory: Option<&Path>,
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>), PlatformError> {
    #[cfg(target_os = "linux")]
    {
        let raw = file.as_raw_fd();
        let _ = (staging_lease, working_directory);
        Ok((PathBuf::from(format!("/proc/self/fd/{raw}")), None, None))
    }
    #[cfg(target_os = "macos")]
    {
        // posix_spawn CLOEXEC_DEFAULT drops extra fds, so /dev/fd/N cannot be
        // exec'd. Copy the already-opened vnode into a private exclusive file
        // and execute that path. This never reopens the caller pathname.
        let staging_parent = if let Some(lease_path) = staging_lease {
            let parent = lease_path.parent().ok_or_else(|| {
                PlatformError::new(ErrorCode::PathInvalid, "runtime lease has no parent")
            })?;
            let root = parent.join("staging");
            crate::fsutil::create_dir_secure(&root)?;
            root
        } else if let Some(working_directory) = working_directory {
            let tmp = working_directory.join("tmp");
            crate::fsutil::create_dir_secure(&tmp)?;
            let root = tmp.join("staging");
            crate::fsutil::create_dir_secure(&root)?;
            root
        } else {
            #[cfg(any(test, feature = "test-support"))]
            {
                std::env::temp_dir()
            }
            #[cfg(not(any(test, feature = "test-support")))]
            {
                return Err(PlatformError::new(
                    ErrorCode::PathInvalid,
                    "verified executable requires an owned staging root",
                ));
            }
        };
        let staging = staging_parent.join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
        let staging_journal = staging_lease
            .map(|lease_path| write_staging_journal(lease_path, &staging))
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
                let mut out = crate::fsutil::open_file(&dest, true, true).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to materialize verified executable",
                    )
                })?;
                let mut buf = [0u8; 8192];
                // Duplicated verified fds share a cursor across concurrent launches.
                let mut offset = 0;
                loop {
                    let n = file.read_at(&mut buf, offset).map_err(|_| {
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
                    offset += n as u64;
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
        let _ = (file, staging_lease, working_directory);
        Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "fd execution is not supported on this OS",
        ))
    }
}

#[cfg(target_os = "macos")]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagingJournal {
    directory: PathBuf,
}

#[cfg(target_os = "macos")]
fn write_staging_journal(lease_path: &Path, directory: &Path) -> Result<PathBuf, PlatformError> {
    let path = staging_journal_path(lease_path);
    let bytes = serde_json::to_vec(&StagingJournal {
        directory: directory.to_owned(),
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

pub(crate) fn recover_unleased_staging(lease_path: &Path) -> Result<(), PlatformError> {
    #[cfg(target_os = "macos")]
    {
        let journal_path = staging_journal_path(lease_path);
        if let Some(file) = crate::fsutil::open_optional_file(&journal_path)? {
            let journal: StagingJournal =
                serde_json::from_reader(file.take(4097)).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "runtime staging journal is malformed",
                    )
                })?;
            match fs::remove_dir_all(&journal.directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(PlatformError::new(
                        ErrorCode::RuntimeInvalid,
                        "failed to remove runtime staging directory",
                    ));
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = lease_path;
    clear_staging_journal(lease_path)
}

fn cleanup_staging_dir(directory: &Path) {
    #[cfg(any(test, target_os = "macos"))]
    let _ = cleanup_staging_dir_strict(directory);
    #[cfg(not(any(test, target_os = "macos")))]
    let _ = directory;
}

#[cfg(any(test, target_os = "macos"))]
pub(crate) fn cleanup_staging_dir_strict(directory: &Path) -> Result<(), PlatformError> {
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
    crate::launch_resources::remove_staged_files(directory)?;
    crate::fsutil::remove_empty_dir_nofollow(directory)
}

/// Execute the already-opened verified file. Never reopens a caller pathname.
mod execution;
mod signals;

pub(crate) use execution::*;
pub use signals::*;

#[cfg(test)]
#[path = "process_tests.rs"]
mod coverage_tests;
