//! Supervisor state machine snapshot.

use open_compute_core::error::ReadinessReason;
use open_compute_core::ids::StartupId;
use serde::Serialize;
use std::fmt::{Debug, Formatter};
use std::time::SystemTime;

/// Documented workerd supervisor states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SupervisorState {
    /// No child is running.
    Stopped,
    /// Spawn and readiness handshake in progress.
    Starting,
    /// Control-fd listen and Unix probe succeeded.
    Running,
    /// Waiting to retry after a retryable failure.
    BackingOff,
    /// Restart budget exhausted or a permanent dependency failure.
    Failed,
    /// Drain window before SIGTERM.
    Draining,
    /// SIGTERM/SIGKILL in progress.
    Stopping,
}

impl SupervisorState {
    /// Stable uppercase token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "STOPPED",
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::BackingOff => "BACKING_OFF",
            Self::Failed => "FAILED",
            Self::Draining => "DRAINING",
            Self::Stopping => "STOPPING",
        }
    }
}

/// Sanitized child exit, never containing logs or tokens.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SanitizedExit {
    /// Process exit code if it exited.
    pub code: Option<i32>,
    /// POSIX signal if terminated by signal.
    pub signal: Option<i32>,
    /// Whether this failure may consume restart budget and retry.
    pub retryable: bool,
    /// Stable error code string.
    pub code_name: String,
}

/// Operator-safe supervisor snapshot.
#[derive(Clone, PartialEq, Serialize)]
pub struct SupervisorSnapshot {
    /// Current state.
    pub state: SupervisorState,
    /// Stable readiness reason.
    pub reason: ReadinessReason,
    /// Last state transition time.
    pub last_transition_at: SystemTime,
    /// Spawn attempt counter for the current generation window.
    pub attempt: u32,
    /// Last sanitized exit.
    pub last_exit: Option<SanitizedExit>,
    /// Runtime generation that produced [`Self::last_exit`].
    pub last_exit_startup_id: Option<StartupId>,
    /// Next retry instant while backing off.
    pub next_retry_at: Option<SystemTime>,
    /// Child PID while a process exists.
    pub pid: Option<i32>,
    /// Child process group while a process exists.
    pub pgid: Option<i32>,
    /// Build-time binary pin.
    pub binary_digest: String,
    /// Compiled config input digest.
    pub config_digest: String,
    /// Current or last [`StartupId`].
    pub startup_id: Option<StartupId>,
    /// Sanitized low-cardinality evidence kind from the latest accepted generation
    /// suspicion; cleared when a new generation starts. Never URLs, tokens, or raw errors.
    pub last_suspicion: Option<&'static str>,
    /// Private Unix ingress endpoint, omitted from public status and Debug.
    #[serde(skip)]
    pub ingress_socket: Option<std::path::PathBuf>,
}

impl Debug for SupervisorSnapshot {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupervisorSnapshot")
            .field("state", &self.state)
            .field("reason", &self.reason)
            .field("last_transition_at", &self.last_transition_at)
            .field("attempt", &self.attempt)
            .field("last_exit", &self.last_exit)
            .field("last_exit_startup_id", &self.last_exit_startup_id)
            .field("next_retry_at", &self.next_retry_at)
            .field("pid", &self.pid)
            .field("pgid", &self.pgid)
            .field("binary_digest", &self.binary_digest)
            .field("config_digest", &self.config_digest)
            .field("startup_id", &self.startup_id)
            .finish_non_exhaustive()
    }
}

impl SupervisorSnapshot {
    pub(crate) fn initial(now: SystemTime, binary_digest: String) -> Self {
        Self {
            state: SupervisorState::Stopped,
            reason: ReadinessReason::Starting,
            last_transition_at: now,
            attempt: 0,
            last_exit: None,
            last_exit_startup_id: None,
            next_retry_at: None,
            pid: None,
            pgid: None,
            binary_digest,
            config_digest: String::new(),
            startup_id: None,
            last_suspicion: None,
            ingress_socket: None,
        }
    }
}
