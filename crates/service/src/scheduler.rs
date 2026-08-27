//! P2.1 multi-workload scheduler kernel with the production Alarm adapter.

#[path = "scheduler/alarm.rs"]
mod alarm;
#[path = "scheduler/cron.rs"]
mod cron;
#[path = "scheduler/fairness.rs"]
mod fairness;
#[path = "scheduler/kernel.rs"]
mod kernel;
#[path = "scheduler/operator.rs"]
mod operator;
#[path = "scheduler/queue.rs"]
mod queue;
#[path = "scheduler/runner.rs"]
mod runner;
#[path = "scheduler/wake.rs"]
mod wake;
#[path = "scheduler/workflow.rs"]
mod workflow;

use crate::metrics::{AlarmOutcome, MetricsRegistry};
use crate::runtime_bridge::WorkerdTransport;
use fairness::FairSelector;
use futures::{StreamExt as _, stream};
use kernel::{
    AdmissionTracker, InfrastructureBackoff, PoolRuntime, bounded_drain, permanent_pool_error,
};
use open_compute_core::{
    AccountId, ComponentName, ComponentState, CronActivationId, DeploymentId, DurableObjectId,
    ErrorCode, PlatformError, QueueConsumerId, QueueId, ReadinessReason, ResourceId,
    SchedulerClock, SchedulerConfig, SchedulerKind, SchedulerPoolState, WorkerId,
};
use open_compute_storage::{
    CronActivationState, DurableObjectRecord, PlatformStorage, QueueConsumerState, SchedulerStore,
    SchedulerSummary,
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
    workflows: open_compute_core::WorkflowsConfig,
    workflow_reconcile_cursor: Mutex<open_compute_workers::WorkflowReconcileCursor>,
    workflow_infra_failures: AtomicUsize,
    workflow_version_cursor: Mutex<Option<open_compute_core::WorkflowVersionId>>,
    workflow_artifact_cursor: Mutex<Option<open_compute_core::WorkflowInstanceId>>,
    clock: Arc<dyn SchedulerClock>,
    observed_wall_floor_ms: AtomicI64,
    paused: AtomicBool,
    alarm_paused: AtomicBool,
    queue_paused: AtomicBool,
    cron_paused: AtomicBool,
    workflow_paused: AtomicBool,
    alarm_pool_state: AtomicU8,
    queue_pool_state: AtomicU8,
    cron_pool_state: AtomicU8,
    workflow_pool_state: AtomicU8,
    global_in_flight: AtomicUsize,
    alarm_in_flight: AtomicUsize,
    queue_in_flight: AtomicUsize,
    cron_in_flight: AtomicUsize,
    workflow_in_flight: AtomicUsize,
    next_wake_at_ms: AtomicI64,
    wake: WakeCoordinator,
    repair_cursor: Mutex<Option<RepairCursor>>,
    queue_claim_cursor: Mutex<Option<QueueId>>,
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
    /// Bounded Queue consumer operator details without message bodies or claim tokens.
    pub queue_consumers: Vec<QueueConsumerInspect>,
    /// Bounded Cron activation operator details.
    pub cron_activations: Vec<CronActivationInspect>,
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

/// One authenticated Queue consumer operator view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConsumerInspect {
    /// Live attachment identity.
    pub id: QueueConsumerId,
    /// Owning account.
    pub account_id: AccountId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen target deployment.
    pub deployment_id: DeploymentId,
    /// Persisted next target while the old generation drains.
    pub pending_deployment_id: Option<DeploymentId>,
    /// Exact completion/claim generation.
    pub generation: u64,
    /// Control lifecycle state.
    pub state: QueueConsumerState,
    /// Whether the exact scheduler projection exists.
    pub projection_exists: bool,
    /// Total retained source messages.
    pub backlog_messages: u64,
    /// Total retained source body bytes.
    pub backlog_bytes: u64,
    /// Ready source messages.
    pub ready_messages: u64,
    /// Claimed batches for this generation.
    pub claimed_batches: u64,
    /// Claimed messages for this generation.
    pub claimed_messages: u64,
    /// Terminal messages waiting for DLQ capacity.
    pub dlq_pending: u64,
}

