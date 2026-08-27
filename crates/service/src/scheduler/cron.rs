//! Native scheduled custom-event adapter for durable Cron logical runs.

use super::{SchedulerService, decode_pool_state, encode_pool_state, scheduler_task_failed};
use crate::metrics::{CronRunOutcome, MetricsRegistry};
use crate::runtime_bridge::{DispatchTarget, ScheduledDispatchRequest};
use open_compute_core::{PlatformError, RequestId, SchedulerKind, SchedulerPoolState};
use open_compute_storage::{
    ClaimedCronRun, CronCompletion, CronCompletionResult, WorkerRepository,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl SchedulerService {
    pub(crate) async fn claim_cron(
        &self,
        batch: u32,
    ) -> Result<Vec<ClaimedCronRun>, PlatformError> {
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        let lease_ms = self.config.claim_lease_ms;
        let grace = self.config.cron_misfire_grace_ms;
        let history_limit = self.config.cron_history_limit;
        let history_retention = self.config.cron_history_retention_ms;
        let (slots, runs) = tokio::task::spawn_blocking(move || {
            let slots = store.project_due_cron_slots(now_ms, grace, batch)?;
            store.gc_cron_history(now_ms, history_retention, history_limit)?;
            let runs = store.claim_cron_runs(now_ms, lease_ms, 250, batch)?;
            Ok::<_, PlatformError>((slots, runs))
        })
        .await
        .map_err(|_| scheduler_task_failed())??;
        if let Some(metrics) = &self.metrics {
            metrics.observe_cron_slots(slots);
        }
        Ok(runs)
    }

    pub(crate) async fn dispatch_cron_run(self: Arc<Self>, run: ClaimedCronRun) {
        let _in_flight = self.metrics.as_ref().map(MetricsRegistry::track_cron);
        let authority = {
            let storage = self.storage.clone();
            let current = run.clone();
            tokio::task::spawn_blocking(move || {
                let workers = WorkerRepository::new(storage.db());
                workers.get_worker(current.account_id, current.worker_id)?;
                workers.get_deployment(current.account_id, current.worker_id, current.deployment_id)
            })
            .await
        };
        let Ok(Ok(deployment)) = authority else {
            if let Some(metrics) = &self.metrics {
                metrics.inc_cron_run(CronRunOutcome::Unknown);
            }
            tracing::warn!("Cron authority lookup failed; claim lease retained");
            return;
        };
        let route_generation = match i64::try_from(run.execution_generation) {
            Ok(value) if value > 0 => value,
            _ => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_cron_run(CronRunOutcome::Unknown);
                }
                return;
            }
        };
        let target = DispatchTarget {
            account_id: run.account_id,
            worker_id: run.worker_id,
            deployment_id: run.deployment_id,
            worker_code_sha256: hex::encode(deployment.worker_code_sha256),
            entrypoint: None,
            route_generation,
            request_id: RequestId::generate(),
        };
        let request = ScheduledDispatchRequest {
            scheduled_time_ms: run.scheduled_at_ms,
            cron: run.expression.clone(),
        };
        let response = self
            .transport
            .dispatch_scheduled(
                &target,
                &request,
                Duration::from_millis(self.config.dispatch_timeout_ms),
            )
            .await;
        let Ok(response) = response else {
            if let Some(metrics) = &self.metrics {
                metrics.inc_cron_run(CronRunOutcome::Unknown);
            }
            tracing::warn!("Cron result is unknown; claim lease retained");
            return;
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
                tracing::warn!(outcome, "Cron outcome is unknown; claim lease retained");
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
