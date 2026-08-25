//! Bounded single-process scheduler loop for Durable Object alarms.

use crate::metrics::{AlarmOutcome, AlarmRepairSource, MetricsRegistry, SchedulerClaimOutcome};
use crate::runtime_bridge::{AlarmDispatchOutcome, AlarmRepairResult, WorkerdTransport};
use futures::{StreamExt as _, stream};
use open_compute_core::{
    ComponentName, ComponentState, DurableObjectId, ErrorCode, PlatformError, ReadinessReason,
    ResourceId, SchedulerClock, SchedulerConfig,
};
use open_compute_storage::{
    AlarmProjection, ClaimResult, ClaimedJob, DurableObjectRecord, DurableObjectRepository,
    PlatformStorage, SchedulerStore, SchedulerSummary,
};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;

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
    repair_cursor: Mutex<Option<RepairCursor>>,
    metrics: Option<Arc<MetricsRegistry>>,
    health: Option<crate::health::HealthCoordinator>,
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
        Self {
            store,
            storage,
            transport,
            config,
            clock,
            observed_wall_floor_ms: AtomicI64::new(wall),
            paused: AtomicBool::new(false),
            repair_cursor: Mutex::new(None),
            metrics: None,
            health: None,
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

    /// Independently owned projection database.
    #[must_use]
    pub fn store(&self) -> &Arc<SchedulerStore> {
        &self.store
    }

    /// Stop claiming new jobs while preserving object authority and existing leases.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }

    /// Resume bounded due claims.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }

    /// Whether operator pause currently fences new claims.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Low-cardinality state summary for health and operator inspection.
    pub fn summary(&self) -> Result<SchedulerSummary, PlatformError> {
        self.store.summary(self.observed_wall_time_ms())
    }

    /// Run the poll/repair loops until shutdown, then drain in-flight dispatches boundedly.
    pub async fn run(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), PlatformError> {
        let mut poll = tokio::time::interval(Duration::from_millis(self.config.poll_interval_ms));
        poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut repair =
            tokio::time::interval(Duration::from_millis(self.config.repair_interval_ms));
        repair.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut dispatches = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                completed = dispatches.join_next(), if !dispatches.is_empty() => {
                    if completed.is_some_and(|result| result.is_err()) {
                        tracing::warn!("scheduler dispatch task failed");
                    }
                }
                _ = poll.tick(), if !self.is_paused() => {
                    let available = usize::try_from(self.config.max_in_flight)
                        .unwrap_or(usize::MAX)
                        .saturating_sub(dispatches.len());
                    let batch = available.min(self.config.claim_batch as usize);
                    if batch > 0 {
                        match self.claim(batch as u32).await {
                            Ok(jobs) => {
                                for job in jobs {
                                    let service = self.clone();
                                    dispatches.spawn(async move { service.dispatch_one(job).await });
                                }
                            }
                            Err(error) => tracing::warn!(
                                code = error.code().as_str(),
                                "scheduler due claim failed"
                            ),
                        }
                    }
                }
                _ = repair.tick(), if !self.is_paused() && dispatches.is_empty() => {
                    if let Err(error) = self.repair_once().await {
                        tracing::warn!(
                            code = error.code().as_str(),
                            "scheduler alarm repair pass failed"
                        );
                    }
                }
            }
        }
        let drain = async { while dispatches.join_next().await.is_some() {} };
        if tokio::time::timeout(Duration::from_millis(self.config.shutdown_drain_ms), drain)
            .await
            .is_err()
        {
            dispatches.abort_all();
            while dispatches.join_next().await.is_some() {}
        }
        Ok(())
    }

    /// Claim and deliver one deterministic due batch without real scheduler sleeps.
    pub async fn poll_once(self: &Arc<Self>) -> Result<usize, PlatformError> {
        if self.is_paused() {
            return Ok(0);
        }
        let jobs = self.claim(self.config.claim_batch).await?;
        let count = jobs.len();
        stream::iter(jobs)
            .for_each_concurrent(self.config.max_in_flight as usize, |job| {
                let service = self.clone();
                async move { service.dispatch_one(job).await }
            })
            .await;
        Ok(count)
    }

    async fn claim(&self, batch: u32) -> Result<Vec<ClaimedJob>, PlatformError> {
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        let lease_ms = self.config.claim_lease_ms;
        let result = tokio::task::spawn_blocking(move || {
            store.claim_due_with_recovery(now_ms, lease_ms, batch)
        })
        .await
        .map_err(|_| scheduler_task_failed())?;
        if let Some(metrics) = &self.metrics {
            metrics.inc_scheduler_claim(match &result {
                Ok((jobs, _)) if jobs.is_empty() => SchedulerClaimOutcome::Empty,
                Ok(_) => SchedulerClaimOutcome::Claimed,
                Err(_) => SchedulerClaimOutcome::Error,
            });
            if let Ok((_, recovered)) = &result {
                metrics.inc_scheduler_claim_expired(*recovered);
            }
            if let Ok(summary) = self.store.summary(now_ms) {
                metrics.observe_scheduler_summary(summary, now_ms);
                self.observe_health(summary, now_ms);
            }
        }
        result.map(|(jobs, _)| jobs)
    }

    async fn dispatch_one(self: Arc<Self>, job: ClaimedJob) {
        let started = Instant::now();
        let _in_flight = InFlightMetric::new(self.metrics.clone());
        let authority = match self.authority(&job).await {
            Ok(authority) => authority,
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::DoObjectDeleting
                        | ErrorCode::DoNamespaceNotFound
                        | ErrorCode::DoIdInvalid
                ) =>
            {
                let _ = self.finish(job, ClaimResult::Delete).await;
                self.observe_delivery(AlarmOutcome::Stale, 0, started);
                return;
            }
            Err(error) => {
                tracing::warn!(
                    code = error.code().as_str(),
                    "scheduler alarm authority lookup failed"
                );
                self.observe_delivery(AlarmOutcome::Error, job.retry_count, started);
                return;
            }
        };
        let retargeted = {
            let store = self.store.clone();
            let current = job.clone();
            let deployment = authority.deployment_id;
            let generation = authority.route_generation;
            let now_ms = self.observed_wall_time_ms();
            tokio::task::spawn_blocking(move || {
                store.retarget_claim(&current, deployment, generation, now_ms)
            })
            .await
        };
        if !matches!(retargeted, Ok(Ok(true))) {
            self.observe_delivery(AlarmOutcome::Stale, job.retry_count, started);
            return;
        }
        let response = match self
            .transport
            .dispatch_alarm(&job, Duration::from_millis(self.config.dispatch_timeout_ms))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    code = error.code().as_str(),
                    retry_count = job.retry_count,
                    "Durable Object alarm result is unknown; claim lease retained"
                );
                self.observe_delivery(AlarmOutcome::Error, job.retry_count, started);
                return;
            }
        };
        let metric_outcome = match response.outcome {
            AlarmDispatchOutcome::Success => AlarmOutcome::Success,
            AlarmDispatchOutcome::Stale => AlarmOutcome::Stale,
            AlarmDispatchOutcome::NotDue => AlarmOutcome::NotDue,
            AlarmDispatchOutcome::Retry => AlarmOutcome::Retry,
            AlarmDispatchOutcome::Exhausted => AlarmOutcome::Exhausted,
        };
        let metric_retry_count = response.retry_count.unwrap_or(job.retry_count);
        let result = match response.outcome {
            AlarmDispatchOutcome::Success | AlarmDispatchOutcome::Stale => ClaimResult::Delete,
            AlarmDispatchOutcome::Exhausted => ClaimResult::MarkDiscarding {
                last_error_code: "DO_RUNTIME_EXCEPTION",
            },
            AlarmDispatchOutcome::NotDue | AlarmDispatchOutcome::Retry => {
                let (Some(due_at_ms), Some(retry_count)) =
                    (response.scheduled_time_ms, response.retry_count)
                else {
                    tracing::warn!("private alarm response omitted reschedule authority");
                    return;
                };
                ClaimResult::Reschedule {
                    due_at_ms,
                    retry_count,
                    last_error_code: (response.outcome == AlarmDispatchOutcome::Retry)
                        .then_some("DO_RUNTIME_EXCEPTION"),
                }
            }
        };
        match self.finish(job.clone(), result).await {
            Ok(true) if response.outcome == AlarmDispatchOutcome::Exhausted => {
                if let Err(error) = self.finish_discarding(job).await {
                    tracing::warn!(
                        code = error.code().as_str(),
                        "scheduler exhausted alarm projection cleanup failed"
                    );
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(
                code = error.code().as_str(),
                "scheduler conditional alarm completion failed"
            ),
        }
        self.observe_delivery(metric_outcome, metric_retry_count, started);
    }

    async fn authority(
        &self,
        job: &ClaimedJob,
    ) -> Result<open_compute_storage::AuthorizedDurableObjectDispatch, PlatformError> {
        let storage = self.storage.clone();
        let namespace = job.namespace_resource_id;
        let object = job.object_id;
        let generation = job.object_generation;
        tokio::task::spawn_blocking(move || {
            DurableObjectRepository::new(&storage)
                .authorize_alarm_dispatch(namespace, object, generation)
        })
        .await
        .map_err(|_| scheduler_task_failed())?
    }

    async fn finish(&self, job: ClaimedJob, result: ClaimResult) -> Result<bool, PlatformError> {
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        tokio::task::spawn_blocking(move || store.finish_claim(&job, result, now_ms))
            .await
            .map_err(|_| scheduler_task_failed())?
    }

    async fn finish_discarding(&self, job: ClaimedJob) -> Result<bool, PlatformError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.finish_discarding(&job))
            .await
            .map_err(|_| scheduler_task_failed())?
    }

    /// Probe one stable page of live objects and converge the due projection.
    pub async fn repair_once(&self) -> Result<u32, PlatformError> {
        let after = *self
            .repair_cursor
            .lock()
            .map_err(|_| scheduler_task_failed())?;
        let storage = self.storage.clone();
        let batch = self.config.repair_batch;
        let candidates = tokio::task::spawn_blocking(move || {
            DurableObjectRepository::new(&storage).alarm_repair_candidates(after, batch)
        })
        .await
        .map_err(|_| scheduler_task_failed())??;
        if candidates.is_empty() {
            *self
                .repair_cursor
                .lock()
                .map_err(|_| scheduler_task_failed())? = None;
            return Ok(0);
        }
        let last = candidates.last().map(repair_cursor);
        let mut repaired = 0_u32;
        for object in candidates {
            let result = self.repair_object(object).await;
            if let Some(metrics) = &self.metrics {
                metrics.inc_alarm_repair(AlarmRepairSource::Scan, result.is_ok());
            }
            if result.is_ok() {
                repaired = repaired.saturating_add(1);
            }
        }
        *self
            .repair_cursor
            .lock()
            .map_err(|_| scheduler_task_failed())? = last;
        Ok(repaired)
    }

    async fn repair_object(&self, object: DurableObjectRecord) -> Result<(), PlatformError> {
        let authority = {
            let storage = self.storage.clone();
            tokio::task::spawn_blocking(move || {
                DurableObjectRepository::new(&storage).authorize_alarm_dispatch(
                    object.namespace_resource_id,
                    object.object_id,
                    object.generation,
                )
            })
            .await
            .map_err(|_| scheduler_task_failed())??
        };
        let result = self
            .transport
            .repair_alarm(
                object.namespace_resource_id,
                object.object_id,
                object.generation,
                Duration::from_millis(self.config.dispatch_timeout_ms),
            )
            .await?;
        self.apply_repair(object, authority, result).await
    }

    async fn apply_repair(
        &self,
        object: DurableObjectRecord,
        authority: open_compute_storage::AuthorizedDurableObjectDispatch,
        result: AlarmRepairResult,
    ) -> Result<(), PlatformError> {
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        tokio::task::spawn_blocking(move || {
            if !result.exists {
                store.delete_object(
                    object.namespace_resource_id,
                    object.object_id,
                    object.generation,
                )?;
                return Ok(());
            }
            let (Some(due_at_ms), Some(retry_count), Some(row_token)) = (
                result.scheduled_time_ms,
                result.retry_count,
                result.row_token,
            ) else {
                return Err(scheduler_task_failed());
            };
            store.upsert_alarm(
                &AlarmProjection {
                    namespace_resource_id: object.namespace_resource_id,
                    object_id: object.object_id,
                    object_generation: object.generation,
                    row_token,
                    due_at_ms,
                    target_deployment_id: authority.deployment_id,
                    execution_generation: authority.route_generation,
                    retry_count,
                },
                now_ms,
            )
        })
        .await
        .map_err(|_| scheduler_task_failed())?
    }

    fn observed_wall_time_ms(&self) -> i64 {
        observe_wall_floor(self.clock.as_ref(), &self.observed_wall_floor_ms)
    }

    fn observe_delivery(&self, outcome: AlarmOutcome, retry_count: u8, started: Instant) {
        if let Some(metrics) = &self.metrics {
            metrics.observe_alarm_delivery(outcome, retry_count, started.elapsed());
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
        let (state, reason) = if lagged || summary.expired_claims > 0 {
            (
                ComponentState::Degraded,
                ReadinessReason::SchedulerUnavailable,
            )
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
mod tests {
    use super::*;
    use open_compute_core::DeterministicSchedulerClock;

    #[test]
    fn process_wall_floor_advances_but_never_moves_backwards() {
        let clock = DeterministicSchedulerClock::new(10_000);
        let floor = AtomicI64::new(clock.wall_time_ms());
        clock.set_wall_time_ms(1_000);
        assert_eq!(observe_wall_floor(&clock, &floor), 10_000);
        clock.set_wall_time_ms(20_000);
        assert_eq!(observe_wall_floor(&clock, &floor), 20_000);
        clock.set_wall_time_ms(5_000);
        assert_eq!(observe_wall_floor(&clock, &floor), 20_000);
    }
}
