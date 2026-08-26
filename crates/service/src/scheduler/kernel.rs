//! Global/pool admission, infrastructure backoff, and pool failure isolation.

use open_compute_core::{ErrorCode, SchedulerClock, SchedulerKind, SchedulerPoolState};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

const POOL_COUNT: usize = SchedulerKind::ALL.len();

#[derive(Clone, Debug)]
pub(super) struct AdmissionTracker {
    global_cap: usize,
    pool_caps: [usize; POOL_COUNT],
    global_in_flight: usize,
    pool_in_flight: [usize; POOL_COUNT],
}

impl AdmissionTracker {
    pub(super) fn new(global_cap: usize, pool_caps: [usize; POOL_COUNT]) -> Self {
        Self {
            global_cap,
            pool_caps,
            global_in_flight: 0,
            pool_in_flight: [0; POOL_COUNT],
        }
    }

    pub(super) fn available_global(&self) -> usize {
        self.global_cap.saturating_sub(self.global_in_flight)
    }

    pub(super) fn available_pools(&self) -> [usize; POOL_COUNT] {
        SchedulerKind::ALL.map(|kind| {
            self.pool_caps[kind.index()].saturating_sub(self.pool_in_flight[kind.index()])
        })
    }

    pub(super) fn reserve(&mut self, kind: SchedulerKind, count: usize) -> bool {
        let index = kind.index();
        if count > self.available_global()
            || count > self.pool_caps[index].saturating_sub(self.pool_in_flight[index])
        {
            return false;
        }
        self.global_in_flight = self.global_in_flight.saturating_add(count);
        self.pool_in_flight[index] = self.pool_in_flight[index].saturating_add(count);
        true
    }

    pub(super) fn release(&mut self, kind: SchedulerKind, count: usize) {
        let index = kind.index();
        self.global_in_flight = self.global_in_flight.saturating_sub(count);
        self.pool_in_flight[index] = self.pool_in_flight[index].saturating_sub(count);
    }

    pub(super) fn global_in_flight(&self) -> usize {
        self.global_in_flight
    }

    pub(super) fn pool_in_flight(&self, kind: SchedulerKind) -> usize {
        self.pool_in_flight[kind.index()]
    }
}

#[derive(Clone, Debug)]
pub(super) struct InfrastructureBackoff {
    boot_seed: u64,
    attempts: [u32; POOL_COUNT],
    base: Duration,
    cap: Duration,
}

impl InfrastructureBackoff {
    pub(super) fn new(boot_seed: u64, base: Duration, cap: Duration) -> Self {
        Self {
            boot_seed,
            attempts: [0; POOL_COUNT],
            base,
            cap,
        }
    }

