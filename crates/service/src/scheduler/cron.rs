//! Native scheduled custom-event adapter for durable Cron logical runs.

use super::{SchedulerService, decode_pool_state, encode_pool_state, scheduler_task_failed};
use crate::metrics::{CronRunOutcome, MetricsRegistry, SchedulerClaimOutcome};
use crate::runtime_bridge::{DispatchTarget, ScheduledDispatchRequest};
use open_compute_core::{PlatformError, RequestId, SchedulerKind, SchedulerPoolState};
use open_compute_storage::cron::CronRepository;
use open_compute_storage::scheduler::{
    ClaimedCronRun, CronCompletion, CronCompletionResult, CronUnknownReason,
};
use open_compute_storage::worker_repository::WorkerRepository;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

impl SchedulerService {
    pub(crate) async fn claim_cron(
        &self,
        batch: u32,
    ) -> Result<Vec<ClaimedCronRun>, PlatformError> {
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        let grace = self.config.cron_misfire_grace_ms;
        let history_limit = self.config.cron_history_limit;
        let history_retention = self.config.cron_history_retention_ms;
        let max_retries = self.config.cron_max_retries;
        let started = Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            let slots = store.project_due_cron_slots(now_ms, grace, batch)?;
            store.gc_cron_history(now_ms, history_retention, history_limit)?;
            let runs = store.claim_cron_runs(now_ms, max_retries, batch)?;
            Ok::<_, PlatformError>((slots, runs))
        })
        .await
        .map_err(|_| scheduler_task_failed())?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_scheduler_claim_duration(SchedulerKind::Cron, started.elapsed());
            metrics.inc_scheduler_claim(
                SchedulerKind::Cron,
                match &result {
                    Ok((_, runs)) if runs.is_empty() => SchedulerClaimOutcome::Empty,
                    Ok(_) => SchedulerClaimOutcome::Claimed,
                    Err(_) => SchedulerClaimOutcome::Error,
                },
            );
        }
        let (slots, runs) = result?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_cron_slots(slots);
        }
        Ok(runs)
    }

    pub(crate) async fn dispatch_cron_run(self: Arc<Self>, run: ClaimedCronRun) {
        let duration = self.config.claim_lease_ms.min(
            run.dispatch_deadline_at_ms
                .saturating_sub(self.observed_wall_time_ms())
                .max(0) as u64,
        );
        let completion = match tokio::time::timeout(
            Duration::from_millis(duration),
            self.clone().dispatch_cron_attempt(run.clone()),
        )
        .await
        {
            Ok(Ok(completion)) => completion,
            result => {
                let reason = match result {
                    Ok(Err(reason)) => reason,
                    _ => {
                        if let Some(metrics) = &self.metrics {
                            metrics.inc_cron_run(CronRunOutcome::Unknown);
                        }
                        CronUnknownReason::TransportTimeout
                    }
                };
                self.record_cron_unknown(&run, reason).await;
                return;
            }
        };
        let native_success = matches!(completion, CronCompletion::Success);
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        let max_retries = self.config.cron_max_retries;
        match tokio::task::spawn_blocking(move || {
            store.complete_cron_run(&run, completion, now_ms, max_retries)
        })
        .await
        {
            Ok(Ok(CronCompletionResult::Stale)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_scheduler_stale_completion(SchedulerKind::Cron);
                    metrics.inc_cron_stale_completion();
                }
            }
            Ok(Ok(result)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_cron_run(match result {
                        CronCompletionResult::Retried => CronRunOutcome::Retry,
                        CronCompletionResult::Terminal if native_success => CronRunOutcome::Success,
                        CronCompletionResult::Terminal => CronRunOutcome::Exception,
                        CronCompletionResult::Stale => CronRunOutcome::Unknown,
                    });
                }
            }
            Ok(Err(error)) => tracing::warn!(
                code = error.code().as_str(),
                "Cron completion transaction failed"
            ),
            Err(_) => tracing::warn!("Cron completion task failed"),
        }
    }

    async fn dispatch_cron_attempt(
        self: Arc<Self>,
        run: ClaimedCronRun,
    ) -> Result<CronCompletion, CronUnknownReason> {
        let _in_flight = self.metrics.as_ref().map(MetricsRegistry::track_cron);
        let authority = {
            let storage = self.storage.clone();
            let current = run.clone();
            tokio::task::spawn_blocking(move || {
                let workers = WorkerRepository::new(storage.db());
                workers.get_worker(current.instance_id, current.worker_id)?;
                let version = workers.get_version(
                    current.instance_id,
                    current.worker_id,
                    current.version_id,
                )?;
                let activation = CronRepository::new(storage.db())
                    .version_config(current.version_id)?
                    .declarations
                    .into_iter()
                    .find(|declaration| declaration.expression == current.expression)
                    .ok_or_else(scheduler_task_failed)?;
                Ok::<_, PlatformError>((version, activation))
            })
            .await
        };
        let Ok(Ok((version, activation))) = authority else {
            if let Some(metrics) = &self.metrics {
                metrics.inc_cron_run(CronRunOutcome::Unknown);
            }
            tracing::warn!("Cron authority lookup failed; retry outcome recorded");
            return Err(CronUnknownReason::ConnectionLoss);
        };
        let route_generation = match i64::try_from(run.execution_generation) {
            Ok(value) if value > 0 => value,
            _ => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_cron_run(CronRunOutcome::Unknown);
                }
                return Err(CronUnknownReason::MalformedResponse);
            }
        };
        let target = DispatchTarget {
            instance_id: run.instance_id,
            worker_id: run.worker_id,
            version_id: run.version_id,
            worker_code_sha256: hex::encode(version.worker_code_sha256),
            entrypoint: None,
            route_generation,
            request_id: RequestId::generate(),
        };
        let request = ScheduledDispatchRequest {
            scheduled_time_ms: run.scheduled_at_ms,
            cron: run.expression.clone(),
            scheduled_handler: activation.scheduled_handler,
            workflow_bindings: activation.workflow_bindings,
        };
        let response = self
            .transport
            .dispatch_scheduled(
                &target,
                &request,
                Duration::from_millis(self.config.dispatch_timeout_ms),
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let reason = match error.code() {
                    open_compute_core::ErrorCode::SchedulerUnavailable => {
                        CronUnknownReason::TransportTimeout
                    }
                    open_compute_core::ErrorCode::RuntimeUnavailable => {
                        CronUnknownReason::RuntimeGenerationLost
                    }
                    _ => CronUnknownReason::ConnectionLoss,
                };
                if let Some(metrics) = &self.metrics {
                    metrics.inc_cron_run(CronRunOutcome::Unknown);
                }
                tracing::warn!(
                    code = error.code().as_str(),
                    "Cron result is unknown; retry outcome recorded"
                );
                return Err(reason);
            }
        };
        let completion = match response.outcome.as_str() {
            "ok" => CronCompletion::Success,
            "exception" => CronCompletion::Failure {
                no_retry: response.no_retry,
                error_code: "CRON_RUNTIME_EXCEPTION",
            },
            outcome => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_cron_run(CronRunOutcome::Unknown);
                }
                tracing::warn!(outcome, "Cron outcome is unknown; retry required");
                return Err(CronUnknownReason::MalformedResponse);
            }
        };
        Ok(completion)
    }

    async fn record_cron_unknown(&self, run: &ClaimedCronRun, reason: CronUnknownReason) {
        let store = self.store.clone();
        let run = run.clone();
        let now_ms = self.observed_wall_time_ms();
        let max_retries = self.config.cron_max_retries;
        if let Ok(Err(error)) = tokio::task::spawn_blocking(move || {
            store.mark_cron_unknown(&run, reason, now_ms, max_retries)
        })
        .await
        {
            tracing::warn!(
                code = error.code().as_str(),
                "Cron unknown outcome was not recorded"
            );
        }
    }

    pub(super) fn cron_pool_state(&self) -> SchedulerPoolState {
        decode_pool_state(self.cron_pool_state.load(Ordering::Acquire))
    }

    pub(super) fn set_cron_pool_state(&self, state: SchedulerPoolState) {
        self.cron_pool_state
            .store(encode_pool_state(state), Ordering::Release);
        if let Some(metrics) = &self.metrics {
            metrics.set_scheduler_pool_state(SchedulerKind::Cron, state);
        }
    }
}
