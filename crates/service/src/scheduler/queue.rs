//! Queue retention and native push-consumer workload adapter.

use super::{SchedulerService, decode_pool_state, encode_pool_state, scheduler_task_failed};
use crate::metrics::{MetricsRegistry, QueueConsumerBatchOutcome};
use crate::runtime_bridge::{DispatchTarget, QueueDispatchMessage, QueueDispatchRequest};
use base64::Engine as _;
use open_compute_core::{
    ErrorCode, PlatformError, QueueMessageId, RequestId, SchedulerKind, SchedulerPoolState,
};
use open_compute_storage::{
    ClaimedQueueBatch, QueueCompletionAction, QueueCompletionDecision, QueueRepository,
    WorkerRepository,
};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

impl SchedulerService {
    pub(crate) async fn claim_queue_consumers(
        &self,
        batch: u32,
    ) -> Result<Vec<ClaimedQueueBatch>, PlatformError> {
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        let lease_ms = self.config.claim_lease_ms;
        let started = Instant::now();
        let cursor = self
            .queue_claim_cursor
            .lock()
            .map_err(|_| scheduler_task_failed())?
            .to_owned();
        let claimed = tokio::task::spawn_blocking(move || {
            store.claim_queue_batches_after(now_ms, lease_ms, 250, batch, cursor)
        })
        .await
        .map_err(|_| scheduler_task_failed())??;
        if let Some(last) = claimed.last() {
            *self
                .queue_claim_cursor
                .lock()
                .map_err(|_| scheduler_task_failed())? = Some(last.queue_id);
        }
        if let Some(metrics) = &self.metrics {
            metrics.observe_queue_consumer_claim(started.elapsed());
        }
        Ok(claimed)
    }

    pub(crate) async fn dispatch_queue_batch(self: Arc<Self>, batch: ClaimedQueueBatch) {
        let started = Instant::now();
        let _in_flight = self
            .metrics
            .as_ref()
            .map(MetricsRegistry::track_queue_consumer);
        let authority = {
            let storage = self.storage.clone();
            let current = batch.clone();
            tokio::task::spawn_blocking(move || {
                let workers = WorkerRepository::new(storage.db());
                let worker = workers.get_worker(current.account_id, current.worker_id)?;
                let deployment = workers.get_deployment(
                    worker.account_id,
                    current.worker_id,
                    current.deployment_id,
                )?;
                let queue =
                    QueueRepository::new(storage.db()).get(worker.account_id, current.queue_id)?;
                Ok::<_, PlatformError>((worker, deployment, queue.name))
            })
            .await
        };
        let Ok(Ok((worker, deployment, queue_name))) = authority else {
            if let Some(metrics) = &self.metrics {
                metrics.observe_queue_consumer_batch(
                    QueueConsumerBatchOutcome::Unknown,
                    started.elapsed(),
                );
            }
            tracing::warn!("Queue consumer authority lookup failed; claim lease retained");
            return;
        };
        let route_generation = match i64::try_from(batch.execution_generation) {
            Ok(value) if value > 0 => value,
            _ => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_consumer_batch(
                        QueueConsumerBatchOutcome::Unknown,
                        started.elapsed(),
                    );
                }
                return;
            }
        };
        let request = QueueDispatchRequest {
            queue_name,
            messages: batch
                .messages
                .iter()
                .map(|message| QueueDispatchMessage {
                    id: message.id.to_string(),
                    timestamp_ms: message.enqueued_at_ms,
                    attempts: message.delivery_attempt,
                    content_type: message.content_type,
                    body_base64: base64::engine::general_purpose::STANDARD.encode(&message.body),
                })
                .collect(),
        };
        let target = DispatchTarget {
            account_id: worker.account_id,
            worker_id: batch.worker_id,
            deployment_id: batch.deployment_id,
            worker_code_sha256: hex::encode(deployment.worker_code_sha256),
            entrypoint: batch.entrypoint.clone(),
            route_generation,
            request_id: RequestId::generate(),
        };
        let response = self
            .transport
            .dispatch_queue(
                &target,
                &request,
                Duration::from_millis(self.config.dispatch_timeout_ms),
            )
            .await;
        let Ok(response) = response else {
            if let Some(metrics) = &self.metrics {
                metrics.observe_queue_consumer_batch(
                    QueueConsumerBatchOutcome::Unknown,
                    started.elapsed(),
                );
            }
            tracing::warn!("Queue consumer result is unknown; claim lease retained");
            return;
        };
        let known_success = response.outcome == "ok";
        let known_failure = response.outcome == "exception";
        if !known_success && !known_failure {
            if let Some(metrics) = &self.metrics {
                metrics.observe_queue_consumer_batch(
                    QueueConsumerBatchOutcome::Unknown,
                    started.elapsed(),
                );
            }
            tracing::warn!(
                outcome = response.outcome,
                "Queue consumer outcome is unknown"
            );
            return;
        }
        let decisions = match resolve_queue_disposition(&batch, &response, known_success) {
            Ok(decisions) => decisions,
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_consumer_batch(
                        QueueConsumerBatchOutcome::Invalid,
                        started.elapsed(),
                    );
                }
                tracing::warn!(
                    code = error.code().as_str(),
                    "Queue disposition was rejected"
                );
                return;
            }
        };
        if let Some(metrics) = &self.metrics {
            metrics.observe_queue_consumer_batch(
                if known_success {
                    QueueConsumerBatchOutcome::Success
                } else {
                    QueueConsumerBatchOutcome::Exception
                },
                started.elapsed(),
            );
        }
        let store = self.store.clone();
        let now_ms = self.observed_wall_time_ms();
        match tokio::task::spawn_blocking(move || {
            store.complete_queue_batch(&batch, &decisions, now_ms)
        })
        .await
        {
            Ok(Ok(summary)) if summary.stale => {
                if let Some(metrics) = &self.metrics {
                    metrics.inc_scheduler_stale_completion(SchedulerKind::Queue);
                    metrics.inc_queue_consumer_stale_completion();
                }
            }
            Ok(Ok(summary)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_consumer_completion(summary);
                }
            }
            Ok(Err(error)) => tracing::warn!(
                code = error.code().as_str(),
                "Queue consumer completion transaction failed"
            ),
            Err(_) => tracing::warn!("Queue consumer completion task failed"),
        }
    }

    pub(super) async fn run_queue_maintenance(
        &self,
        now_ms: i64,
        rows: u32,
    ) -> Result<(), PlatformError> {
        let store = self.store.clone();
        let result = tokio::task::spawn_blocking(move || {
            let dlq = store.forward_queue_dlq_pending(now_ms, 1_000, rows)?;
            let retention = store.sweep_queue_retention(now_ms, rows, 4 * 1024 * 1024)?;
            let pending = store.queue_dlq_pending_count()?;
            Ok::<_, PlatformError>((dlq, retention, pending))
        })
        .await
        .map_err(|_| scheduler_task_failed())?;
        match result {
            Ok((dlq, batch, pending)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_retention(true, batch.messages, batch.bytes);
                    metrics.observe_queue_dlq_forward(dlq, pending);
                    if let Ok((messages, bytes)) = self.store.queue_backlog_totals() {
                        metrics.set_queue_backlog(messages, bytes);
                    }
                }
                Ok(())
            }
            Err(error) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_retention(false, 0, 0);
                }
                Err(error)
            }
        }
    }

    pub(super) fn poll_queue_once(&self) -> Result<usize, PlatformError> {
        let queue = self.config.pool(SchedulerKind::Queue);
        if !queue.enabled || self.is_paused() || self.queue_paused.load(Ordering::Acquire) {
            return Ok(0);
        }
        let result = self.store.sweep_queue_retention(
            self.observed_wall_time_ms(),
            queue.claim_batch,
            4 * 1024 * 1024,
        )?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_queue_retention(true, result.messages, result.bytes);
            if let Ok((messages, bytes)) = self.store.queue_backlog_totals() {
                metrics.set_queue_backlog(messages, bytes);
            }
        }
        Ok(usize::try_from(result.messages).unwrap_or(usize::MAX))
    }

    pub(super) fn queue_pool_state(&self) -> SchedulerPoolState {
        decode_pool_state(self.queue_pool_state.load(Ordering::Acquire))
    }

    pub(super) fn set_queue_pool_state(&self, state: SchedulerPoolState) {
        self.queue_pool_state
            .store(encode_pool_state(state), Ordering::Release);
        if let Some(metrics) = &self.metrics {
            metrics.set_scheduler_pool_state(SchedulerKind::Queue, state);
        }
    }
}

