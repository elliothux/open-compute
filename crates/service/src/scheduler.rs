//! P2.1 multi-workload scheduler kernel with the production Alarm adapter.

#[path = "scheduler/alarm.rs"]
mod alarm;
#[path = "scheduler/fairness.rs"]
mod fairness;
#[path = "scheduler/kernel.rs"]
mod kernel;
#[path = "scheduler/queue.rs"]
mod queue;
#[path = "scheduler/wake.rs"]
mod wake;

use crate::metrics::{AlarmOutcome, MetricsRegistry};
use crate::runtime_bridge::WorkerdTransport;
use fairness::FairSelector;
use futures::{StreamExt as _, stream};
use kernel::{
    AdmissionTracker, InfrastructureBackoff, PoolRuntime, bounded_drain, permanent_pool_error,
};
use open_compute_core::{
    ComponentName, ComponentState, DurableObjectId, ErrorCode, PlatformError, ReadinessReason,
    ResourceId, SchedulerClock, SchedulerConfig, SchedulerKind, SchedulerPoolState,
};
use open_compute_storage::{
    DurableObjectRecord, PlatformStorage, SchedulerStore, SchedulerSummary,
};
use serde::Serialize;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tokio::sync::watch;
use tokio::task::JoinSet;
use wake::{WakeCoordinator, WakeDeadline, WakeReason};

type RepairCursor = (ResourceId, DurableObjectId, u64);

/// Composed scheduler DB, control authority, clock, and current workerd transport.
pub struct SchedulerService {
    store: Arc<SchedulerStore>,
    storage: Arc<PlatformStorage>,
    transport: WorkerdTransport,
    config: SchedulerConfig,
    clock: Arc<dyn SchedulerClock>,
    observed_wall_floor_ms: AtomicI64,
    paused: AtomicBool,
    alarm_paused: AtomicBool,
    queue_paused: AtomicBool,
    alarm_pool_state: AtomicU8,
    queue_pool_state: AtomicU8,
    global_in_flight: AtomicUsize,
    alarm_in_flight: AtomicUsize,
    queue_in_flight: AtomicUsize,
    next_wake_at_ms: AtomicI64,
    wake: WakeCoordinator,
    repair_cursor: Mutex<Option<RepairCursor>>,
    metrics: Option<Arc<MetricsRegistry>>,
    health: Option<crate::health::HealthCoordinator>,
    #[cfg(any(test, feature = "test-support"))]
    fault_hook: Option<Arc<dyn Fn(open_compute_core::SchedulerFaultPoint) + Send + Sync>>,
}

/// Versioned P2.1 scheduler operator response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerInspectV2 {
    /// Response schema version.
    pub version: u32,
    /// Whether global operator pause blocks all new claims.
    pub paused: bool,
    /// Global admission and wake state.
    pub global: SchedulerGlobalInspect,
    /// Registered production pools only.
    pub pools: Vec<SchedulerPoolInspect>,
}

/// Global scheduler admission facts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerGlobalInspect {
    /// Total in-flight dispatch count.
    pub in_flight: usize,
    /// Global dispatch cap.
    pub max_in_flight: u32,
    /// Earliest persisted wall-clock wake, if any.
    pub next_wake_at: Option<i64>,
}

/// One registered production workload pool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerPoolInspect {
    /// Fixed workload identity.
    pub kind: SchedulerKind,
    /// Whether this pool admits production work.
    pub enabled: bool,
    /// Process-local pool state.
    pub state: SchedulerPoolState,
    /// Ready claim count.
    pub ready: u64,
    /// Leased claim count.
    pub claimed: u64,
    /// Expired claims awaiting recovery.
    pub expired: u64,
    /// Oldest scheduled due timestamp.
    pub oldest_due_at: Option<i64>,
    /// Earliest due or lease-recovery timestamp.
    pub next_due_at: Option<i64>,
    /// Current pool in-flight count.
    pub in_flight: usize,
    /// Pool dispatch cap.
    pub max_in_flight: u32,
}

