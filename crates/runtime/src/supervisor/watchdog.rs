//! Functional liveness watchdog for the running workerd generation.
//!
//! Readiness (`/internal/ready`) gates admission at startup. Liveness (`/internal/live`)
//! proves that a Running generation's event loop, system Worker dispatch, and generation
//! credential can still complete one minimal request/response. It never reads SQLite, S3, or
//! any external dependency, so an outage of those cannot manufacture a runtime restart loop.
//!
//! All commands reaching the supervisor actor are generation-fenced by [`StartupId`]; a late
//! probe or suspicion from a superseded generation is dropped before it can affect the
//! current child.

use super::probe::probe_authenticated;
use super::*;
use open_compute_core::SecretString;

/// Authenticated liveness path served by the system gateway Worker.
pub const LIVE_PATH: &str = "/internal/live";

/// Fixed product constants for the functional watchdog. Not operator-tunable; tests inject
/// shorter values through `test-support` construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WatchdogConfig {
    /// Periodic probe cadence while Running.
    pub probe_interval: Duration,
    /// Bound on one liveness probe; a timeout counts as one failed probe.
    pub probe_timeout: Duration,
    /// Consecutive failed probes (periodic or suspicion-triggered) that confirm an
    /// unhealthy generation.
    pub failure_threshold: u32,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            probe_interval: Duration::from_secs(5),
            probe_timeout: Duration::from_secs(2),
            failure_threshold: 3,
        }
    }
}

/// Low-cardinality, secret-free evidence kinds carried by bridge suspicion. Never includes
/// URLs, tokens, loader keys, response bodies, or raw exceptions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFailureEvidence {
    /// A bounded internal request timed out waiting for its response headers.
    ResponseHeaderTimeout,
    /// A loopback connection to the runtime could not be established.
    ConnectFailed,
    /// An authenticated internal response could not be parsed or was rejected.
    MalformedInternalResponse,
    /// The periodic functional probe failed.
    PeriodicProbeFailed,
    /// The supervisor control channel reported a protocol failure.
    ControlChannelFailed,
}

impl RuntimeFailureEvidence {
    /// Stable name for sanitized status and logs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResponseHeaderTimeout => "response-header-timeout",
            Self::ConnectFailed => "connect-failed",
            Self::MalformedInternalResponse => "malformed-internal-response",
            Self::PeriodicProbeFailed => "periodic-probe-failed",
            Self::ControlChannelFailed => "control-channel-failed",
        }
    }
}

/// One authenticated liveness probe against the Running generation.
pub(crate) async fn probe_live(
    port: u16,
    token: &SecretString,
    config: WatchdogConfig,
) -> Result<(), PlatformError> {
    probe_authenticated(port, token.expose(), LIVE_PATH, config.probe_timeout).await
}

impl Actor {
    /// Bind the attempt's probe token and reset all per-generation watchdog state.
    pub(super) fn begin_generation_watchdog_state(&mut self, token: SecretString) {
        self.live_token = Some(token);
        self.probe_failures = 0;
        self.probe_flight = None;
        self.next_periodic_probe_at = None;
        self.snap.last_suspicion = None;
    }

    /// Clear all per-generation watchdog state during teardown.
    pub(super) fn clear_generation_watchdog_state(&mut self) {
        self.live_token = None;
        self.probe_flight = None;
        self.probe_failures = 0;
        self.next_periodic_probe_at = None;
    }

    /// Schedule the first periodic probe after a generation becomes Running.
    pub(super) fn schedule_first_periodic_probe(&mut self) {
        self.probe_failures = 0;
        self.probe_flight = None;
        self.next_periodic_probe_at = Some(
            self.clock.now()
                + self
                    .watchdog
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .probe_interval,
        );
    }
}

impl Actor {
    /// Bridge-level suspicion for one generation: record the sanitized evidence and answer
    /// with a single functional probe. Suspicion alone never restarts; a superseded or
    /// non-running generation's evidence is dropped without consuming budget.
    pub(super) fn handle_suspicion(
        &mut self,
        startup_id: StartupId,
        evidence: RuntimeFailureEvidence,
    ) {
        if self.snap.state == SupervisorState::Running
            && !self.shutting_down
            && self.snap.startup_id == Some(startup_id)
        {
            self.snap.last_suspicion = Some(evidence.as_str());
            self.publish();
            self.launch_functional_probe(startup_id);
        }
    }

    /// One functional probe result, fenced to the generation it probed. A failed probe
    /// re-probes immediately until the small threshold confirms the fault; one confirmed
    /// fault consumes exactly one teardown and one budget consumption.
    pub(super) async fn handle_probe_result(&mut self, startup_id: StartupId, healthy: bool) {
        if self.snap.state != SupervisorState::Running || self.snap.startup_id != Some(startup_id) {
            return;
        }
        self.probe_flight = None;
        let watchdog = *self
            .watchdog
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if healthy {
            self.probe_failures = 0;
            self.next_periodic_probe_at = Some(self.clock.now() + watchdog.probe_interval);
        } else {
            self.probe_failures = self.probe_failures.saturating_add(1);
            if self.probe_failures >= watchdog.failure_threshold {
                match self.teardown_child().await {
                    Ok(report) => {
                        self.fail_or_backoff(
                            ErrorCode::RuntimeExitedInFlight,
                            true,
                            report.as_ref(),
                        )
                        .await;
                    }
                    Err(_) => self.fail_closed_after_teardown(),
                }
            } else {
                // Re-probe immediately so the small threshold converges quickly.
                self.launch_functional_probe(startup_id);
            }
        }
    }

    pub(super) fn launch_functional_probe(&mut self, startup_id: StartupId) {
        if self.probe_flight.is_some() {
            // Single-flight per generation: concurrent suspicions merge into one probe.
            return;
        }
        let (Some(port), Some(token)) = (self.snap.listen_port, self.live_token.clone()) else {
            return;
        };
        self.probe_flight = Some(startup_id);
        let tx = self.cmd_tx.clone();
        let config = *self
            .watchdog
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokio::spawn(async move {
            let healthy = probe_live(port, &token, config).await.is_ok();
            let _ = tx.send(Command::FunctionalProbeResult {
                startup_id,
                healthy,
            });
        });
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::*;

    #[test]
    fn evidence_names_are_stable_and_secret_free() {
        for (evidence, name) in [
            (
                RuntimeFailureEvidence::ResponseHeaderTimeout,
                "response-header-timeout",
            ),
            (RuntimeFailureEvidence::ConnectFailed, "connect-failed"),
            (
                RuntimeFailureEvidence::MalformedInternalResponse,
                "malformed-internal-response",
            ),
            (
                RuntimeFailureEvidence::PeriodicProbeFailed,
                "periodic-probe-failed",
            ),
            (
                RuntimeFailureEvidence::ControlChannelFailed,
                "control-channel-failed",
            ),
        ] {
            assert_eq!(evidence.as_str(), name);
        }
    }

    #[test]
    fn default_watchdog_uses_fixed_product_constants() {
        let config = WatchdogConfig::default();
        assert_eq!(config.probe_interval, Duration::from_secs(5));
        assert_eq!(config.probe_timeout, Duration::from_secs(2));
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(LIVE_PATH, "/internal/live");
    }
}