/// One authenticated Cron activation operator view.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronActivationInspect {
    /// Live activation identity.
    pub id: CronActivationId,
    /// Owning account.
    pub account_id: AccountId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen target deployment.
    pub deployment_id: DeploymentId,
    /// Exact declared expression.
    pub expression: String,
    /// Parser contract version.
    pub parser_version: u32,
    /// Exact activation generation.
    pub generation: u64,
    /// Control lifecycle state.
    pub state: CronActivationState,
    /// Whether the exact scheduler projection exists.
    pub projection_exists: bool,
    /// Scheduler projection state.
    pub schedule_state: Option<String>,
    /// Next logical UTC slot.
    pub next_fire_at: Option<i64>,
    /// Ready logical runs.
    pub ready_runs: u64,
    /// Claimed logical runs.
    pub claimed_runs: u64,
    /// Last retained terminal outcome.
    pub last_outcome: Option<String>,
    /// Oldest ready logical-run lag.
    pub lag_ms: u64,
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
        workflows: open_compute_core::WorkflowsConfig,
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
        let cron_enabled = config.pool(SchedulerKind::Cron).enabled;
        let workflow_enabled = config.pool(SchedulerKind::Workflow).enabled;
        Self {
            store,
            storage,
            transport,
            config,
            workflows,
            workflow_reconcile_cursor: Mutex::new(
                open_compute_workers::WorkflowReconcileCursor::default(),
            ),
            workflow_infra_failures: AtomicUsize::new(0),
            workflow_version_cursor: Mutex::new(None),
            workflow_artifact_cursor: Mutex::new(None),
            clock,
            observed_wall_floor_ms: AtomicI64::new(wall),
            paused: AtomicBool::new(false),
            alarm_paused: AtomicBool::new(false),
            queue_paused: AtomicBool::new(false),
            cron_paused: AtomicBool::new(false),
            workflow_paused: AtomicBool::new(false),
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
            cron_pool_state: AtomicU8::new(encode_pool_state(if cron_enabled {
                SchedulerPoolState::Ready
            } else {
                SchedulerPoolState::Disabled
            })),
            workflow_pool_state: AtomicU8::new(encode_pool_state(if workflow_enabled {
                SchedulerPoolState::Ready
            } else {
                SchedulerPoolState::Disabled
            })),
            global_in_flight: AtomicUsize::new(0),
            alarm_in_flight: AtomicUsize::new(0),
            queue_in_flight: AtomicUsize::new(0),
            cron_in_flight: AtomicUsize::new(0),
            workflow_in_flight: AtomicUsize::new(0),
            next_wake_at_ms: AtomicI64::new(-1),
            wake,
            repair_cursor: Mutex::new(None),
            queue_claim_cursor: Mutex::new(None),
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
            SchedulerKind::Cron => {
                self.cron_paused.store(true, Ordering::Release);
                self.set_cron_pool_state(SchedulerPoolState::Paused);
            }
            SchedulerKind::Workflow => {
                self.workflow_paused.store(true, Ordering::Release);
                self.set_workflow_pool_state(SchedulerPoolState::Paused);
            }
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
            SchedulerKind::Cron => {
                self.cron_paused.store(false, Ordering::Release);
                self.set_cron_pool_state(SchedulerPoolState::Ready);
            }
            SchedulerKind::Workflow => {
                self.workflow_paused.store(false, Ordering::Release);
                self.workflow_infra_failures.store(0, Ordering::Release);
                self.set_workflow_pool_state(SchedulerPoolState::Ready);
            }
        }
        self.wake.notify();
        Ok(())
    }

    fn ensure_kind_enabled(&self, kind: SchedulerKind) -> Result<(), PlatformError> {
        if self.config.pool(kind).enabled {
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
        let states = [
            state,
            self.alarm_pool_state(),
            self.queue_pool_state(),
            self.cron_pool_state(),
            self.workflow_pool_state(),
        ];
        let state = if states.contains(&SchedulerPoolState::CircuitOpen) {
            SchedulerPoolState::CircuitOpen
        } else if states.contains(&SchedulerPoolState::Backoff) {
            SchedulerPoolState::Backoff
        } else {
            state
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
        let pool_states = [
            self.alarm_pool_state(),
            self.queue_pool_state(),
            self.cron_pool_state(),
            self.workflow_pool_state(),
        ];
        let (state, reason) = if pool_states.contains(&SchedulerPoolState::CircuitOpen) {
            (
                ComponentState::Degraded,
                ReadinessReason::SchedulerUnavailable,
            )
        } else if lagged
            || summary.expired_claims > 0
            || pool_states.contains(&SchedulerPoolState::Backoff)
        {
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
    kind: SchedulerKind,
    admission: &mut AdmissionTracker,
    global_in_flight: &AtomicUsize,
    alarm_in_flight: &AtomicUsize,
    queue_in_flight: &AtomicUsize,
    cron_in_flight: &AtomicUsize,
    workflow_in_flight: &AtomicUsize,
) {
    admission.release(kind, 1);
    store_admission_metrics(
        admission,
        global_in_flight,
        alarm_in_flight,
        queue_in_flight,
        cron_in_flight,
        workflow_in_flight,
    );
}

fn completed_kind(
    completed: Result<(tokio::task::Id, SchedulerKind), tokio::task::JoinError>,
    kinds: &mut std::collections::HashMap<tokio::task::Id, SchedulerKind>,
) -> Result<SchedulerKind, PlatformError> {
    let id = match completed {
        Ok((id, _)) => id,
        Err(error) => {
            tracing::warn!("scheduler dispatch task failed; lease retained");
            error.id()
        }
    };
    kinds.remove(&id).ok_or_else(scheduler_task_failed)
}

fn store_admission_metrics(
    admission: &AdmissionTracker,
    global_in_flight: &AtomicUsize,
    alarm_in_flight: &AtomicUsize,
    queue_in_flight: &AtomicUsize,
    cron_in_flight: &AtomicUsize,
    workflow_in_flight: &AtomicUsize,
) {
    global_in_flight.store(admission.global_in_flight(), Ordering::Release);
    alarm_in_flight.store(
        admission.pool_in_flight(SchedulerKind::Alarm),
        Ordering::Release,
    );
    queue_in_flight.store(
        admission.pool_in_flight(SchedulerKind::Queue),
        Ordering::Release,
    );
    cron_in_flight.store(
        admission.pool_in_flight(SchedulerKind::Cron),
        Ordering::Release,
    );
    workflow_in_flight.store(
        admission.pool_in_flight(SchedulerKind::Workflow),
        Ordering::Release,
    );
}

fn minimum_timestamp<const N: usize>(values: [Option<i64>; N]) -> Option<i64> {
    values.into_iter().flatten().min()
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

fn queue_consumer_generation_stale() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueConsumerGenerationStale,
        "Queue consumer generation is stale",
    )
}

fn observe_wall_floor(clock: &dyn SchedulerClock, floor: &AtomicI64) -> i64 {
    let observed = clock.wall_time_ms();
    floor.fetch_max(observed, Ordering::AcqRel).max(observed)
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
