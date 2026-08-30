//! Runtime-generation fences for process-local deployment resources.

use crate::service_invocations::ServiceInvocationRegistry;
use open_compute_runtime::{SupervisorSnapshot, SupervisorState};
use open_compute_workers::DeploymentPins;

/// Observable effects of applying one supervisor snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeGenerationUpdate {
    /// A later running child replaced a previously observed running child.
    pub child_changed: bool,
    /// Process-local Service handles and generation retentions were cleared.
    pub resources_cleared: bool,
}

/// Single owner of process-local resource cleanup across workerd generations.
#[derive(Debug)]
pub struct RuntimeGenerationResources {
    services: ServiceInvocationRegistry,
    deployment_pins: DeploymentPins,
    running_pid: Option<i32>,
    running_generation: Option<String>,
}

impl RuntimeGenerationResources {
    /// Bind cleanup to the same Service authority and deployment-pin registry used for dispatch.
    #[must_use]
    pub fn new(services: ServiceInvocationRegistry, deployment_pins: DeploymentPins) -> Self {
        Self {
            services,
            deployment_pins,
            running_pid: None,
            running_generation: None,
        }
    }

    /// Apply one sanitized supervisor snapshot after the child lifecycle transition commits.
    ///
    /// Resources are cleared only when a replacement reaches `RUNNING`, or after the previously
    /// running child is confirmed absent in a terminal/backoff state. Transient startup and drain
    /// snapshots cannot prematurely release deletion fences owned by a live child.
    pub fn observe(&mut self, snapshot: &SupervisorSnapshot) -> RuntimeGenerationUpdate {
        if snapshot.state == SupervisorState::Running {
            let next_generation = snapshot.startup_id.map(|value| value.to_string());
            let child_changed = self.running_pid.is_some()
                && (self.running_pid != snapshot.pid || self.running_generation != next_generation);
            if child_changed {
                self.clear();
            }
            self.running_pid = snapshot.pid;
            self.running_generation = next_generation;
            return RuntimeGenerationUpdate {
                child_changed,
                resources_cleared: child_changed,
            };
        }

        let confirmed_exit = self.running_pid.is_some()
            && snapshot.pid.is_none()
            && matches!(
                snapshot.state,
                SupervisorState::Stopped | SupervisorState::BackingOff | SupervisorState::Failed
            );
        if confirmed_exit {
            self.clear();
            self.running_pid = None;
            self.running_generation = None;
        }
        RuntimeGenerationUpdate {
            child_changed: false,
            resources_cleared: confirmed_exit,
        }
    }

    fn clear(&self) {
        self.services.clear_after_child_exit();
        self.deployment_pins.clear_generation_retentions();
    }
}
