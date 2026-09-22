//! Supervised long-lived execution of an already-opened host executable.

use crate::process::{
    ExecImage, VerifiedLaunchImage, exec_image_with_lease, terminate_group_kill, verify_self_pgid,
};
use crate::supervisor::logs::LogCollector;
use crate::supervisor::owner::{ChildHandle, OwnerCompletion, OwnerRegistry};
use command_fds::{CommandFdExt, FdMapping};
use open_compute_core::{ErrorCode, PlatformError, Redactor};
use std::ffi::OsString;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// Exact launch inputs for one supervised long-lived host child.
#[derive(Debug)]
pub struct PersistentHostProcessSpec {
    /// Argument vector excluding argv[0].
    pub args: Vec<OsString>,
    /// Complete environment; the parent environment is always cleared.
    pub environment: Vec<(OsString, OsString)>,
    /// Private working directory selected by the owning domain.
    pub working_directory: PathBuf,
    /// Optional private control socket mapped to fd 0. Without one, stdin is null.
    pub control_fd: Option<OwnedFd>,
    /// Private crash-recovery lease path.
    pub lease_path: PathBuf,
    /// SHA-256 of the exact opened executable.
    pub binary_sha256: String,
    /// Exact-value redactor applied to bounded child logs.
    pub redactor: Redactor,
}

/// Unique owner of a long-lived verified host process group.
pub struct PersistentHostProcess {
    handle: Option<ChildHandle>,
    lease_path: PathBuf,
    _image: ExecImage,
}

/// Sanitized completion state for a reaped persistent host process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistentHostProcessOutcome {
    /// Normal process exit code, when available.
    pub exit_code: Option<i32>,
    /// Terminating signal, when available.
    pub signal: Option<i32>,
    /// Whether either bounded log reader failed.
    pub reader_failed: bool,
}

impl std::fmt::Debug for PersistentHostProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersistentHostProcess")
            .field("running", &self.is_running())
            .finish_non_exhaustive()
    }
}

impl PersistentHostProcess {
    /// Spawn the exact opened executable as its own supervised process group.
    pub fn spawn(
        image: &VerifiedLaunchImage,
        spec: PersistentHostProcessSpec,
    ) -> Result<Self, PlatformError> {
        let image = exec_image_with_lease(&image.file, &spec.lease_path, &spec.binary_sha256)?;
        let mut command = std::process::Command::new(&image.program);
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command
            .args(spec.args)
            .env_clear()
            .envs(spec.environment)
            .current_dir(spec.working_directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(control_fd) = spec.control_fd {
            command
                .fd_mappings(vec![FdMapping {
                    parent_fd: control_fd,
                    child_fd: 0,
                }])
                .map_err(|_| invalid())?;
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command.spawn().map_err(|_| invalid())?;
        let pid = i32::try_from(child.id()).map_err(|_| invalid())?;
        if verify_self_pgid(pid).is_err() {
            terminate_group_kill(Some(pid));
            let _ = child.wait();
            return Err(invalid());
        }
        let Some(lease) = crate::lease::capture_lease(pid, pid, &spec.binary_sha256) else {
            terminate_group_kill(Some(pid));
            let _ = child.wait();
            return Err(invalid());
        };
        if let Err(error) = crate::lease::write_lease(&spec.lease_path, &lease) {
            terminate_group_kill(Some(pid));
            let _ = child.wait();
            return Err(error);
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let owners = OwnerRegistry::default();
        let handle = ChildHandle::start(
            child,
            pid,
            pid,
            stdout,
            stderr,
            LogCollector::new(spec.redactor.clone()),
            LogCollector::new(spec.redactor),
            &owners,
        )
        .inspect_err(|_| {
            let _ = crate::lease::clear_lease(&spec.lease_path);
        })?;
        Ok(Self {
            handle: Some(handle),
            lease_path: spec.lease_path,
            _image: image,
        })
    }

    /// Recover only an orphan matching the exact lease and executable digest.
    pub fn recover_orphan(
        lease_path: &std::path::Path,
        binary_sha256: &str,
    ) -> Result<(), PlatformError> {
        crate::lease::recover_orphans(lease_path, binary_sha256).map(|_| ())
    }

    /// Recover an orphan for a removed extension from its protected lease identity.
    pub fn recover_recorded_orphan(lease_path: &std::path::Path) -> Result<(), PlatformError> {
        crate::lease::recover_recorded_orphan(lease_path).map(|_| ())
    }

    /// Return whether the supervised leader is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(ChildHandle::leader_alive)
    }

    /// Return the supervised leader PID for peer-credential admission checks.
    /// The owner handle remains responsible for signaling and reaping the process.
    #[must_use]
    pub fn pid(&self) -> i32 {
        self.handle.as_ref().map_or(0, |handle| handle.pid)
    }

    /// Wait until the process group owner has reaped an exited child.
    pub async fn wait_exited(&self) {
        if let Some(handle) = &self.handle {
            handle.wait_reaped().await;
        }
    }

    /// Stop, force-stop if needed, and reap the complete process group.
    pub async fn shutdown(
        mut self,
        grace: Duration,
        kill_after: Duration,
    ) -> PersistentHostProcessOutcome {
        let completion = if let Some(handle) = self.handle.take() {
            Some(handle.shutdown(grace, kill_after).await)
        } else {
            None
        };
        let _ = crate::lease::clear_lease(&self.lease_path);
        PersistentHostProcessOutcome {
            exit_code: completion.as_ref().and_then(OwnerCompletion::exit_code),
            signal: completion.as_ref().and_then(OwnerCompletion::signal),
            reader_failed: completion.is_some_and(|value| value.reader_failed),
        }
    }
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "failed to start supervised host process",
    )
}
