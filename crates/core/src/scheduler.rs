//! Deterministic time boundary for the P0.8 scheduler.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Wall and monotonic time used by scheduler persistence and process-local waits.
pub trait SchedulerClock: Send + Sync {
    /// Persistable Unix epoch time in milliseconds.
    fn wall_time_ms(&self) -> i64;

    /// Process-local monotonic deadline after `delay`.
    fn monotonic_deadline(&self, delay: Duration) -> Instant;
}

/// Operating-system scheduler clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSchedulerClock;

impl SchedulerClock for SystemSchedulerClock {
    fn wall_time_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(i64::MAX)
    }

    fn monotonic_deadline(&self, delay: Duration) -> Instant {
        Instant::now()
            .checked_add(delay)
            .unwrap_or_else(Instant::now)
    }
}

/// Scheduler clock whose wall and monotonic observations move only under test control.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct DeterministicSchedulerClock {
    state: std::sync::Mutex<DeterministicSchedulerClockState>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
struct DeterministicSchedulerClockState {
    wall_time_ms: i64,
    monotonic: Instant,
}

#[cfg(any(test, feature = "test-support"))]
impl DeterministicSchedulerClock {
    /// Freeze a test clock at the supplied wall time.
    #[must_use]
    pub fn new(wall_time_ms: i64) -> Self {
        Self {
            state: std::sync::Mutex::new(DeterministicSchedulerClockState {
                wall_time_ms,
                monotonic: Instant::now(),
            }),
        }
    }

    /// Move both clocks forward without sleeping.
    pub fn advance(&self, delay: Duration) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.wall_time_ms = state
            .wall_time_ms
            .saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX));
        state.monotonic = state
            .monotonic
            .checked_add(delay)
            .unwrap_or(state.monotonic);
    }

    /// Jump wall time independently, including backwards.
    pub fn set_wall_time_ms(&self, wall_time_ms: i64) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wall_time_ms = wall_time_ms;
    }
}

#[cfg(any(test, feature = "test-support"))]
impl SchedulerClock for DeterministicSchedulerClock {
    fn wall_time_ms(&self) -> i64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wall_time_ms
    }

    fn monotonic_deadline(&self, delay: Duration) -> Instant {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .monotonic
            .checked_add(delay)
            .unwrap_or(state.monotonic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_clock_supports_independent_wall_jumps() {
        let clock = DeterministicSchedulerClock::new(10_000);
        let deadline = clock.monotonic_deadline(Duration::from_secs(3));
        clock.set_wall_time_ms(1_000);
        assert_eq!(clock.wall_time_ms(), 1_000);
        assert_eq!(clock.monotonic_deadline(Duration::from_secs(3)), deadline);
        clock.advance(Duration::from_secs(2));
        assert_eq!(clock.wall_time_ms(), 3_000);
        assert_eq!(clock.monotonic_deadline(Duration::from_secs(1)), deadline);
    }
}
