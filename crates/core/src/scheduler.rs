//! Shared scheduler identities, fences, outcomes, and deterministic time.

use serde::{Deserialize, Serialize};
#[cfg(any(test, feature = "test-support"))]
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Arc, Mutex};
#[cfg(any(test, feature = "test-support"))]
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Boxed scheduler timer future returned by [`SchedulerClock`].
pub type SchedulerSleep<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Fixed, low-cardinality workload identity understood by the scheduler kernel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum SchedulerKind {
    /// Durable Object alarm delivery.
    #[serde(rename = "do_alarm")]
    Alarm,
    /// Queue consumer delivery, reserved until P2.3.
    #[serde(rename = "queue")]
    Queue,
    /// Cron logical slots, reserved until P2.3.
    #[serde(rename = "cron")]
    Cron,
    /// Workflow wakeups, reserved until P2.4.
    #[serde(rename = "workflow")]
    Workflow,
}

impl SchedulerKind {
    /// All kinds in the deterministic fairness order.
    pub const ALL: [Self; 4] = [Self::Alarm, Self::Queue, Self::Cron, Self::Workflow];

    /// Stable external JSON and metrics label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alarm => "do_alarm",
            Self::Queue => "queue",
            Self::Cron => "cron",
            Self::Workflow => "workflow",
        }
    }

    /// Stable array index used only for fixed-size kernel state.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Alarm => 0,
            Self::Queue => 1,
            Self::Cron => 2,
            Self::Workflow => 3,
        }
    }

    /// Parse the fixed operator/metrics spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// Generic scheduler fence shape; product completion still verifies its full typed authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchedulerFenceV1 {
    /// Workload kind that owns the claim.
    pub kind: SchedulerKind,
    /// Stable product-owned source identity.
    pub source_id: String,
    /// Product authority generation captured at claim time.
    pub authority_generation: u64,
    /// Unpredictable product claim token.
    pub claim_token: String,
    /// Persisted wall-clock lease expiry.
    pub claim_until_ms: i64,
}

/// Kernel-visible result after workload-specific dispatch and completion handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOutcome {
    /// Product authority completed under its exact fence.
    Completed,
    /// Product policy persisted a future retry.
    ProductRetryScheduled,
    /// A stale claim became a conditional no-op.
    StaleNoop,
    /// Dispatch may have happened, so the lease remains for recovery.
    LeaseRetainedUnknown,
    /// The owning workload pool rejected new work while degraded.
    CircuitOpen,
}

/// Process-local state exposed for one registered workload pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerPoolState {
    /// Pool can claim and dispatch work.
    Ready,
    /// Operator pause prevents new claims.
    Paused,
    /// Infrastructure retry deadline has not elapsed.
    Backoff,
    /// A permanent pool-local error requires an authenticated repair probe.
    CircuitOpen,
    /// Pool is present in configuration but not enabled.
    Disabled,
}

/// Low-cardinality workload facts consumed by the kernel and operator surface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadSummary {
    /// Work ready at the supplied effective wall time.
    pub ready: u64,
    /// Work currently protected by a claim lease.
    pub claimed: u64,
    /// Expired claims awaiting bounded recovery.
    pub expired: u64,
    /// Oldest due wall timestamp.
    pub oldest_due_at_ms: Option<i64>,
    /// Earliest persisted due or lease-recovery wall timestamp.
    pub next_due_at_ms: Option<i64>,
}

/// Test-only crash boundaries shared by scheduler workload adapters.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerFaultPoint {
    /// Claim transaction committed but dispatch has not begun.
    AfterClaimCommit,
    /// Immediately before invoking tenant code.
    BeforeDispatch,
    /// Dispatch returned but product completion has not committed.
    AfterDispatchBeforeComplete,
    /// Product completion committed.
    AfterCompleteCommit,
    /// Projection refresh is between authority observation and convergence.
    DuringProjectionRefresh,
}

/// Wall and monotonic time used by scheduler persistence, waits, timeouts, and drains.
pub trait SchedulerClock: Send + Sync {
    /// Persistable Unix epoch time in milliseconds.
    fn wall_time_ms(&self) -> i64;

    /// Current process-local monotonic time.
    fn monotonic_now(&self) -> Instant;

    /// Sleep until a process-local monotonic deadline.
    fn sleep_until(&self, deadline: Instant) -> SchedulerSleep<'_>;

    /// Process-local monotonic deadline after a delay.
    fn monotonic_deadline(&self, delay: Duration) -> Instant {
        self.monotonic_now()
            .checked_add(delay)
            .unwrap_or_else(|| self.monotonic_now())
    }
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

    fn monotonic_now(&self) -> Instant {
        Instant::now()
    }

    fn sleep_until(&self, deadline: Instant) -> SchedulerSleep<'_> {
        Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
            deadline,
        )))
    }
}

