//! Concurrency-safe health coordinator around core [`PlatformStatus`].

use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, PlatformError, PlatformStatus, ReadinessReason,
};
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use std::sync::{Arc, Mutex};

/// Shared health authority for HTTP and supervisor watchers.
#[derive(Clone, Debug)]
pub struct HealthCoordinator {
    inner: Arc<Mutex<PlatformStatus>>,
}

impl Default for HealthCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthCoordinator {
    /// All components starting.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PlatformStatus::starting())),
        }
    }

    /// Snapshot clone.
    #[must_use]
    pub fn snapshot(&self) -> PlatformStatus {
        self.lock().clone()
    }

    /// Aggregate readiness.
    #[must_use]
    pub fn readiness(&self) -> ReadinessReason {
        self.lock().readiness
    }

    /// Transition one component and recompute aggregate readiness.
    pub fn set_component(
        &self,
        name: ComponentName,
        state: ComponentState,
        reason: Option<ReadinessReason>,
    ) -> Result<(), PlatformError> {
        let mut status = self.lock();
        set_component_locked(&mut status, name, state, reason)
    }

    /// Map a supervisor snapshot onto the runtime component.
    ///
    /// Intermediate `watch` states may be coalesced; this bridges skipped
    /// legal edges (`Failed -> Starting -> Healthy`, `Healthy -> Failed -> Starting`)
    /// instead of requiring every snapshot.
    pub fn apply_supervisor(&self, snap: &SupervisorSnapshot) -> Result<(), PlatformError> {
        let mut status = self.lock();
        let idx = status
            .components
            .iter()
            .position(|c| c.name == ComponentName::Runtime)
            .expect("fixed component set");
        let previous = status.components[idx].state;
        if previous == ComponentState::Draining {
            status.components[idx].reason = Some(ReadinessReason::Draining);
            status.recompute();
            return Ok(());
        }
        let (desired, reason) = map_supervisor(snap);
        apply_bridged(&mut status, idx, previous, desired, reason)
    }

    /// Mark every component draining.
    pub fn begin_drain(&self) -> Result<(), PlatformError> {
        let names = [
            ComponentName::Process,
            ComponentName::DataDir,
            ComponentName::ControlDb,
            ComponentName::MasterKey,
            ComponentName::S3,
            ComponentName::Cache,
            ComponentName::Runtime,
            ComponentName::Scheduler,
            ComponentName::Operations,
        ];
        let mut status = self.lock();
        for name in names {
            let idx = status
                .components
                .iter()
                .position(|c| c.name == name)
                .expect("fixed component set");
            let current = status.components[idx].state;
            if current == ComponentState::Draining {
                status.components[idx].reason = Some(ReadinessReason::Draining);
                continue;
            }
            if !current.can_transition_to(ComponentState::Draining) {
                tracing::error!(
                    component = name.as_str(),
                    from = current.as_str(),
                    "illegal drain transition"
                );
                return Err(PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "illegal component health transition",
                ));
            }
            status.components[idx]
                .transition(ComponentState::Draining, Some(ReadinessReason::Draining))?;
        }
        status.recompute();
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PlatformStatus> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn set_component_locked(
    status: &mut PlatformStatus,
    name: ComponentName,
    state: ComponentState,
    reason: Option<ReadinessReason>,
) -> Result<(), PlatformError> {
    let component = status
        .components
        .iter_mut()
        .find(|c| c.name == name)
        .expect("fixed component set");
    component.transition(state, reason)?;
    status.recompute();
    Ok(())
}

fn apply_bridged(
    status: &mut PlatformStatus,
    idx: usize,
    previous: ComponentState,
    desired: ComponentState,
    reason: ReadinessReason,
) -> Result<(), PlatformError> {
    if previous == desired {
        status.components[idx].reason = Some(reason);
        status.recompute();
        return Ok(());
    }
    if previous.can_transition_to(desired) {
        status.components[idx].transition(desired, Some(reason))?;
        status.recompute();
        return Ok(());
    }
    let hops: &[ComponentState] = match (previous, desired) {
        (ComponentState::Failed, ComponentState::Healthy) => {
            &[ComponentState::Starting, ComponentState::Healthy]
        }
        (ComponentState::Healthy, ComponentState::Starting) => {
            &[ComponentState::Failed, ComponentState::Starting]
        }
        (ComponentState::Degraded, ComponentState::Starting) => {
            &[ComponentState::Failed, ComponentState::Starting]
        }
        (ComponentState::Healthy, ComponentState::Healthy) => &[],
        _ => {
            tracing::error!(
                from = previous.as_str(),
                to = desired.as_str(),
                reason = reason.as_str(),
                "illegal runtime health transition"
            );
            if previous.can_transition_to(ComponentState::Failed) {
                let _ = status.components[idx].transition(
                    ComponentState::Failed,
                    Some(ReadinessReason::RuntimeInvalid),
                );
                status.recompute();
            }
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "illegal component health transition",
            ));
        }
    };
    for (i, hop) in hops.iter().enumerate() {
        let hop_reason = if i + 1 == hops.len() {
            reason
        } else if *hop == ComponentState::Starting {
            ReadinessReason::RuntimeStarting
        } else if *hop == ComponentState::Failed {
            ReadinessReason::RuntimeRestartBackoff
        } else {
            reason
        };
        status.components[idx].transition(*hop, Some(hop_reason))?;
    }
    status.recompute();
    Ok(())
}

/// Exact supervisor-to-component mapping.
#[must_use]
pub fn map_supervisor(snap: &SupervisorSnapshot) -> (ComponentState, ReadinessReason) {
    match snap.state {
        SupervisorState::Stopped => (ComponentState::Starting, ReadinessReason::Starting),
        SupervisorState::Starting => (ComponentState::Starting, ReadinessReason::RuntimeStarting),
        SupervisorState::Running => (ComponentState::Healthy, ReadinessReason::Ready),
        SupervisorState::BackingOff => (
            ComponentState::Failed,
            ReadinessReason::RuntimeRestartBackoff,
        ),
        SupervisorState::Failed => (
            ComponentState::Failed,
            if snap.reason == ReadinessReason::RuntimeInvalid {
                ReadinessReason::RuntimeInvalid
            } else {
                snap.reason
            },
        ),
        SupervisorState::Draining | SupervisorState::Stopping => {
            (ComponentState::Draining, ReadinessReason::Draining)
        }
    }
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
