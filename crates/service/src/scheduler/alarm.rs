//! Durable Object Alarm workload adapter.

use super::{SchedulerService, repair_cursor, scheduler_task_failed, scheduler_timeout};
use crate::metrics::{AlarmOutcome, AlarmRepairSource, SchedulerClaimOutcome};
use crate::runtime_bridge::{AlarmDispatchOutcome, AlarmRepairResult};
#[cfg(any(test, feature = "test-support"))]
use open_compute_core::SchedulerFaultPoint;
use open_compute_core::{
    ErrorCode, PlatformError, SchedulerKind, SchedulerPoolState, WorkloadSummary,
};
use open_compute_storage::{
    AlarmProjection, ClaimResult, ClaimedJob, DurableObjectRecord, DurableObjectRepository,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl SchedulerService {
    pub(super) async fn claim(&self, batch: u32) -> Result<Vec<ClaimedJob>, PlatformError> {
        let started = self.clock.monotonic_now();
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        let lease_ms = self.config.claim_lease_ms;
        let result = tokio::task::spawn_blocking(move || {
            store.claim_due_with_recovery(now_ms, lease_ms, batch)
        })
        .await
        .map_err(|_| scheduler_task_failed())?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_scheduler_claim_duration(
                SchedulerKind::Alarm,
                self.clock
                    .monotonic_now()
                    .saturating_duration_since(started),
            );
            metrics.inc_scheduler_claim(
                SchedulerKind::Alarm,
                match &result {
                    Ok((jobs, _)) if jobs.is_empty() => SchedulerClaimOutcome::Empty,
                    Ok(_) => SchedulerClaimOutcome::Claimed,
                    Err(_) => SchedulerClaimOutcome::Error,
                },
            );
            if let Ok((_, recovered)) = &result {
                metrics.inc_scheduler_claim_expired(SchedulerKind::Alarm, *recovered);
            }
            if let Ok(summary) = self.store.summary(now_ms) {
                metrics.observe_scheduler_summary(summary, now_ms);
                self.observe_health(summary, now_ms);
            }
        }
        #[cfg(any(test, feature = "test-support"))]
        if result.as_ref().is_ok_and(|(jobs, _)| !jobs.is_empty()) {
            self.hit_fault(SchedulerFaultPoint::AfterClaimCommit);
        }
        result.map(|(jobs, _)| jobs)
    }

    pub(super) async fn dispatch_one(self: Arc<Self>, job: ClaimedJob) {
        let started = self.clock.monotonic_now();
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
        #[cfg(any(test, feature = "test-support"))]
        self.hit_fault(SchedulerFaultPoint::BeforeDispatch);
        let dispatch = self.transport.dispatch_alarm_unbounded(&job);
        let response = match scheduler_timeout(
            self.clock.as_ref(),
            Duration::from_millis(self.config.dispatch_timeout_ms),
            dispatch,
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                tracing::warn!(
                    code = error.code().as_str(),
                    retry_count = job.retry_count,
                    "Durable Object alarm result is unknown; claim lease retained"
                );
                self.observe_delivery(AlarmOutcome::Error, job.retry_count, started);
                return;
            }
            Err(()) => {
                tracing::warn!(
                    retry_count = job.retry_count,
                    timeout_ms = self.config.dispatch_timeout_ms,
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
        #[cfg(any(test, feature = "test-support"))]
        self.hit_fault(SchedulerFaultPoint::AfterDispatchBeforeComplete);
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
                #[cfg(any(test, feature = "test-support"))]
                self.hit_fault(SchedulerFaultPoint::AfterCompleteCommit);
                if let Err(error) = self.finish_discarding(job).await {
                    tracing::warn!(
                        code = error.code().as_str(),
                        "scheduler exhausted alarm projection cleanup failed"
                    );
                }
            }
            Ok(false) => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_scheduler_stale_completion(SchedulerKind::Alarm);
                }
            }
            Ok(true) => {
                #[cfg(any(test, feature = "test-support"))]
                self.hit_fault(SchedulerFaultPoint::AfterCompleteCommit);
            }
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

    /// Run an authenticated repair probe and half-open a recovered Alarm pool.
    pub async fn repair_and_probe(&self) -> Result<u32, PlatformError> {
        let repaired = self.repair_once().await?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.quick_check())
            .await
            .map_err(|_| scheduler_task_failed())??;
        if self.config.pool(SchedulerKind::Alarm).enabled
            && !self.alarm_paused.load(Ordering::Acquire)
        {
            self.set_alarm_pool_state(SchedulerPoolState::Ready);
        }
        self.wake.notify();
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
        let repair = self.transport.repair_alarm_unbounded(
            object.namespace_resource_id,
            object.object_id,
            object.generation,
        );
        let result = scheduler_timeout(
            self.clock.as_ref(),
            Duration::from_millis(self.config.dispatch_timeout_ms),
            repair,
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::DoDispatchTimeout,
                "Durable Object alarm repair result is unknown",
            )
        })??;
        #[cfg(any(test, feature = "test-support"))]
        self.hit_fault(SchedulerFaultPoint::DuringProjectionRefresh);
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

    pub(super) async fn pool_summary(&self, now_ms: i64) -> Result<WorkloadSummary, PlatformError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.workload_summary(now_ms))
            .await
            .map_err(|_| scheduler_task_failed())?
    }
}
