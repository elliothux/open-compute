//! Shared wait and task-completion handling for daemon and instance lifecycles.

use super::*;

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub(super) async fn wait_instance_and_servers(
    health: &HealthCoordinator,
    supervisor: &WorkerdSupervisor,
    mut daemon_shutdown: watch::Receiver<bool>,
    mut instance_shutdown: watch::Receiver<bool>,
    route_lease: http::RouteLease,
    shutdown_tx: watch::Sender<bool>,
    scheduler_shutdown_tx: watch::Sender<bool>,
    runtime_source_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    binding_backend_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    observability_backend_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    host_extension_broker_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    control_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    maintenance_task: tokio::task::JoinHandle<Result<(), PlatformError>>,
    scheduler_task: Option<tokio::task::JoinHandle<Result<(), PlatformError>>>,
) -> Option<PlatformError> {
    let mut runtime_source_task = runtime_source_task;
    let mut binding_backend_task = binding_backend_task;
    let mut observability_backend_task = observability_backend_task;
    let mut host_extension_broker_task = host_extension_broker_task;
    let mut control_task = control_task;
    let mut maintenance_task = maintenance_task;
    let mut scheduler_task = scheduler_task;
    let mut listener_error = None;
    'wait: loop {
        if *daemon_shutdown.borrow() || *instance_shutdown.borrow() {
            break 'wait;
        }
        tokio::select! {
            biased;
            _ = daemon_shutdown.changed() => break 'wait,
            _ = instance_shutdown.changed() => break 'wait,
            res = &mut runtime_source_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut binding_backend_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut observability_backend_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut host_extension_broker_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut control_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = &mut maintenance_task => {
                listener_error = Some(join_runtime_source(res));
                break 'wait;
            }
            res = async {
                match scheduler_task.as_mut() {
                    Some(task) => task.await,
                    None => std::future::pending().await,
                }
            } => {
                let error = join_scheduler(res);
                tracing::error!(code = error.code().as_str(), "scheduler task stopped");
                let _ = health.set_component(
                    ComponentName::Scheduler,
                    ComponentState::Failed,
                    Some(ReadinessReason::SchedulerUnavailable),
                );
                scheduler_task = None;
            }
        }
    }
    route_lease.withdraw();
    let _ = health.begin_drain();
    let _ = scheduler_shutdown_tx.send(true);
    if let Some(task) = scheduler_task
        && !task.is_finished()
    {
        let _ = task.await;
    }
    supervisor.begin_drain();
    let _ = shutdown_tx.send(true);
    if !control_task.is_finished() {
        let _ = control_task.await;
    }
    supervisor.shutdown().await;
    if !runtime_source_task.is_finished() {
        let _ = runtime_source_task.await;
    }
    if !binding_backend_task.is_finished() {
        let _ = binding_backend_task.await;
    }
    if !observability_backend_task.is_finished() {
        let _ = observability_backend_task.await;
    }
    if !host_extension_broker_task.is_finished() {
        let _ = host_extension_broker_task.await;
    }
    if !maintenance_task.is_finished() {
        let _ = maintenance_task.await;
    }
    listener_error
}

pub(crate) fn join_scheduler(
    res: Result<Result<(), PlatformError>, tokio::task::JoinError>,
) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(
            ErrorCode::SchedulerUnavailable,
            "scheduler task stopped unexpectedly",
        ),
        Ok(Err(error)) => error,
        Err(_) => PlatformError::new(ErrorCode::SchedulerUnavailable, "scheduler task failed"),
    }
}

pub(crate) fn join_listener(
    res: Result<Result<(), PlatformError>, tokio::task::JoinError>,
) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(ErrorCode::ConfigInvalid, "health listener failed"),
        Ok(Err(err)) => err,
        Err(_) => PlatformError::new(ErrorCode::ConfigInvalid, "health listener failed"),
    }
}

pub(crate) fn join_runtime_source(
    res: Result<Result<(), PlatformError>, tokio::task::JoinError>,
) -> PlatformError {
    match res {
        Ok(Ok(())) => PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "private RuntimeSource listener stopped unexpectedly",
        ),
        Ok(Err(err)) => err,
        Err(_) => PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "private RuntimeSource listener task failed",
        ),
    }
}

pub(super) async fn wait_for_supervisor_running(
    supervisor: &WorkerdSupervisor,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut watch_rx = supervisor.subscribe();
    loop {
        if watch_rx.borrow().state == SupervisorState::Running {
            return true;
        }
        if watch_rx.borrow().state == SupervisorState::Failed {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let wait = remaining.min(Duration::from_millis(250));
        if tokio::time::timeout(wait, watch_rx.changed())
            .await
            .is_err()
        {
            continue;
        }
    }
}
