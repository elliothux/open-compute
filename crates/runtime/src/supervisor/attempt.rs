//! One runtime compile, spawn, and readiness attempt.

use super::*;

pub(super) struct AttemptArgs {
    pub(super) compiler: Arc<dyn ConfigCompiler>,
    pub(super) runtime: VerifiedRuntime,
    pub(super) token: SecretString,
    pub(super) redactor: Redactor,
    pub(super) startup_id: StartupId,
    pub(super) startup: Duration,
    pub(super) owners: OwnerRegistry,
    pub(super) external_services: Arc<[ExternalServiceAddress]>,
    pub(super) directory_services: Arc<[DirectoryServicePath]>,
    pub(super) lease_path: Option<PathBuf>,
    pub(super) host_extension_fd: Option<std::os::fd::OwnedFd>,
}

pub(super) async fn run_attempt(
    args: AttemptArgs,
    mut cancel: oneshot::Receiver<()>,
) -> AttemptOutcome {
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
        host_extension_fd,
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
            host_extension_fd: host_extension_fd.as_ref(),
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
