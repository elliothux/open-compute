//! Durable Queue consumer projection, delivery lease, completion, and DLQ authority.

#[path = "queue_consumer/helpers.rs"]
mod helpers;
#[path = "queue_consumer/inspection.rs"]
mod inspection;

use helpers::*;

use super::{SchedulerStore, map_sql_error};
use crate::QueueConsumerConfig;
use open_compute_core::{
    AccountId, DeploymentId, ErrorCode, PlatformError, QueueBatchId, QueueConsumerId, QueueId,
    QueueMessageId, WorkerId, WorkloadSummary,
};
use rand::TryRngCore as _;
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use super::QueueContentType;

/// Exact control-authoritative consumer target copied into the scheduler database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueConsumerProjection {
    /// Live attachment identity.
    pub consumer_id: QueueConsumerId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Monotonic attachment generation.
    pub consumer_generation: u64,
    /// Frozen deployment target.
    pub deployment_id: DeploymentId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen Worker execution generation.
    pub execution_generation: u64,
    /// Optional named entrypoint.
    pub entrypoint: Option<String>,
    /// Frozen delivery policy.
    pub config: QueueConsumerConfig,
    /// Optional dead-letter Queue and lifecycle generation.
    pub dead_letter_queue: Option<(QueueId, u64)>,
    /// Canonical declaration digest.
    pub descriptor_sha256: [u8; 32],
    /// Projection mutation time.
    pub updated_at_ms: i64,
}

/// One immutable message frozen into a durable Queue delivery batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedQueueMessage {
    /// Stable message identity.
    pub id: QueueMessageId,
    /// Original enqueue time exposed as the event timestamp.
    pub enqueued_at_ms: i64,
    /// One-based delivery number exposed to the tenant.
    pub delivery_attempt: u16,
    /// Persisted body representation.
    pub content_type: QueueContentType,
    /// Owned serialized body.
    pub body: Vec<u8>,
}

/// One exact native Queue custom-event claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedQueueBatch {
    /// Durable batch identity.
    pub id: QueueBatchId,
    /// Owning account authority.
    pub account_id: AccountId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Attachment identity.
    pub consumer_id: QueueConsumerId,
    /// Attachment generation fence.
    pub consumer_generation: u64,
    /// Frozen deployment target.
    pub deployment_id: DeploymentId,
    /// Frozen Worker target.
    pub worker_id: WorkerId,
    /// Frozen execution generation.
    pub execution_generation: u64,
    /// Optional named entrypoint.
    pub entrypoint: Option<String>,
    /// Frozen consumer default retry delay.
    pub retry_delay_seconds: u32,
    /// Secret scheduler-only completion fence.
    pub claim_token: [u8; 32],
    /// Persisted lease expiry.
    pub claim_until_ms: i64,
    /// Frozen messages in deterministic order.
    pub messages: Vec<ClaimedQueueMessage>,
}

/// One known native disposition action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueCompletionAction {
    /// Delete the exact claimed message.
    Ack,
    /// Requeue, discard, or dead-letter after one known failed delivery.
    Retry {
        /// Effective retry delay after precedence resolution.
        delay_seconds: u32,
    },
}

/// One exact message decision in a known batch result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueCompletionDecision {
    /// Claimed message identity.
    pub message_id: QueueMessageId,
    /// Final host-resolved action.
    pub action: QueueCompletionAction,
}

/// Atomic completion counts for metrics and operator inspection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueCompletionSummary {
    /// Acknowledged messages.
    pub acknowledged: u64,
    /// Messages scheduled for another product delivery.
    pub retried: u64,
    /// Messages moved atomically to the dead-letter Queue.
    pub dead_lettered: u64,
    /// Terminal messages retained in the pending DLQ intake lane.
    pub dlq_pending: u64,
    /// Terminal messages discarded without a DLQ or after retention expiry.
    pub discarded: u64,
    /// Whether the supplied token/generation was already stale.
    pub stale: bool,
}