/// Scheduler clock whose wall time, monotonic time, and timers move only under test control.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug)]
pub struct DeterministicSchedulerClock {
    inner: Arc<Mutex<DeterministicSchedulerClockState>>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
struct DeterministicSchedulerClockState {
    wall_time_ms: i64,
    monotonic: Instant,
    next_timer_sequence: u64,
    timers: BTreeMap<(Instant, u64), Waker>,
}

#[cfg(any(test, feature = "test-support"))]
impl DeterministicSchedulerClock {
    /// Freeze a test clock at the supplied wall time.
    #[must_use]
    pub fn new(wall_time_ms: i64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DeterministicSchedulerClockState {
                wall_time_ms,
                monotonic: Instant::now(),
                next_timer_sequence: 0,
                timers: BTreeMap::new(),
            })),
        }
    }

    /// Move both clocks forward without sleeping.
    pub fn advance(&self, delay: Duration) {
        self.advance_both(delay);
    }

    /// Move wall time forward without changing process-local timers.
    pub fn advance_wall(&self, delay: Duration) {
        let mut state = self.lock();
        state.wall_time_ms = state.wall_time_ms.saturating_add(duration_ms(delay));
    }

    /// Move wall time backwards without changing process-local timers.
    pub fn set_wall_backwards(&self, delay: Duration) {
        let mut state = self.lock();
        state.wall_time_ms = state.wall_time_ms.saturating_sub(duration_ms(delay));
    }

    /// Move monotonic time forward and wake due timers in deadline/registration order.
    pub fn advance_monotonic(&self, delay: Duration) {
        let wakers = {
            let mut state = self.lock();
            state.monotonic = state
                .monotonic
                .checked_add(delay)
                .unwrap_or(state.monotonic);
            take_due_wakers(&mut state)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    /// Move wall and monotonic time forward together.
    pub fn advance_both(&self, delay: Duration) {
        self.advance_wall(delay);
        self.advance_monotonic(delay);
    }

    /// Set wall time independently, including backwards.
    pub fn set_wall_time_ms(&self, wall_time_ms: i64) {
        self.lock().wall_time_ms = wall_time_ms;
    }

    /// Number of pending deterministic timers.
    #[must_use]
    pub fn pending_timer_count(&self) -> usize {
        self.lock().timers.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DeterministicSchedulerClockState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl SchedulerClock for DeterministicSchedulerClock {
    fn wall_time_ms(&self) -> i64 {
        self.lock().wall_time_ms
    }

    fn monotonic_now(&self) -> Instant {
        self.lock().monotonic
    }

    fn sleep_until(&self, deadline: Instant) -> SchedulerSleep<'_> {
        Box::pin(DeterministicSleep {
            inner: self.inner.clone(),
            deadline,
            registration: None,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
struct DeterministicSleep {
    inner: Arc<Mutex<DeterministicSchedulerClockState>>,
    deadline: Instant,
    registration: Option<(Instant, u64)>,
}

#[cfg(any(test, feature = "test-support"))]
impl Future for DeterministicSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.inner.clone();
        let deadline = self.deadline;
        let mut state = inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.monotonic >= deadline {
            if let Some(key) = self.registration.take() {
                state.timers.remove(&key);
            }
            return Poll::Ready(());
        }
        let key = self.registration.unwrap_or_else(|| {
            let sequence = state.next_timer_sequence;
            state.next_timer_sequence = state.next_timer_sequence.saturating_add(1);
            (deadline, sequence)
        });
        state.timers.insert(key, context.waker().clone());
        drop(state);
        self.registration = Some(key);
        Poll::Pending
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for DeterministicSleep {
    fn drop(&mut self) {
        let Some(key) = self.registration else {
            return;
        };
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .timers
            .remove(&key);
    }
}

#[cfg(any(test, feature = "test-support"))]
fn duration_ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(any(test, feature = "test-support"))]
fn take_due_wakers(state: &mut DeterministicSchedulerClockState) -> Vec<Waker> {
    let due = state
        .timers
        .range(..=(state.monotonic, u64::MAX))
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();
    due.into_iter()
        .filter_map(|key| state.timers.remove(&key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::task::{ArcWake, waker};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    #[derive(Debug)]
    struct OrderedWake {
        id: usize,
        next: Arc<AtomicUsize>,
        order: Arc<Mutex<Vec<(usize, usize)>>>,
    }

    impl ArcWake for OrderedWake {
        fn wake_by_ref(value: &Arc<Self>) {
            let sequence = value.next.fetch_add(1, AtomicOrdering::AcqRel);
            value
                .order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((sequence, value.id));
        }
    }

    #[test]
    fn scheduler_kinds_have_fixed_external_identity_and_order() {
        assert_eq!(
            SchedulerKind::ALL.map(SchedulerKind::as_str),
            ["do_alarm", "queue", "cron", "workflow"]
        );
        for kind in SchedulerKind::ALL {
            assert_eq!(SchedulerKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(SchedulerKind::parse("tenant-value"), None);
    }

    #[tokio::test]
    async fn deterministic_clock_controls_all_timer_progress() {
        let clock = DeterministicSchedulerClock::new(10_000);
        let deadline = clock.monotonic_deadline(Duration::from_secs(3));
        let mut timer = clock.sleep_until(deadline);
        assert!(futures::poll!(&mut timer).is_pending());
        assert_eq!(clock.pending_timer_count(), 1);
        clock.set_wall_backwards(Duration::from_secs(20));
        assert!(futures::poll!(&mut timer).is_pending());
        clock.advance_monotonic(Duration::from_secs(3));
        timer.await;
        assert_eq!(clock.pending_timer_count(), 0);
        assert_eq!(clock.wall_time_ms(), -10_000);
    }

    #[test]
    fn same_deadline_timers_wake_in_registration_order() {
        let clock = DeterministicSchedulerClock::new(1_000);
        let deadline = clock.monotonic_deadline(Duration::from_secs(1));
        let next = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut first = clock.sleep_until(deadline);
        let mut second = clock.sleep_until(deadline);
        let first_waker = waker(Arc::new(OrderedWake {
            id: 1,
            next: next.clone(),
            order: order.clone(),
        }));
        let second_waker = waker(Arc::new(OrderedWake {
            id: 2,
            next,
            order: order.clone(),
        }));
        assert!(
            first
                .as_mut()
                .poll(&mut Context::from_waker(&first_waker))
                .is_pending()
        );
        assert!(
            second
                .as_mut()
                .poll(&mut Context::from_waker(&second_waker))
                .is_pending()
        );
        clock.advance_monotonic(Duration::from_secs(1));
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![(0, 1), (1, 2)]
        );
    }

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