impl std::fmt::Debug for SchedulerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulerService")
            .field("config", &self.config)
            .field("paused", &self.paused.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl SchedulerService {
    /// Bind scheduler authority to the current single-process platform composition.
    #[must_use]
    pub fn new(
        store: Arc<SchedulerStore>,
        storage: Arc<PlatformStorage>,
        transport: WorkerdTransport,
        config: SchedulerConfig,
        clock: Arc<dyn SchedulerClock>,
    ) -> Self {
        let wall = clock.wall_time_ms();
        let wake = WakeCoordinator::new(
            store.wake_signal(),
            clock.clone(),
            Duration::from_millis(config.poll_interval_ms),
        );
        let alarm_enabled = config.pool(SchedulerKind::Alarm).enabled;
        let queue_enabled = config.pool(SchedulerKind::Queue).enabled;
        Self {
            store,
            storage,
            transport,
            config,
            clock,
            observed_wall_floor_ms: AtomicI64::new(wall),
            paused: AtomicBool::new(false),
            alarm_paused: AtomicBool::new(false),
            queue_paused: AtomicBool::new(false),
            alarm_pool_state: AtomicU8::new(encode_pool_state(if alarm_enabled {
                SchedulerPoolState::Ready
            } else {
                SchedulerPoolState::Disabled
            })),
            queue_pool_state: AtomicU8::new(encode_pool_state(if queue_enabled {
                SchedulerPoolState::Ready
            } else {
                SchedulerPoolState::Disabled
            })),
            global_in_flight: AtomicUsize::new(0),
            alarm_in_flight: AtomicUsize::new(0),
            queue_in_flight: AtomicUsize::new(0),
            next_wake_at_ms: AtomicI64::new(-1),
            wake,
            repair_cursor: Mutex::new(None),
            metrics: None,
            health: None,
            #[cfg(any(test, feature = "test-support"))]
            fault_hook: None,
        }
    }

    /// Attach the fixed-series scheduler and alarm metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Attach the scheduler component health coordinator.
    #[must_use]
    pub fn with_health(mut self, health: crate::health::HealthCoordinator) -> Self {
        self.health = Some(health);
        self
    }

    /// Attach a fixed test-support scheduler crash-boundary hook.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_fault_hook(
        mut self,
        hook: Arc<dyn Fn(open_compute_core::SchedulerFaultPoint) + Send + Sync>,
    ) -> Self {
        self.fault_hook = Some(hook);
        self
    }

    /// Independently owned projection database.
    #[must_use]
    pub fn store(&self) -> &Arc<SchedulerStore> {
        &self.store
    }

    /// Stop claiming new jobs while preserving object authority and existing leases.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
        self.wake.notify();
    }

    /// Resume bounded due claims.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
        self.wake.notify();
    }

    /// Whether operator pause currently fences new claims.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Pause one registered fixed workload without affecting other pools.
    pub fn pause_kind(&self, kind: SchedulerKind) -> Result<(), PlatformError> {
        self.ensure_kind_enabled(kind)?;
        match kind {
            SchedulerKind::Alarm => {
                self.alarm_paused.store(true, Ordering::Release);
                self.set_alarm_pool_state(SchedulerPoolState::Paused);
            }
            SchedulerKind::Queue => {
                self.queue_paused.store(true, Ordering::Release);
                self.set_queue_pool_state(SchedulerPoolState::Paused);
            }
            SchedulerKind::Cron | SchedulerKind::Workflow => unreachable!(),
        }
        self.wake.notify();
        Ok(())
    }

    /// Resume one registered fixed workload.
    pub fn resume_kind(&self, kind: SchedulerKind) -> Result<(), PlatformError> {
        self.ensure_kind_enabled(kind)?;
        match kind {
            SchedulerKind::Alarm => {
                self.alarm_paused.store(false, Ordering::Release);
                self.set_alarm_pool_state(SchedulerPoolState::Ready);
            }
            SchedulerKind::Queue => {
                self.queue_paused.store(false, Ordering::Release);
                self.set_queue_pool_state(SchedulerPoolState::Ready);
            }
            SchedulerKind::Cron | SchedulerKind::Workflow => unreachable!(),
        }
        self.wake.notify();
        Ok(())
    }

    /// Whether a fixed workload is process-locally paused.
    pub fn is_kind_paused(&self, kind: SchedulerKind) -> Result<bool, PlatformError> {
        self.ensure_kind_enabled(kind)?;
        Ok(match kind {
            SchedulerKind::Alarm => self.alarm_paused.load(Ordering::Acquire),
            SchedulerKind::Queue => self.queue_paused.load(Ordering::Acquire),
            SchedulerKind::Cron | SchedulerKind::Workflow => unreachable!(),
        })
    }

    /// Low-cardinality state summary for health and operator inspection.
    pub fn summary(&self) -> Result<SchedulerSummary, PlatformError> {
        self.store.summary(self.observed_wall_time_ms())
    }

    /// Versioned global and registered-pool operator state.
    pub fn inspect(&self) -> Result<SchedulerInspectV2, PlatformError> {
        let now_ms = self.observed_wall_time_ms();
        let summary = self.store.workload_summary(now_ms)?;
        let alarm = self.config.pool(SchedulerKind::Alarm);
        let queue = self.config.pool(SchedulerKind::Queue);
        let state = if !alarm.enabled {
            SchedulerPoolState::Disabled
        } else if self.is_paused() || self.alarm_paused.load(Ordering::Acquire) {
            SchedulerPoolState::Paused
        } else {
            self.alarm_pool_state()
        };
        Ok(SchedulerInspectV2 {
            version: 2,
            paused: self.is_paused(),
            global: SchedulerGlobalInspect {
                in_flight: self.global_in_flight.load(Ordering::Acquire),
                max_in_flight: self.config.max_in_flight,
                next_wake_at: atomic_option_i64(&self.next_wake_at_ms),
            },
            pools: vec![
                SchedulerPoolInspect {
                    kind: SchedulerKind::Alarm,
                    enabled: alarm.enabled,
                    state,
                    ready: summary.ready,
                    claimed: summary.claimed,
                    expired: summary.expired,
                    oldest_due_at: summary.oldest_due_at_ms,
                    next_due_at: summary.next_due_at_ms,
                    in_flight: self.alarm_in_flight.load(Ordering::Acquire),
                    max_in_flight: alarm.max_in_flight,
                },
                SchedulerPoolInspect {
                    kind: SchedulerKind::Queue,
                    enabled: queue.enabled,
                    state: if !queue.enabled {
                        SchedulerPoolState::Disabled
                    } else if self.is_paused() || self.queue_paused.load(Ordering::Acquire) {
                        SchedulerPoolState::Paused
                    } else {
                        self.queue_pool_state()
                    },
                    ready: self.store.queue_workload_summary(now_ms)?.ready,
                    claimed: 0,
                    expired: 0,
                    oldest_due_at: self.store.queue_workload_summary(now_ms)?.oldest_due_at_ms,
                    next_due_at: self.store.queue_workload_summary(now_ms)?.next_due_at_ms,
                    in_flight: self.queue_in_flight.load(Ordering::Acquire),
                    max_in_flight: queue.max_in_flight,
                },
            ],
        })
    }

    /// Run generation-safe claim/repair loops, then boundedly drain in-flight dispatches.
    pub async fn run(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        let alarm = self.config.pool(SchedulerKind::Alarm);
        let queue = self.config.pool(SchedulerKind::Queue);
        let pool_caps = SchedulerKind::ALL.map(|kind| {
            usize::try_from(self.config.pool(kind).max_in_flight).unwrap_or(usize::MAX)
        });
        let weights = SchedulerKind::ALL.map(|kind| self.config.pool(kind).weight);
        let mut admission = AdmissionTracker::new(
            usize::try_from(self.config.max_in_flight).unwrap_or(usize::MAX),
            pool_caps,
        );
        let mut selector = FairSelector::new(weights);
        let mut backoff = InfrastructureBackoff::new(
            self.observed_wall_time_ms().unsigned_abs(),
            Duration::from_millis(25),
            Duration::from_secs(5),
        );
        let mut pool = PoolRuntime::ready();
        let mut repair_deadline = self.clock.monotonic_now();
        let mut dispatches = JoinSet::new();

        loop {
            while let Some(completed) = dispatches.try_join_next() {
                release_completed(
                    &completed,
                    &mut admission,
                    &self.global_in_flight,
                    &self.alarm_in_flight,
                );
            }
            if *shutdown.borrow() {
                break;
            }

            let observed_generation = self.wake.generation();
            let now_mono = self.clock.monotonic_now();
            pool.refresh_deadline(now_mono);
            if pool.state() == SchedulerPoolState::CircuitOpen
                && self.alarm_pool_state() == SchedulerPoolState::Ready
            {
                pool.probe_succeeded();
            }
            if self.is_paused() || self.alarm_paused.load(Ordering::Acquire) {
                self.set_alarm_pool_state(SchedulerPoolState::Paused);
            } else {
                self.set_alarm_pool_state(pool.state());
            }

            if now_mono >= repair_deadline {
                if let Err(error) = self.repair_once().await {
                    tracing::warn!(
                        code = error.code().as_str(),
                        "scheduler alarm repair pass failed"
                    );
                }
                repair_deadline = self
                    .clock
                    .monotonic_deadline(Duration::from_millis(self.config.repair_interval_ms));
            }

            let now_ms = self.observed_wall_time_ms();
            let summary = self.pool_summary(now_ms).await?;
            let queue_summary = self.store.queue_workload_summary(now_ms)?;
            set_atomic_option_i64(
                &self.next_wake_at_ms,
                match (summary.next_due_at_ms, queue_summary.next_due_at_ms) {
                    (Some(left), Some(right)) => Some(left.min(right)),
                    (left, right) => left.or(right),
                },
            );
            if let Some(metrics) = &self.metrics {
                metrics.observe_scheduler_workload(SchedulerKind::Alarm, summary, now_ms);
                metrics.observe_scheduler_workload(SchedulerKind::Queue, queue_summary, now_ms);
                if let Ok((messages, bytes)) = self.store.queue_backlog_totals() {
                    metrics.set_queue_backlog(messages, bytes);
                }
            }
            let queue_runnable = queue.enabled
                && !self.is_paused()
                && !self.queue_paused.load(Ordering::Acquire)
                && self.queue_pool_state() != SchedulerPoolState::CircuitOpen;
            if queue_runnable && queue_summary.ready > 0 {
                self.run_queue_maintenance(now_ms, queue.claim_batch).await;
            }
            let pool_runnable = alarm.enabled
                && !self.is_paused()
                && !self.alarm_paused.load(Ordering::Acquire)
                && pool.state() == SchedulerPoolState::Ready;
            let has_ready = summary.ready > 0 || summary.expired > 0;
            if pool_runnable && has_ready && admission.available_global() > 0 {
                let mut ready = [false; SchedulerKind::ALL.len()];
                ready[SchedulerKind::Alarm.index()] = true;
                let selected = selector.select(
                    ready,
                    admission.available_pools(),
                    admission.available_global(),
                );
                let selected_alarm = selected
                    .iter()
                    .filter(|kind| **kind == SchedulerKind::Alarm)
                    .count()
                    .min(usize::try_from(alarm.claim_batch).unwrap_or(usize::MAX));
                if selected_alarm > 0 {
                    match self
                        .claim(u32::try_from(selected_alarm).unwrap_or(u32::MAX))
                        .await
                    {
                        Ok(jobs) => {
                            backoff.reset(SchedulerKind::Alarm);
                            let unused = selected_alarm.saturating_sub(jobs.len());
                            selector.refund(SchedulerKind::Alarm, unused);
                            if !jobs.is_empty() {
                                if !admission.reserve(SchedulerKind::Alarm, jobs.len()) {
                                    return Err(scheduler_task_failed());
                                }
                                store_admission_metrics(
                                    &admission,
                                    &self.global_in_flight,
                                    &self.alarm_in_flight,
                                );
                                for job in jobs {
                                    let service = self.clone();
                                    dispatches.spawn(async move {
                                        service.dispatch_one(job).await;
                                        SchedulerKind::Alarm
                                    });
                                }
                                continue;
                            }
                        }
                        Err(error) if error.code() == ErrorCode::SchedulerCorrupt => {
                            return Err(error);
                        }
                        Err(error) if permanent_pool_error(error.code()) => {
                            pool.permanent_failure();
                            self.set_alarm_pool_state(pool.state());
                            self.observe_pool_health(pool.state());
                        }
                        Err(error) => {
                            let delay = backoff.fail(
                                SchedulerKind::Alarm,
                                infrastructure_error_class(error.code()),
                            );
                            pool.transient_failure(self.clock.monotonic_deadline(delay));
                            self.set_alarm_pool_state(pool.state());
                            tracing::warn!(
                                code = error.code().as_str(),
                                "scheduler due claim entered bounded backoff"
                            );
                        }
                    }
                }
            }

            let mut deadlines = vec![WakeDeadline {
                at: repair_deadline,
                reason: WakeReason::Repair,
            }];
            if pool_runnable && let Some(next_due_at_ms) = summary.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if queue_runnable && let Some(next_due_at_ms) = queue_summary.next_due_at_ms {
                deadlines.push(WakeDeadline {
                    at: self.wake.wall_deadline(now_ms, next_due_at_ms),
                    reason: WakeReason::Due,
                });
            }
            if let Some(retry_at) = pool.retry_at() {
                deadlines.push(WakeDeadline {
                    at: retry_at,
                    reason: WakeReason::Backoff,
                });
            }
            let wait = self.wake.wait(observed_generation, &deadlines);
            tokio::pin!(wait);
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                completed = dispatches.join_next(), if !dispatches.is_empty() => {
                    if let Some(completed) = completed {
                        release_completed(
                            &completed,
                            &mut admission,
                            &self.global_in_flight,
                            &self.alarm_in_flight,
                        );
                    }
                }
                reason = &mut wait => self.observe_wake(reason),
            }
        }

        self.wake.notify();
        let _ = bounded_drain(
            self.clock.as_ref(),
            Duration::from_millis(self.config.shutdown_drain_ms),
            &mut dispatches,
        )
        .await;
        admission.release(
            SchedulerKind::Alarm,
            admission.pool_in_flight(SchedulerKind::Alarm),
        );
        store_admission_metrics(&admission, &self.global_in_flight, &self.alarm_in_flight);
        Ok(())
    }

    /// Claim and deliver one deterministic due batch without real scheduler sleeps.
    pub async fn poll_once(self: &Arc<Self>) -> Result<usize, PlatformError> {
        let alarm = self.config.pool(SchedulerKind::Alarm);
        let completed = self.poll_queue_once()?;
        if !alarm.enabled || self.is_paused() || self.alarm_paused.load(Ordering::Acquire) {
            return Ok(completed);
        }
        let batch = alarm
            .claim_batch
            .min(alarm.max_in_flight)
            .min(self.config.max_in_flight);
        let jobs = self.claim(batch).await?;
        let count = jobs.len();
        stream::iter(jobs)
            .for_each_concurrent(usize::try_from(batch).unwrap_or(usize::MAX), |job| {
                let service = self.clone();
                async move { service.dispatch_one(job).await }
            })
            .await;
        Ok(completed.saturating_add(count))
    }

    fn ensure_kind_enabled(&self, kind: SchedulerKind) -> Result<(), PlatformError> {
        if matches!(kind, SchedulerKind::Alarm | SchedulerKind::Queue)
            && self.config.pool(kind).enabled
        {
            return Ok(());
        }
        Err(PlatformError::new(
            ErrorCode::SchedulerKindNotEnabled,
            "scheduler workload kind is not enabled in this release",
        ))
    }

    fn alarm_pool_state(&self) -> SchedulerPoolState {
        decode_pool_state(self.alarm_pool_state.load(Ordering::Acquire))
    }

    fn set_alarm_pool_state(&self, state: SchedulerPoolState) {
        self.alarm_pool_state
            .store(encode_pool_state(state), Ordering::Release);
        if let Some(metrics) = &self.metrics {
            metrics.set_scheduler_pool_state(SchedulerKind::Alarm, state);
        }
    }

    fn observe_pool_health(&self, state: SchedulerPoolState) {
        let Some(health) = &self.health else {
            return;
        };
        let (component, reason) = match state {
            SchedulerPoolState::CircuitOpen => (
                ComponentState::Degraded,
                ReadinessReason::SchedulerUnavailable,
            ),
            SchedulerPoolState::Backoff => {
                (ComponentState::Degraded, ReadinessReason::SchedulerBacklog)
            }
            SchedulerPoolState::Ready
            | SchedulerPoolState::Paused
            | SchedulerPoolState::Disabled => (ComponentState::Healthy, ReadinessReason::Ready),
        };
        if let Err(error) = health.set_component(ComponentName::Scheduler, component, Some(reason))
        {
            tracing::warn!(
                code = error.code().as_str(),
                "scheduler pool health transition failed"
            );
        }
    }

    fn observe_wake(&self, reason: WakeReason) {
        if let Some(metrics) = &self.metrics {
            metrics.inc_scheduler_wake(reason.as_str());
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn hit_fault(&self, point: open_compute_core::SchedulerFaultPoint) {
        if let Some(hook) = &self.fault_hook {
            hook(point);
        }
    }

    fn observed_wall_time_ms(&self) -> i64 {
        observe_wall_floor(self.clock.as_ref(), &self.observed_wall_floor_ms)
    }

    fn observe_delivery(&self, outcome: AlarmOutcome, retry_count: u8, started: Instant) {
        if let Some(metrics) = &self.metrics {
            metrics.observe_alarm_delivery(
                outcome,
                retry_count,
                self.clock
                    .monotonic_now()
                    .saturating_duration_since(started),
            );
        }
    }

    fn observe_health(&self, summary: SchedulerSummary, now_ms: i64) {
        let Some(health) = &self.health else {
            return;
        };
        let lagged = summary.oldest_due_at_ms.is_some_and(|due| {
            now_ms.saturating_sub(due)
                > i64::try_from(self.config.repair_interval_ms).unwrap_or(i64::MAX)
        });
        let (state, reason) = if self.alarm_pool_state() == SchedulerPoolState::CircuitOpen {
            (
                ComponentState::Degraded,
                ReadinessReason::SchedulerUnavailable,
            )
        } else if lagged || summary.expired_claims > 0 {
            (ComponentState::Degraded, ReadinessReason::SchedulerBacklog)
        } else {
            (ComponentState::Healthy, ReadinessReason::Ready)
        };
        if let Err(error) = health.set_component(ComponentName::Scheduler, state, Some(reason)) {
            tracing::warn!(
                code = error.code().as_str(),
                "scheduler health transition failed"
            );
        }
    }
}

fn release_completed(
    completed: &Result<SchedulerKind, tokio::task::JoinError>,
    admission: &mut AdmissionTracker,
    global_in_flight: &AtomicUsize,
    alarm_in_flight: &AtomicUsize,
) {
    let kind = match completed {
        Ok(kind) => *kind,
        Err(_) => {
            tracing::warn!("scheduler dispatch task failed");
            SchedulerKind::Alarm
        }
    };
    admission.release(kind, 1);
    store_admission_metrics(admission, global_in_flight, alarm_in_flight);
}

fn store_admission_metrics(
    admission: &AdmissionTracker,
    global_in_flight: &AtomicUsize,
    alarm_in_flight: &AtomicUsize,
) {
    global_in_flight.store(admission.global_in_flight(), Ordering::Release);
    alarm_in_flight.store(
        admission.pool_in_flight(SchedulerKind::Alarm),
        Ordering::Release,
    );
}

const fn infrastructure_error_class(code: ErrorCode) -> u64 {
    match code {
        ErrorCode::SchedulerBusy => 1,
        ErrorCode::SchedulerUnavailable => 2,
        ErrorCode::SchedulerInternalProtocolError => 3,
        _ => 4,
    }
}

const fn encode_pool_state(state: SchedulerPoolState) -> u8 {
    match state {
        SchedulerPoolState::Ready => 0,
        SchedulerPoolState::Paused => 1,
        SchedulerPoolState::Backoff => 2,
        SchedulerPoolState::CircuitOpen => 3,
        SchedulerPoolState::Disabled => 4,
    }
}

const fn decode_pool_state(value: u8) -> SchedulerPoolState {
    match value {
        1 => SchedulerPoolState::Paused,
        2 => SchedulerPoolState::Backoff,
        3 => SchedulerPoolState::CircuitOpen,
        4 => SchedulerPoolState::Disabled,
        _ => SchedulerPoolState::Ready,
    }
}

fn set_atomic_option_i64(target: &AtomicI64, value: Option<i64>) {
    target.store(value.unwrap_or(-1), Ordering::Release);
}

fn atomic_option_i64(target: &AtomicI64) -> Option<i64> {
    let value = target.load(Ordering::Acquire);
    (value >= 0).then_some(value)
}

async fn scheduler_timeout<T>(
    clock: &dyn SchedulerClock,
    delay: Duration,
    future: impl Future<Output = T>,
) -> Result<T, ()> {
    let future = future;
    let timer = clock.sleep_until(clock.monotonic_deadline(delay));
    tokio::pin!(future);
    tokio::pin!(timer);
    tokio::select! {
        value = &mut future => Ok(value),
        () = &mut timer => Err(()),
    }
}

struct InFlightMetric(Option<Arc<MetricsRegistry>>);

impl InFlightMetric {
    fn new(metrics: Option<Arc<MetricsRegistry>>) -> Self {
        if let Some(registry) = &metrics {
            registry.adjust_scheduler_in_flight(true);
        }
        Self(metrics)
    }
}

impl Drop for InFlightMetric {
    fn drop(&mut self) {
        if let Some(metrics) = &self.0 {
            metrics.adjust_scheduler_in_flight(false);
        }
    }
}

fn repair_cursor(object: &DurableObjectRecord) -> RepairCursor {
    (
        object.namespace_resource_id,
        object.object_id,
        object.generation,
    )
}

fn scheduler_task_failed() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerUnavailable,
        "scheduler blocking task failed",
    )
}

fn observe_wall_floor(clock: &dyn SchedulerClock, floor: &AtomicI64) -> i64 {
    let observed = clock.wall_time_ms();
    floor.fetch_max(observed, Ordering::AcqRel).max(observed)
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
