//! Scheduler notification and deadline coordination.

use open_compute_core::SchedulerClock;
use open_compute_storage::SchedulerWakeSignal;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WakeReason {
    Notification,
    Due,
    Repair,
    Backoff,
    Safety,
}

impl WakeReason {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Notification => "notification",
            Self::Due => "due",
            Self::Repair => "repair",
            Self::Backoff => "backoff",
            Self::Safety => "safety",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WakeDeadline {
    pub(super) at: Instant,
    pub(super) reason: WakeReason,
}

#[derive(Clone)]
pub(super) struct WakeCoordinator {
    signal: Arc<SchedulerWakeSignal>,
    clock: Arc<dyn SchedulerClock>,
    safety_interval: Duration,
}

impl std::fmt::Debug for WakeCoordinator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WakeCoordinator")
            .field("generation", &self.signal.generation())
            .field("safety_interval", &self.safety_interval)
            .finish_non_exhaustive()
    }
}

impl WakeCoordinator {
    pub(super) fn new(
        signal: Arc<SchedulerWakeSignal>,
        clock: Arc<dyn SchedulerClock>,
        safety_interval: Duration,
    ) -> Self {
        Self {
            signal,
            clock,
            safety_interval,
        }
    }

    pub(super) fn generation(&self) -> u64 {
        self.signal.generation()
    }

    pub(super) fn notify(&self) {
        self.signal.notify();
    }

    pub(super) fn wall_deadline(&self, effective_wall_now_ms: i64, due_at_ms: i64) -> Instant {
        let delay_ms = due_at_ms.saturating_sub(effective_wall_now_ms).max(0);
        self.clock.monotonic_deadline(Duration::from_millis(
            u64::try_from(delay_ms).unwrap_or(u64::MAX),
        ))
    }

    pub(super) async fn wait(
        &self,
        observed_generation: u64,
        deadlines: &[WakeDeadline],
    ) -> WakeReason {
        let safety = WakeDeadline {
            at: self.clock.monotonic_deadline(self.safety_interval),
            reason: WakeReason::Safety,
        };
        let earliest = deadlines
            .iter()
            .copied()
            .chain(std::iter::once(safety))
            .min_by_key(|deadline| deadline.at)
            .unwrap_or(safety);
        let notification = self.signal.notified_since(observed_generation);
        let timer = self.clock.sleep_until(earliest.at);
        tokio::pin!(notification);
        tokio::pin!(timer);
        tokio::select! {
            _ = &mut notification => WakeReason::Notification,
            () = &mut timer => earliest.reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::DeterministicSchedulerClock;

    #[tokio::test]
    async fn earlier_deadline_wins_without_real_sleep() {
        let clock = Arc::new(DeterministicSchedulerClock::new(1_000));
        let signal = Arc::new(SchedulerWakeSignal::default());
        let wake = WakeCoordinator::new(signal, clock.clone(), Duration::from_secs(60));
        let observed = wake.generation();
        let deadline = WakeDeadline {
            at: clock.monotonic_deadline(Duration::from_secs(10)),
            reason: WakeReason::Due,
        };
        let waiter = tokio::spawn({
            let wake = wake.clone();
            async move { wake.wait(observed, &[deadline]).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(clock.pending_timer_count(), 1);
        clock.advance_monotonic(Duration::from_secs(10));
        assert_eq!(waiter.await.unwrap(), WakeReason::Due);
    }

    #[tokio::test]
    async fn notification_during_query_window_is_observed() {
        let clock = Arc::new(DeterministicSchedulerClock::new(1_000));
        let signal = Arc::new(SchedulerWakeSignal::default());
        let wake = WakeCoordinator::new(signal, clock, Duration::from_secs(60));
        let observed = wake.generation();
        wake.notify();
        assert_eq!(wake.wait(observed, &[]).await, WakeReason::Notification);
    }
}