pub(super) fn resolve_queue_disposition(
    batch: &ClaimedQueueBatch,
    response: &crate::runtime_bridge::QueueDispatchResult,
    success: bool,
) -> Result<Vec<QueueCompletionDecision>, PlatformError> {
    if response.ack_all && response.retry_batch.retry {
        return Err(disposition_invalid());
    }
    let expected: HashSet<_> = batch.messages.iter().map(|message| message.id).collect();
    let mut explicit = HashMap::new();
    for id in &response.explicit_acks {
        let id = QueueMessageId::from_str(id).map_err(|_| disposition_invalid())?;
        if !expected.contains(&id) || explicit.insert(id, QueueCompletionAction::Ack).is_some() {
            return Err(disposition_invalid());
        }
    }
    for retry in &response.retry_messages {
        let id = QueueMessageId::from_str(&retry.msg_id).map_err(|_| disposition_invalid())?;
        let delay_seconds = retry
            .delay_seconds
            .map_or(Ok(batch.retry_delay_seconds), |value| {
                u32::try_from(value).map_err(|_| disposition_invalid())
            })?;
        if !expected.contains(&id)
            || explicit
                .insert(id, QueueCompletionAction::Retry { delay_seconds })
                .is_some()
        {
            return Err(disposition_invalid());
        }
    }
    let batch_delay = response
        .retry_batch
        .delay_seconds
        .map(|value| u32::try_from(value).map_err(|_| disposition_invalid()))
        .transpose()?;
    batch
        .messages
        .iter()
        .map(|message| {
            let action = explicit.get(&message.id).copied().unwrap_or_else(|| {
                if response.retry_batch.retry {
                    QueueCompletionAction::Retry {
                        delay_seconds: batch_delay.unwrap_or(batch.retry_delay_seconds),
                    }
                } else if response.ack_all || success {
                    QueueCompletionAction::Ack
                } else {
                    QueueCompletionAction::Retry {
                        delay_seconds: batch.retry_delay_seconds,
                    }
                }
            });
            Ok(QueueCompletionDecision {
                message_id: message.id,
                action,
            })
        })
        .collect()
}

fn disposition_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueDispositionInvalid,
        "Queue disposition does not exactly match the claimed batch",
    )
}