/// Result of one bounded DLQ intake maintenance pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueDlqForwardSummary {
    /// Messages moved to their target Queue.
    pub moved: u64,
    /// Source messages discarded after their original retention expired.
    pub expired: u64,
    /// Rows left pending after target backpressure or unavailability.
    pub deferred: u64,
}

/// Secret-free per-consumer runtime facts for authenticated operator inspection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueConsumerRuntimeInspection {
    /// Whether the exact consumer generation projection exists.
    pub projection_exists: bool,
    /// Total retained source Queue messages.
    pub backlog_messages: u64,
    /// Total retained source Queue body bytes.
    pub backlog_bytes: u64,
    /// Ready source messages not held in the DLQ intake lane.
    pub ready_messages: u64,
    /// Claimed batches for this exact generation.
    pub claimed_batches: u64,
    /// Claimed messages for this exact generation.
    pub claimed_messages: u64,
    /// Terminal source messages waiting for DLQ capacity.
    pub dlq_pending: u64,
}

#[derive(Debug)]
struct ConsumerRow {
    consumer_id: QueueConsumerId,
    queue_id: QueueId,
    account_id: AccountId,
    consumer_generation: u64,
    deployment_id: DeploymentId,
    worker_id: WorkerId,
    execution_generation: u64,
    entrypoint: Option<String>,
    config: QueueConsumerConfig,
    dead_letter_queue: Option<(QueueId, u64)>,
}

#[derive(Debug)]
struct MessageRow {
    seq: i64,
    id: QueueMessageId,
    enqueued_at_ms: i64,
    expires_at_ms: i64,
    content_type: QueueContentType,
    body: Vec<u8>,
    attempts: u16,
}

