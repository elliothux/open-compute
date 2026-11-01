//! One-shot Python preparation using the runtime's sole process-group owner.

use super::attempt::{ChildStartOptions, spawn_ready};
use super::*;
use crate::compile::ConfigRole;
use crate::fsutil::{open_dir, require_absolute};
use std::os::fd::OwnedFd;

/// Exact host-owned inputs for an isolated preparation process.
///
/// The caller holds its instance's preparation lock through shutdown and selects a
/// private lease distinct from the instance runtime and compiler leases.
pub struct PythonPreparationOptions {
    /// Verified runtime executable; no PATH lookup or runtime download is allowed.
    pub runtime: VerifiedRuntime,
    /// Verified binary config compiled from the Python preparation profile.
    pub compiled: CompiledConfig,
    /// Bound on control-fd listen evidence and the Unix HTTP readiness probe.
    pub startup_timeout: Duration,
    /// Absolute private crash-recovery lease, exclusively owned by the caller.
    pub lease_path: PathBuf,
    /// Exactly the instance Source, binding/observability backends and DO router.
    pub external_services: Vec<ExternalServiceAddress>,
    /// Optional independent broker socket for the instance's existing extension authority.
    pub host_extension_fd: Option<OwnedFd>,
}

impl Debug for PythonPreparationOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonPreparationOptions")
            .finish_non_exhaustive()
    }
}

/// A ready preparation child with unique process-group ownership and no DO storage.
///
/// Dropping this handle or cancelling startup still commands the existing owner to
/// stop and reap the group. An unconfirmed reap retains the lease for next-start recovery.
pub struct PythonPreparationProcess {
    live: LiveRuntime,
    lease_path: PathBuf,
}

impl Debug for PythonPreparationProcess {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonPreparationProcess")
            .finish_non_exhaustive()
    }
}

impl PythonPreparationProcess {
    /// Recover a prior orphaned process group, spawn once, and require both readiness proofs.
    /// No restart budget, authority registration, or generation rotation is performed.
    pub async fn start(options: PythonPreparationOptions) -> Result<Self, PlatformError> {
        validate_options(&options)?;
        let lease = options.lease_path.clone();
        tokio::task::spawn_blocking(move || recover_orphans(&lease))
            .await
            .map_err(|_| invalid())??;
        let lease_path = options.lease_path;
        let compiled = options.compiled;
        let (cancel, cancelled) = oneshot::channel();
        let task = tokio::spawn(spawn_ready(
            ChildStartOptions {
                runtime: options.runtime,
                startup: options.startup_timeout,
                owners: OwnerRegistry::default(),
                external_services: options.external_services.into(),
                directory_services: Arc::from([]),
                lease_path: Some(lease_path.clone()),
                host_extension_fd: options.host_extension_fd,
            },
            compiled,
            cancelled,
        ));
        // Dropping this sender on cancellation wakes the detached startup task. That
        // task waits even for an in-progress blocking spawn before stopping its child.
        let outcome = task.await.map_err(|_| invalid())?;
        drop(cancel);
        match outcome {
            AttemptOutcome::Ready(live) => Ok(Self {
                live: *live,
                lease_path,
            }),
            AttemptOutcome::Failed(failure) => Err(failure.error),
            AttemptOutcome::Cancelled => Err(invalid()),
        }
    }

    /// Private Unix socket for the host's bounded preparation request.
    #[must_use]
    pub fn ingress_socket(&self) -> &Path {
        &self.live.socket_path
    }

    /// Gracefully stop, force-stop when needed, prove reaping, and clear the private lease.
    pub async fn shutdown(
        self,
        grace: Duration,
        kill_after: Duration,
    ) -> Result<(), PlatformError> {
        let pid = self.live.pid();
        let completion = self.live.shutdown(grace, kill_after).await;
        wait_reaped(pid, Duration::from_secs(2))?;
        clear_lease(&self.lease_path)?;
        if completion.reader_failed {
            return Err(invalid());
        }
        Ok(())
    }
}

fn validate_options(options: &PythonPreparationOptions) -> Result<(), PlatformError> {
    if options.compiled.role() != ConfigRole::PythonPreparation
        || options.startup_timeout.is_zero()
        || std::time::Instant::now()
            .checked_add(options.startup_timeout)
            .is_none()
        || options.external_services.len() != 4
        || [
            "runtime-source",
            "binding-backend",
            "observability-backend",
            "do-router",
        ]
        .iter()
        .any(|required| {
            options
                .external_services
                .iter()
                .filter(|service| service.name == *required)
                .count()
                != 1
        })
    {
        return Err(invalid());
    }
    require_absolute(&options.lease_path)?;
    let parent = options.lease_path.parent().ok_or_else(invalid)?;
    let _ = open_dir(parent)?;
    Ok(())
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "Python preparation process is unavailable",
    )
}
