//! Time source used by later crates.
//!
//! The deterministic implementation is compiled only for tests and the
//! `test-support` feature so production binaries cannot inject a fake clock
//! without an explicit opt-in.

#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;
#[cfg(any(test, feature = "test-support"))]
use std::time::Duration;
use std::time::SystemTime;

/// Monotonic-enough wall clock used for timeouts, IDs, and health timestamps.
pub trait Clock: Send + Sync {
    /// Current wall-clock instant.
    fn now(&self) -> SystemTime;
}

/// Operating-system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Test clock that advances only when the caller says so.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct DeterministicClock {
    now: Mutex<SystemTime>,
}

#[cfg(any(test, feature = "test-support"))]
impl DeterministicClock {
    /// Create a clock frozen at `now`.
    #[must_use]
    pub fn new(now: SystemTime) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }

    /// Advance the clock by `delta`.
    pub fn advance(&self, delta: Duration) {
        let mut guard = self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard += delta;
    }

    /// Jump the clock to an absolute instant.
    pub fn set(&self, now: SystemTime) {
        let mut guard = self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = now;
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Clock for DeterministicClock {
    fn now(&self) -> SystemTime {
        *self
            .now
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "clock_tests.rs"]
mod tests;
