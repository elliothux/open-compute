//! Exponential backoff with injectable jitter and a rolling restart window.

use open_compute_core::config::RuntimeConfig;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// Source of restart jitter in milliseconds.
pub trait JitterRng: Send + Sync {
    /// Return a value in `0..=max_inclusive_ms`.
    fn jitter(&self, max_inclusive_ms: u64) -> u64;
}

/// Operating-system entropy jitter.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsJitter;

impl JitterRng for OsJitter {
    fn jitter(&self, max_inclusive_ms: u64) -> u64 {
        if max_inclusive_ms == 0 {
            return 0;
        }
        use rand::Rng;
        rand::rng().random_range(0..=max_inclusive_ms)
    }
}

/// Deterministic jitter sequence for tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct SequenceJitter {
    values: std::sync::Mutex<VecDeque<u64>>,
}

#[cfg(any(test, feature = "test-support"))]
impl SequenceJitter {
    /// Use these jitter values in order, then zero.
    #[must_use]
    pub fn new(values: Vec<u64>) -> Self {
        Self {
            values: std::sync::Mutex::new(values.into()),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl JitterRng for SequenceJitter {
    fn jitter(&self, max_inclusive_ms: u64) -> u64 {
        let mut guard = self
            .values
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = guard.pop_front().unwrap_or(0);
        value.min(max_inclusive_ms)
    }
}

#[derive(Debug)]
pub(crate) struct RestartBudget {
    events: VecDeque<SystemTime>,
}

impl RestartBudget {
    pub(crate) fn new() -> Self {
        Self {
            events: VecDeque::new(),
        }
    }

    pub(crate) fn record(&mut self, now: SystemTime, window: Duration) {
        self.events.push_back(now);
        self.prune(now, window);
    }

    pub(crate) fn prune(&mut self, now: SystemTime, window: Duration) {
        while let Some(front) = self.events.front().copied() {
            match now.duration_since(front) {
                Ok(age) if age > window => {
                    self.events.pop_front();
                }
                _ => break,
            }
        }
    }

    pub(crate) fn exceeded(&self, budget: u32) -> bool {
        self.events.len() >= budget as usize
    }
}

pub(crate) fn backoff_delay(
    cfg: &RuntimeConfig,
    failures: u32,
    jitter: &dyn JitterRng,
) -> Duration {
    let exp = failures.saturating_sub(1).min(20);
    let mut ms = cfg.restart_backoff_initial_ms.saturating_mul(1u64 << exp);
    ms = ms.min(cfg.restart_backoff_max_ms);
    let jitter_ms = jitter.jitter(ms / 2);
    Duration::from_millis(ms.saturating_add(jitter_ms))
}

#[cfg(test)]
#[path = "backoff_tests.rs"]
mod tests;