impl SchedulerStore {
    /// Idempotently stage or verify one exact consumer projection.
    pub fn ensure_queue_consumer_projection(
        &self,
        projection: &QueueConsumerProjection,
    ) -> Result<(), PlatformError> {
        projection.config.validate(4096)?;
        if projection.consumer_generation == 0 || projection.execution_generation == 0 {
            return Err(consumer_invariant());
        }
        let dlq_queue_id = projection
            .dead_letter_queue
            .map(|value| value.0.to_string());
        let dlq_generation = projection
            .dead_letter_queue
            .map(|value| as_i64(value.1))
            .transpose()?;
        let connection = self.lock()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO queue_consumer_state
                 (consumer_id, queue_id, consumer_generation, deployment_id, worker_id,
                  execution_generation, entrypoint, state, max_batch_size,
                  max_batch_timeout_ms, max_retries, retry_delay_seconds, max_concurrency,
                  dlq_queue_id, dlq_queue_generation, descriptor_sha256, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'staged', ?8, ?9, ?10, ?11,
                         ?12, ?13, ?14, ?15, ?16)",
                params![
                    projection.consumer_id.to_string(),
                    projection.queue_id.to_string(),
                    as_i64(projection.consumer_generation)?,
                    projection.deployment_id.to_string(),
                    projection.worker_id.to_string(),
                    as_i64(projection.execution_generation)?,
                    projection.entrypoint,
                    i64::from(projection.config.max_batch_size),
                    i64::from(projection.config.max_batch_timeout_seconds) * 1000,
                    i64::from(projection.config.max_retries),
                    i64::from(projection.config.retry_delay_seconds),
                    i64::from(projection.config.max_concurrency),
                    dlq_queue_id,
                    dlq_generation,
                    projection.descriptor_sha256.as_slice(),
                    projection.updated_at_ms,
                ],
            )
            .map_err(consumer_sql_error)?;
        let exact: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_consumer_state
                  WHERE consumer_id = ?1 AND queue_id = ?2 AND consumer_generation = ?3
                    AND deployment_id = ?4 AND worker_id = ?5 AND execution_generation = ?6
                    AND entrypoint IS ?7 AND max_batch_size = ?8
                    AND max_batch_timeout_ms = ?9 AND max_retries = ?10
                    AND retry_delay_seconds = ?11 AND max_concurrency = ?12
                    AND dlq_queue_id IS ?13 AND dlq_queue_generation IS ?14
                    AND descriptor_sha256 = ?15)",
                params![
                    projection.consumer_id.to_string(),
                    projection.queue_id.to_string(),
                    as_i64(projection.consumer_generation)?,
                    projection.deployment_id.to_string(),
                    projection.worker_id.to_string(),
                    as_i64(projection.execution_generation)?,
                    projection.entrypoint,
                    i64::from(projection.config.max_batch_size),
                    i64::from(projection.config.max_batch_timeout_seconds) * 1000,
                    i64::from(projection.config.max_retries),
                    i64::from(projection.config.retry_delay_seconds),
                    i64::from(projection.config.max_concurrency),
                    projection
                        .dead_letter_queue
                        .map(|value| value.0.to_string()),
                    projection
                        .dead_letter_queue
                        .map(|value| as_i64(value.1))
                        .transpose()?,
                    projection.descriptor_sha256.as_slice(),
                ],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if !exact {
            return Err(PlatformError::new(
                ErrorCode::QueueConsumerProjectionPending,
                "Queue consumer projection conflicts with frozen authority",
            ));
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Move an exact staged or paused projection into accepting state.
    pub fn activate_queue_consumer(
        &self,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.set_queue_consumer_state(
            consumer_id,
            consumer_generation,
            "accepting",
            &["staged", "paused", "accepting"],
            now_ms,
        )
    }

    /// Pause one exact consumer generation without changing its backlog.
    pub fn pause_queue_consumer(
        &self,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.set_queue_consumer_state(
            consumer_id,
            consumer_generation,
            "paused",
            &["accepting", "paused"],
            now_ms,
        )
    }

    /// Stop new claims for an exact consumer generation before update or delete.
    pub fn drain_queue_consumer(
        &self,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        self.set_queue_consumer_state(
            consumer_id,
            consumer_generation,
            "draining",
            &["staged", "accepting", "paused", "draining"],
            now_ms,
        )?;
        self.queue_consumer_in_flight(consumer_id, consumer_generation)
    }

    /// Delete a drained projection; already absent is idempotent success.
    pub fn delete_queue_consumer_projection(
        &self,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let in_flight: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_delivery_batches
                  WHERE consumer_id = ?1 AND consumer_generation = ?2)",
                params![consumer_id.to_string(), as_i64(consumer_generation)?],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if in_flight {
            return Err(PlatformError::new(
                ErrorCode::QueueConsumerProjectionPending,
                "Queue consumer still has in-flight batches",
            ));
        }
        connection
            .execute(
                "DELETE FROM queue_consumer_state
                 WHERE consumer_id = ?1 AND consumer_generation = ?2
                   AND state IN ('staged', 'draining', 'deleting')",
                params![consumer_id.to_string(), as_i64(consumer_generation)?],
            )
            .map_err(consumer_sql_error)?;
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Claim with a deterministic wraparound cursor so a busy Queue cannot starve peers.
    pub fn claim_queue_batches(
        &self,
        now_ms: i64,
        lease_ms: u64,
        infrastructure_backoff_ms: u64,
        max_batches: u32,
        after_queue_id: Option<QueueId>,
    ) -> Result<(Vec<ClaimedQueueBatch>, u64), PlatformError> {
        if lease_ms == 0 || max_batches == 0 {
            return Err(consumer_invariant());
        }
        let claim_until_ms = add_ms(now_ms, lease_ms)?;
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(consumer_sql_error)?;
        let recovered = recover_expired_batches_tx(
            &tx,
            now_ms,
            infrastructure_backoff_ms,
            max_batches.saturating_mul(4),
        )?;
        let consumers = eligible_consumers_tx(&tx, now_ms, max_batches, after_queue_id)?;
        let mut batches = Vec::with_capacity(consumers.len());
        for consumer in consumers {
            let messages = due_messages_tx(&tx, &consumer, now_ms)?;
            if messages.is_empty() {
                continue;
            }
            let batch_id = QueueBatchId::generate();
            let claim_token = random_claim_token()?;
            tx.execute(
                "INSERT INTO queue_delivery_batches
                 (id, queue_id, consumer_id, consumer_generation, deployment_id,
                  execution_generation, entrypoint, claim_token, state, claimed_at_ms,
                  claim_until_ms, message_count, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'claimed', ?9, ?10, ?11, ?9)",
                params![
                    batch_id.to_string(),
                    consumer.queue_id.to_string(),
                    consumer.consumer_id.to_string(),
                    as_i64(consumer.consumer_generation)?,
                    consumer.deployment_id.to_string(),
                    as_i64(consumer.execution_generation)?,
                    consumer.entrypoint,
                    claim_token.as_slice(),
                    now_ms,
                    claim_until_ms,
                    i64::try_from(messages.len()).map_err(|_| consumer_invariant())?,
                ],
            )
            .map_err(consumer_sql_error)?;
            for message in &messages {
                let changed = tx
                    .execute(
                        "UPDATE queue_messages
                         SET state = 'claimed', claim_token = ?1, claim_until_ms = ?2,
                             claimed_at_ms = ?3, claim_batch_id = ?4, consumer_id = ?5,
                             consumer_generation = ?6
                         WHERE seq = ?7 AND state = 'ready' AND available_at_ms <= ?3
                           AND expires_at_ms > ?3 AND NOT EXISTS (
                             SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = queue_messages.id
                           )",
                        params![
                            claim_token.as_slice(),
                            claim_until_ms,
                            now_ms,
                            batch_id.to_string(),
                            consumer.consumer_id.to_string(),
                            as_i64(consumer.consumer_generation)?,
                            message.seq,
                        ],
                    )
                    .map_err(consumer_sql_error)?;
                if changed != 1 {
                    return Err(consumer_invariant());
                }
            }
            batches.push(ClaimedQueueBatch {
                id: batch_id,
                account_id: consumer.account_id,
                queue_id: consumer.queue_id,
                consumer_id: consumer.consumer_id,
                consumer_generation: consumer.consumer_generation,
                deployment_id: consumer.deployment_id,
                worker_id: consumer.worker_id,
                execution_generation: consumer.execution_generation,
                entrypoint: consumer.entrypoint,
                retry_delay_seconds: consumer.config.retry_delay_seconds,
                claim_token,
                claim_until_ms,
                messages: messages
                    .into_iter()
                    .map(|message| ClaimedQueueMessage {
                        id: message.id,
                        enqueued_at_ms: message.enqueued_at_ms,
                        delivery_attempt: message.attempts.saturating_add(1),
                        content_type: message.content_type,
                        body: message.body,
                    })
                    .collect(),
            });
        }
        tx.commit().map_err(consumer_sql_error)?;
        Ok((batches, recovered))
    }

    /// Apply a complete known disposition under the exact batch token and generation.
    pub fn complete_queue_batch(
        &self,
        batch: &ClaimedQueueBatch,
        decisions: &[QueueCompletionDecision],
        now_ms: i64,
    ) -> Result<QueueCompletionSummary, PlatformError> {
        if decisions.len() != batch.messages.len() {
            return Err(disposition_invalid());
        }
        let expected: HashSet<_> = batch.messages.iter().map(|message| message.id).collect();
        let supplied: HashSet<_> = decisions
            .iter()
            .map(|decision| decision.message_id)
            .collect();
        if supplied.len() != decisions.len() || supplied != expected {
            return Err(disposition_invalid());
        }
        if decisions.iter().any(|decision| {
            matches!(decision.action, QueueCompletionAction::Retry { delay_seconds } if delay_seconds > 86_400)
        }) {
            return Err(PlatformError::new(
                ErrorCode::QueueRetryDelayInvalid,
                "Queue retry delay is outside 0..86400",
            ));
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(consumer_sql_error)?;
        let authority: Option<(i64, Vec<u8>)> = tx
            .query_row(
                "SELECT consumer_generation, claim_token FROM queue_delivery_batches
                 WHERE id = ?1 AND consumer_id = ?2",
                params![batch.id.to_string(), batch.consumer_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sql_error)?;
        if authority.as_ref().is_none_or(|(generation, token)| {
            u64::try_from(*generation).ok() != Some(batch.consumer_generation)
                || token.as_slice() != batch.claim_token
        }) {
            return Ok(QueueCompletionSummary {
                stale: true,
                ..QueueCompletionSummary::default()
            });
        }
        let consumer = read_consumer_tx(&tx, batch.consumer_id)?;
        if consumer.consumer_generation != batch.consumer_generation {
            return Ok(QueueCompletionSummary {
                stale: true,
                ..QueueCompletionSummary::default()
            });
        }
        let decision_map: HashMap<_, _> = decisions
            .iter()
            .map(|decision| (decision.message_id, decision.action))
            .collect();
        let messages = claimed_messages_tx(&tx, batch)?;
        if messages.len() != decisions.len() {
            return Err(consumer_invariant());
        }
        let mut summary = QueueCompletionSummary::default();
        for message in messages {
            match decision_map
                .get(&message.id)
                .copied()
                .ok_or_else(disposition_invalid)?
            {
                QueueCompletionAction::Ack => {
                    delete_claimed_message_tx(&tx, &message, batch)?;
                    summary.acknowledged += 1;
                }
                QueueCompletionAction::Retry { delay_seconds } => {
                    let delivery = message.attempts.saturating_add(1);
                    if now_ms >= message.expires_at_ms {
                        delete_claimed_message_tx(&tx, &message, batch)?;
                        summary.discarded += 1;
                    } else if u32::from(delivery) <= consumer.config.max_retries {
                        retry_message_tx(&tx, &message, batch, delivery, now_ms, delay_seconds)?;
                        summary.retried += 1;
                    } else if let Some(target) = consumer.dead_letter_queue {
                        if move_to_dlq_tx(&tx, &message, batch, target, delivery, now_ms)? {
                            summary.dead_lettered += 1;
                        } else {
                            retry_message_tx(&tx, &message, batch, delivery, now_ms, 0)?;
                            tx.execute(
                                "INSERT INTO queue_dlq_pending
                                 (message_id, source_queue_id, target_queue_id,
                                  target_queue_generation, terminal_attempts, next_attempt_at_ms,
                                  created_at_ms, last_error_code)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'QUEUE_DLQ_BACKPRESSURED')",
                                params![
                                    message.id.to_string(),
                                    batch.queue_id.to_string(),
                                    target.0.to_string(),
                                    as_i64(target.1)?,
                                    i64::from(delivery),
                                    add_ms(now_ms, 1000)?,
                                    now_ms,
                                ],
                            )
                            .map_err(consumer_sql_error)?;
                            summary.dlq_pending += 1;
                        }
                    } else {
                        delete_claimed_message_tx(&tx, &message, batch)?;
                        summary.discarded += 1;
                    }
                }
            }
        }
        let deleted = tx
            .execute(
                "DELETE FROM queue_delivery_batches
                 WHERE id = ?1 AND consumer_id = ?2 AND consumer_generation = ?3
                   AND claim_token = ?4",
                params![
                    batch.id.to_string(),
                    batch.consumer_id.to_string(),
                    as_i64(batch.consumer_generation)?,
                    batch.claim_token.as_slice(),
                ],
            )
            .map_err(consumer_sql_error)?;
        if deleted != 1 {
            return Err(consumer_invariant());
        }
        tx.commit().map_err(consumer_sql_error)?;
        drop(connection);
        self.wake.notify();
        Ok(summary)
    }

    /// Recover a bounded set of expired Queue batch leases without consuming tenant retries.
    pub fn recover_expired_queue_batches(
        &self,
        now_ms: i64,
        infrastructure_backoff_ms: u64,
        limit: u32,
    ) -> Result<u64, PlatformError> {
        if limit == 0 {
            return Err(consumer_invariant());
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(consumer_sql_error)?;
        let recovered = recover_expired_batches_tx(&tx, now_ms, infrastructure_backoff_ms, limit)?;
        tx.commit().map_err(consumer_sql_error)?;
        if recovered > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(recovered)
    }

    /// Move a bounded number of due DLQ-pending messages atomically when targets admit them.
    pub fn forward_queue_dlq_pending(
        &self,
        now_ms: i64,
        retry_backoff_ms: u64,
        limit: u32,
    ) -> Result<QueueDlqForwardSummary, PlatformError> {
        if limit == 0 {
            return Err(consumer_invariant());
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(consumer_sql_error)?;
        let ids = {
            let mut statement = tx
                .prepare(
                    "SELECT message_id FROM queue_dlq_pending
                     WHERE next_attempt_at_ms <= ?1 ORDER BY next_attempt_at_ms, message_id LIMIT ?2",
                )
                .map_err(map_sql_error)?;
            statement
                .query_map(params![now_ms, i64::from(limit)], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sql_error)?
        };
        let mut summary = QueueDlqForwardSummary::default();
        for id in ids {
            let pending: (String, i64) = tx
                .query_row(
                    "SELECT target_queue_id, target_queue_generation
                     FROM queue_dlq_pending WHERE message_id = ?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(map_sql_error)?;
            let message = ready_message_by_id_tx(&tx, &id)?;
            if now_ms >= message.expires_at_ms {
                tx.execute("DELETE FROM queue_dlq_pending WHERE message_id = ?1", [&id])
                    .map_err(consumer_sql_error)?;
                tx.execute(
                    "DELETE FROM queue_messages WHERE seq = ?1 AND state = 'ready'",
                    [message.seq],
                )
                .map_err(consumer_sql_error)?;
                summary.expired += 1;
                continue;
            }
            let target = (
                QueueId::from_str(&pending.0).map_err(|_| consumer_invariant())?,
                u64::try_from(pending.1).map_err(|_| consumer_invariant())?,
            );
            if move_ready_to_dlq_tx(&tx, &message, target, now_ms)? {
                tx.execute("DELETE FROM queue_dlq_pending WHERE message_id = ?1", [&id])
                    .map_err(consumer_sql_error)?;
                summary.moved += 1;
            } else {
                tx.execute(
                    "UPDATE queue_dlq_pending SET next_attempt_at_ms = ?1,
                            last_error_code = 'QUEUE_DLQ_BACKPRESSURED' WHERE message_id = ?2",
                    params![add_ms(now_ms, retry_backoff_ms)?, id],
                )
                .map_err(consumer_sql_error)?;
                summary.deferred += 1;
            }
        }
        tx.commit().map_err(consumer_sql_error)?;
        if summary.moved > 0 || summary.expired > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(summary)
    }

    fn set_queue_consumer_state(
        &self,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
        target: &str,
        sources: &[&str],
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let placeholders = (0..sources.len())
            .map(|index| format!("?{}", index + 5))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE queue_consumer_state SET state = ?1, updated_at_ms = ?2
             WHERE consumer_id = ?3 AND consumer_generation = ?4 AND state IN ({placeholders})"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![
            target.to_owned().into(),
            now_ms.into(),
            consumer_id.to_string().into(),
            as_i64(consumer_generation)?.into(),
        ];
        values.extend(sources.iter().map(|source| (*source).to_owned().into()));
        let changed = connection
            .execute(&sql, rusqlite::params_from_iter(values))
            .map_err(consumer_sql_error)?;
        if changed != 1 {
            return Err(PlatformError::new(
                ErrorCode::QueueConsumerGenerationStale,
                "Queue consumer generation or state is stale",
            ));
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }
}