    pub(super) fn fail(&mut self, kind: SchedulerKind, error_class: u64) -> Duration {
        let index = kind.index();
        let attempt = self.attempts[index];
        self.attempts[index] = attempt.saturating_add(1);
        let exponent = attempt.min(31);
        let factor = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let exponential = self.base.saturating_mul(factor).min(self.cap);
        let jitter_bound = (exponential / 4).max(Duration::from_millis(1));
        let jitter_ms = stable_mix(
            self.boot_seed
                ^ ((kind.index() as u64) << 48)
                ^ (error_class << 16)
                ^ u64::from(attempt),
        ) % u64::try_from(jitter_bound.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        exponential
            .saturating_add(Duration::from_millis(jitter_ms))
            .min(self.cap)
    }

    pub(super) fn reset(&mut self, kind: SchedulerKind) {
        self.attempts[kind.index()] = 0;
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PoolRuntime {
    state: SchedulerPoolState,
    retry_at: Option<Instant>,
}

impl PoolRuntime {
    pub(super) const fn ready() -> Self {
        Self {
            state: SchedulerPoolState::Ready,
            retry_at: None,
        }
    }

    pub(super) const fn state(self) -> SchedulerPoolState {
        self.state
    }

    pub(super) const fn retry_at(self) -> Option<Instant> {
        self.retry_at
    }

    pub(super) fn transient_failure(&mut self, retry_at: Instant) {
        self.state = SchedulerPoolState::Backoff;
        self.retry_at = Some(retry_at);
    }

    pub(super) fn permanent_failure(&mut self) {
        self.state = SchedulerPoolState::CircuitOpen;
        self.retry_at = None;
    }

    pub(super) fn probe_succeeded(&mut self) {
        self.state = SchedulerPoolState::Ready;
        self.retry_at = None;
    }

    pub(super) fn refresh_deadline(&mut self, now: Instant) {
        if self.state == SchedulerPoolState::Backoff
            && self.retry_at.is_some_and(|deadline| deadline <= now)
        {
            self.probe_succeeded();
        }
    }
}

pub(super) const fn permanent_pool_error(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::SchedulerInternalProtocolError)
}

fn stable_mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

pub(super) async fn bounded_drain<T: Send + 'static>(
    clock: &dyn SchedulerClock,
    timeout: Duration,
    tasks: &mut JoinSet<T>,
) -> bool {
    let timer = clock.sleep_until(clock.monotonic_deadline(timeout));
    tokio::pin!(timer);
    while !tasks.is_empty() {
        tokio::select! {
            _ = tasks.join_next() => {}
            () = &mut timer => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::DeterministicSchedulerClock;
    use std::sync::Arc;

    #[test]
    fn global_and_pool_admission_never_exceed_caps() {
        let mut tracker = AdmissionTracker::new(4, [2, 3, 1, 1]);
        assert!(tracker.reserve(SchedulerKind::Queue, 3));
        assert!(!tracker.reserve(SchedulerKind::Queue, 1));
        assert!(tracker.reserve(SchedulerKind::Alarm, 1));
        assert!(!tracker.reserve(SchedulerKind::Cron, 1));
        assert_eq!(tracker.global_in_flight(), 4);
        tracker.release(SchedulerKind::Queue, 2);
        assert!(tracker.reserve(SchedulerKind::Cron, 1));
        assert_eq!(tracker.pool_in_flight(SchedulerKind::Cron), 1);
    }

    #[test]
    fn hung_pool_only_consumes_its_own_and_global_permits() {
        let mut tracker = AdmissionTracker::new(4, [2, 2, 2, 2]);
        assert!(tracker.reserve(SchedulerKind::Queue, 2));
        assert!(tracker.reserve(SchedulerKind::Alarm, 2));
        tracker.release(SchedulerKind::Alarm, 2);
        assert!(tracker.reserve(SchedulerKind::Workflow, 2));
        assert_eq!(tracker.pool_in_flight(SchedulerKind::Queue), 2);
    }

    #[test]
    fn deterministic_backoff_caps_and_resets() {
        let mut first =
            InfrastructureBackoff::new(7, Duration::from_millis(10), Duration::from_millis(100));
        let mut second = first.clone();
        let sequence = (0..8)
            .map(|_| first.fail(SchedulerKind::Queue, 3))
            .collect::<Vec<_>>();
        assert_eq!(
            sequence,
            (0..8)
                .map(|_| second.fail(SchedulerKind::Queue, 3))
                .collect::<Vec<_>>()
        );
        assert!(sequence.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(
            sequence
                .iter()
                .all(|delay| *delay <= Duration::from_millis(100))
        );
        first.reset(SchedulerKind::Queue);
        assert_eq!(
            first.fail(SchedulerKind::Queue, 3),
            InfrastructureBackoff::new(7, Duration::from_millis(10), Duration::from_millis(100))
                .fail(SchedulerKind::Queue, 3)
        );
    }

    #[test]
    fn pool_circuit_and_backoff_are_isolated() {
        let now = Instant::now();
        let mut pools = [PoolRuntime::ready(); POOL_COUNT];
        pools[SchedulerKind::Queue.index()].permanent_failure();
        pools[SchedulerKind::Cron.index()].transient_failure(now + Duration::from_secs(1));
        assert_eq!(
            pools[SchedulerKind::Alarm.index()].state(),
            SchedulerPoolState::Ready
        );
        assert_eq!(
            pools[SchedulerKind::Queue.index()].state(),
            SchedulerPoolState::CircuitOpen
        );
        pools[SchedulerKind::Cron.index()].refresh_deadline(now + Duration::from_secs(1));
        assert_eq!(
            pools[SchedulerKind::Cron.index()].state(),
            SchedulerPoolState::Ready
        );
    }

    #[tokio::test]
    async fn bounded_drain_completes_ready_tasks_and_aborts_multiple_hung_pools() {
        let clock = Arc::new(DeterministicSchedulerClock::new(1_000));
        let mut ready = JoinSet::new();
        ready.spawn(async { SchedulerKind::Alarm });
        ready.spawn(async { SchedulerKind::Cron });
        assert!(bounded_drain(clock.as_ref(), Duration::from_secs(10), &mut ready).await);

        let mut hung = JoinSet::new();
        hung.spawn(async {
            std::future::pending::<()>().await;
            SchedulerKind::Queue
        });
        hung.spawn(async {
            std::future::pending::<()>().await;
            SchedulerKind::Workflow
        });
        let drain = tokio::spawn({
            let clock = clock.clone();
            async move { bounded_drain(clock.as_ref(), Duration::from_secs(10), &mut hung).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(clock.pending_timer_count(), 1);
        clock.advance_monotonic(Duration::from_secs(10));
        assert!(!drain.await.unwrap());
    }
}
