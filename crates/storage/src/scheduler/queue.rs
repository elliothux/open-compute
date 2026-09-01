//! Durable Queue producer authority and bounded retention maintenance.

use super::{SchedulerStore, map_sql_error};
use crate::QueueConfig;
use open_compute_core::{
    AccountId, ErrorCode, PlatformError, QueueId, QueueMessageId, WorkloadSummary,
};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

#[path = "queue/helpers.rs"]
pub(super) mod helpers;
#[path = "queue/operations.rs"]
mod operations;
use helpers::*;

/// Queue message content representation persisted by capability version one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueContentType {
    /// UTF-8 JSON bytes.
    Json,
    /// UTF-8 string bytes.
    Text,
    /// Opaque owned bytes.
    Bytes,
    /// Day1 structured-clone `v8` body encoded by the queue-v8 codec.
    V8,
}

impl QueueContentType {
    /// Stable database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
            Self::Bytes => "bytes",
            Self::V8 => "v8",
        }
    }
}

impl FromStr for QueueContentType {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            "bytes" => Ok(Self::Bytes),
            "v8" => Ok(Self::V8),
            _ => Err(PlatformError::new(
                ErrorCode::QueueContentTypeUnsupported,
                "Queue content type is not supported",
            )),
        }
    }
}

/// Exact control-plane Queue projection copied into `scheduler.sqlite`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueProjection {
    /// Queue identity.
    pub queue_id: QueueId,
    /// Owning account.
    pub account_id: AccountId,
    /// Immutable lifecycle generation.
    pub lifecycle_generation: u64,
    /// Mutable send-config generation.
    pub config_generation: u64,
    /// Exact persisted Queue config.
    pub config: QueueConfig,
    /// Original creation timestamp.
    pub created_at_ms: i64,
    /// Projection mutation timestamp.
    pub updated_at_ms: i64,
}

/// One already-serialized producer message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueMessageInput {
    /// Stable supported content representation.
    pub content_type: QueueContentType,
    /// Owned serialized body bytes.
    pub body: Vec<u8>,
    /// Per-message delay override; explicit zero disables inherited delay.
    pub delay_seconds: Option<u32>,
}

/// Trusted batch request after immutable binding authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueEnqueueRequest {
    /// Frozen Queue identity.
    pub queue_id: QueueId,
    /// Stable producer operation identity reused across response-loss retries.
    pub request_id: Uuid,
    /// Whether a Durable Object output intent must explicitly finalize this operation.
    pub output_gate: bool,
    /// Frozen lifecycle generation.
    pub lifecycle_generation: u64,
    /// Current healthy control config generation.
    pub config_generation: u64,
    /// Batch-level delay override.
    pub batch_delay_seconds: Option<u32>,
    /// Non-empty input-order messages.
    pub messages: Vec<QueueMessageInput>,
}

/// Durable backlog summary returned from the committing transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueMetrics {
    /// Retained message count, including delayed messages.
    pub backlog_count: u64,
    /// Retained serialized body bytes.
    pub backlog_bytes: u64,
    /// Oldest enqueue timestamp, if non-empty.
    pub oldest_message_timestamp_ms: Option<i64>,
}

/// Successful enqueue result. Message IDs remain internal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueEnqueueResult {
    /// Generated immutable message identities in input order.
    pub message_ids: Vec<QueueMessageId>,
    /// Transaction-local post-commit backlog summary.
    pub metrics: QueueMetrics,
}

/// Bounded retention or force-delete batch result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueDeleteBatch {
    /// Rows removed.
    pub messages: u64,
    /// Serialized body bytes removed.
    pub bytes: u64,
    /// Earliest remaining expiry in the entire Queue store.
    pub next_expiry_at_ms: Option<i64>,
    /// Whether another already-expired row remains.
    pub expired_remaining: bool,
}

/// Counter mismatch found by a read-only invariant check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueCounterMismatch {
    /// Queue with inconsistent cached counters.
    pub queue_id: QueueId,
    /// Persisted message count.
    pub stored_count: u64,
    /// Actual message row count.
    pub actual_count: u64,
    /// Persisted serialized body bytes.
    pub stored_bytes: u64,
    /// Actual serialized body bytes.
    pub actual_bytes: u64,
}

impl SchedulerStore {
    /// Create one missing exact Queue projection with empty counters.
    pub fn create_queue_projection(
        &self,
        projection: &QueueProjection,
    ) -> Result<(), PlatformError> {
        projection.config.validate()?;
        if projection.lifecycle_generation == 0 || projection.config_generation == 0 {
            return Err(queue_invariant());
        }
        let connection = self.lock()?;
        connection
            .execute(
                "INSERT INTO queue_state
                 (queue_id, account_id, lifecycle_generation, config_generation, state,
                  delivery_delay_seconds, retention_seconds, max_message_bytes,
                  max_batch_messages, max_batch_bytes, max_backlog_bytes,
                  message_count, message_bytes, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'accepting', ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)",
                params![
                    projection.queue_id.to_string(),
                    projection.account_id.to_string(),
                    as_i64(projection.lifecycle_generation)?,
                    as_i64(projection.config_generation)?,
                    i64::from(projection.config.delivery_delay_seconds),
                    i64::from(projection.config.retention_seconds),
                    as_i64(projection.config.max_message_bytes)?,
                    i64::from(projection.config.max_batch_messages),
                    as_i64(projection.config.max_batch_bytes)?,
                    as_i64(projection.config.max_backlog_bytes)?,
                    projection.created_at_ms,
                    projection.updated_at_ms,
                ],
            )
            .map_err(|_| queue_invariant())?;
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Idempotently create or verify one exact projection during reconciliation.
    pub fn ensure_queue_projection(
        &self,
        projection: &QueueProjection,
    ) -> Result<(), PlatformError> {
        projection.config.validate()?;
        let connection = self.lock()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO queue_state
                 (queue_id, account_id, lifecycle_generation, config_generation, state,
                  delivery_delay_seconds, retention_seconds, max_message_bytes,
                  max_batch_messages, max_batch_bytes, max_backlog_bytes,
                  message_count, message_bytes, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'accepting', ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)",
                params![
                    projection.queue_id.to_string(),
                    projection.account_id.to_string(),
                    as_i64(projection.lifecycle_generation)?,
                    as_i64(projection.config_generation)?,
                    i64::from(projection.config.delivery_delay_seconds),
                    i64::from(projection.config.retention_seconds),
                    as_i64(projection.config.max_message_bytes)?,
                    i64::from(projection.config.max_batch_messages),
                    as_i64(projection.config.max_batch_bytes)?,
                    as_i64(projection.config.max_backlog_bytes)?,
                    projection.created_at_ms,
                    projection.updated_at_ms,
                ],
            )
            .map_err(|_| queue_invariant())?;
        drop(connection);
        self.verify_queue_projection(projection)
    }

    /// Verify that an exact accepting projection exists.
    pub fn verify_queue_projection(
        &self,
        projection: &QueueProjection,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let exact: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_state WHERE queue_id = ?1 AND account_id = ?2
                   AND lifecycle_generation = ?3 AND config_generation = ?4
                   AND state = 'accepting' AND delivery_delay_seconds = ?5
                   AND retention_seconds = ?6 AND max_message_bytes = ?7
                   AND max_batch_messages = ?8 AND max_batch_bytes = ?9
                   AND max_backlog_bytes = ?10)",
                params![
                    projection.queue_id.to_string(),
                    projection.account_id.to_string(),
                    as_i64(projection.lifecycle_generation)?,
                    as_i64(projection.config_generation)?,
                    i64::from(projection.config.delivery_delay_seconds),
                    i64::from(projection.config.retention_seconds),
                    as_i64(projection.config.max_message_bytes)?,
                    i64::from(projection.config.max_batch_messages),
                    as_i64(projection.config.max_batch_bytes)?,
                    as_i64(projection.config.max_backlog_bytes)?,
                ],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if !exact {
            return Err(queue_invariant());
        }
        Ok(())
    }

    /// Install the no-send fence before a control config generation changes.
    pub fn begin_queue_config(
        &self,
        queue_id: QueueId,
        lifecycle_generation: u64,
        config_generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE queue_state SET state = 'configuring', updated_at_ms = ?1
                 WHERE queue_id = ?2 AND lifecycle_generation = ?3
                   AND config_generation = ?4 AND state IN ('accepting', 'configuring')",
                params![
                    now_ms,
                    queue_id.to_string(),
                    as_i64(lifecycle_generation)?,
                    as_i64(config_generation)?
                ],
            )
            .map_err(map_sql_error)?;
        if changed != 1 {
            return Err(PlatformError::new(
                ErrorCode::QueueConfigPending,
                "Queue config fence could not be installed",
            ));
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Replace only the next config generation while the Queue is fenced.
    pub fn project_queue_config(&self, projection: &QueueProjection) -> Result<(), PlatformError> {
        projection.config.validate()?;
        let prior = projection
            .config_generation
            .checked_sub(1)
            .ok_or_else(queue_invariant)?;
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE queue_state SET config_generation = ?1, delivery_delay_seconds = ?2,
                        retention_seconds = ?3, max_message_bytes = ?4,
                        max_batch_messages = ?5, max_batch_bytes = ?6,
                        max_backlog_bytes = ?7, updated_at_ms = ?8
                 WHERE queue_id = ?9 AND account_id = ?10 AND lifecycle_generation = ?11
                   AND config_generation = ?12 AND state = 'configuring'",
                params![
                    as_i64(projection.config_generation)?,
                    i64::from(projection.config.delivery_delay_seconds),
                    i64::from(projection.config.retention_seconds),
                    as_i64(projection.config.max_message_bytes)?,
                    i64::from(projection.config.max_batch_messages),
                    as_i64(projection.config.max_batch_bytes)?,
                    as_i64(projection.config.max_backlog_bytes)?,
                    projection.updated_at_ms,
                    projection.queue_id.to_string(),
                    projection.account_id.to_string(),
                    as_i64(projection.lifecycle_generation)?,
                    as_i64(prior)?,
                ],
            )
            .map_err(map_sql_error)?;
        if changed != 1 {
            return Err(queue_invariant());
        }
        Ok(())
    }

    /// Converge a control-authoritative pending config without reopening sends early.
    pub fn reconcile_queue_config(
        &self,
        projection: &QueueProjection,
    ) -> Result<(), PlatformError> {
        projection.config.validate()?;
        let prior = projection
            .config_generation
            .checked_sub(1)
            .ok_or_else(queue_invariant)?;
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql_error)?;
        let current: Option<(i64, String)> = tx
            .query_row(
                "SELECT config_generation, state FROM queue_state
                 WHERE queue_id = ?1 AND account_id = ?2 AND lifecycle_generation = ?3",
                params![
                    projection.queue_id.to_string(),
                    projection.account_id.to_string(),
                    as_i64(projection.lifecycle_generation)?,
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sql_error)?;
        let Some((generation, state)) = current else {
            return Err(queue_invariant());
        };
        let generation = u64::try_from(generation).map_err(|_| queue_invariant())?;
        if generation == prior && matches!(state.as_str(), "accepting" | "configuring") {
            let changed = tx
                .execute(
                    "UPDATE queue_state SET config_generation = ?1, state = 'configuring',
                            delivery_delay_seconds = ?2, retention_seconds = ?3,
                            max_message_bytes = ?4, max_batch_messages = ?5,
                            max_batch_bytes = ?6, max_backlog_bytes = ?7, updated_at_ms = ?8
                     WHERE queue_id = ?9 AND lifecycle_generation = ?10
                       AND config_generation = ?11 AND state IN ('accepting', 'configuring')",
                    params![
                        as_i64(projection.config_generation)?,
                        i64::from(projection.config.delivery_delay_seconds),
                        i64::from(projection.config.retention_seconds),
                        as_i64(projection.config.max_message_bytes)?,
                        i64::from(projection.config.max_batch_messages),
                        as_i64(projection.config.max_batch_bytes)?,
                        as_i64(projection.config.max_backlog_bytes)?,
                        projection.updated_at_ms,
                        projection.queue_id.to_string(),
                        as_i64(projection.lifecycle_generation)?,
                        as_i64(prior)?,
                    ],
                )
                .map_err(map_sql_error)?;
            if changed != 1 {
                return Err(queue_invariant());
            }
        } else if generation == projection.config_generation
            && matches!(state.as_str(), "configuring" | "accepting")
        {
            let exact: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM queue_state WHERE queue_id = ?1
                       AND delivery_delay_seconds = ?2 AND retention_seconds = ?3
                       AND max_message_bytes = ?4 AND max_batch_messages = ?5
                       AND max_batch_bytes = ?6 AND max_backlog_bytes = ?7)",
                    params![
                        projection.queue_id.to_string(),
                        i64::from(projection.config.delivery_delay_seconds),
                        i64::from(projection.config.retention_seconds),
                        as_i64(projection.config.max_message_bytes)?,
                        i64::from(projection.config.max_batch_messages),
                        as_i64(projection.config.max_batch_bytes)?,
                        as_i64(projection.config.max_backlog_bytes)?,
                    ],
                    |row| row.get(0),
                )
                .map_err(map_sql_error)?;
            if !exact {
                return Err(queue_invariant());
            }
        } else {
            return Err(queue_invariant());
        }
        tx.commit().map_err(map_sql_error)?;
        Ok(())
    }

    /// Reopen an exact projected config generation for producer traffic.
    pub fn finish_queue_config(
        &self,
        queue_id: QueueId,
        lifecycle_generation: u64,
        config_generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE queue_state SET state = 'accepting', updated_at_ms = ?1
                 WHERE queue_id = ?2 AND lifecycle_generation = ?3
                   AND config_generation = ?4 AND state IN ('configuring', 'accepting')",
                params![
                    now_ms,
                    queue_id.to_string(),
                    as_i64(lifecycle_generation)?,
                    as_i64(config_generation)?
                ],
            )
            .map_err(map_sql_error)?;
        if changed != 1 {
            return Err(queue_invariant());
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Fence an exact Queue lifecycle generation against stale sends.
    pub fn fence_queue_delete(
        &self,
        queue_id: QueueId,
        lifecycle_generation: u64,
        now_ms: i64,
    ) -> Result<QueueMetrics, PlatformError> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql_error)?;
        let changed = tx
            .execute(
                "UPDATE queue_state SET state = 'deleting', updated_at_ms = ?1
                 WHERE queue_id = ?2 AND lifecycle_generation = ?3
                   AND state IN ('accepting', 'configuring', 'deleting')",
                params![now_ms, queue_id.to_string(), as_i64(lifecycle_generation)?],
            )
            .map_err(map_sql_error)?;
        if changed != 1 {
            return Err(queue_invariant());
        }
        let metrics = metrics_tx(&tx, queue_id)?;
        tx.commit().map_err(map_sql_error)?;
        drop(connection);
        self.wake.notify();
        Ok(metrics)
    }

    /// Delete one empty, already-fenced Queue projection.
    pub fn delete_queue_projection(
        &self,
        queue_id: QueueId,
        lifecycle_generation: u64,
    ) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        connection
            .execute(
                "DELETE FROM queue_enqueue_operations
                 WHERE queue_id = ?1 AND finalized_at_ms IS NOT NULL",
                [queue_id.to_string()],
            )
            .map_err(map_sql_error)?;
        let changed = connection
            .execute(
                "DELETE FROM queue_state WHERE queue_id = ?1 AND lifecycle_generation = ?2
                   AND state = 'deleting' AND message_count = 0 AND message_bytes = 0",
                params![queue_id.to_string(), as_i64(lifecycle_generation)?],
            )
            .map_err(map_sql_error)?;
        if changed != 1 {
            let missing: bool = connection
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM queue_state WHERE queue_id = ?1)",
                    [queue_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(map_sql_error)?;
            if !missing {
                return Err(queue_invariant());
            }
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Read an exact Queue backlog summary without scanning message bodies.
    pub fn queue_metrics(
        &self,
        queue_id: QueueId,
        lifecycle_generation: u64,
        config_generation: u64,
    ) -> Result<QueueMetrics, PlatformError> {
        let connection = self.lock()?;
        let exact: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_state WHERE queue_id = ?1
                   AND lifecycle_generation = ?2 AND config_generation = ?3
                   AND state = 'accepting')",
                params![
                    queue_id.to_string(),
                    as_i64(lifecycle_generation)?,
                    as_i64(config_generation)?
                ],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if !exact {
            return Err(PlatformError::new(
                ErrorCode::QueueConfigPending,
                "Queue projection does not admit metrics",
            ));
        }
        metrics_connection(&connection, queue_id)
    }

    /// Read low-cardinality aggregate backlog gauges without exposing Queue identities.
    pub fn queue_backlog_totals(&self) -> Result<(u64, u64), PlatformError> {
        let connection = self.lock()?;
        let (messages, bytes): (i64, i64) = connection
            .query_row(
                "SELECT COALESCE(SUM(message_count), 0),
                        COALESCE(SUM(message_bytes), 0)
                 FROM queue_state",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(queue_sql_error)?;
        Ok((
            u64::try_from(messages).map_err(|_| queue_invariant())?,
            u64::try_from(bytes).map_err(|_| queue_invariant())?,
        ))
    }

    /// Boundedly delete expired messages across all Queues.
    pub fn sweep_queue_retention(
        &self,
        now_ms: i64,
        max_rows: u32,
        max_bytes: u64,
    ) -> Result<QueueDeleteBatch, PlatformError> {
        if max_rows == 0 || max_bytes == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue sweep budget is invalid",
            ));
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(queue_sql_error)?;
        let candidates = select_delete_candidates(&tx, None, Some(now_ms), max_rows, max_bytes)?;
        let mut bytes = 0_u64;
        for (seq, body_bytes) in &candidates {
            tx.execute(
                "DELETE FROM queue_dlq_pending
                 WHERE message_id = (SELECT id FROM queue_messages WHERE seq = ?1)",
                [seq],
            )
            .map_err(queue_sql_error)?;
            let changed = tx
                .execute(
                    "DELETE FROM queue_messages
                     WHERE seq = ?1 AND state = 'ready' AND expires_at_ms <= ?2",
                    params![seq, now_ms],
                )
                .map_err(queue_sql_error)?;
            if changed != 1 {
                return Err(queue_invariant());
            }
            bytes = bytes.checked_add(*body_bytes).ok_or_else(queue_invariant)?;
        }
        operations::delete_expired_operations(&tx, now_ms, max_rows)?;
        let expired_remaining: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_messages
                  WHERE state = 'ready' AND expires_at_ms <= ?1)
                  OR EXISTS(SELECT 1 FROM queue_enqueue_operations
                  WHERE expires_at_ms <= ?1)",
                [now_ms],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        let next_expiry_at_ms = tx
            .query_row(
                "SELECT MIN(expiry) FROM (
                   SELECT MIN(expires_at_ms) AS expiry FROM queue_messages WHERE state = 'ready'
                   UNION ALL
                   SELECT MIN(expires_at_ms) FROM queue_enqueue_operations
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        tx.commit().map_err(queue_sql_error)?;
        let messages = u64::try_from(candidates.len()).map_err(|_| queue_invariant())?;
        if messages > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(QueueDeleteBatch {
            messages,
            bytes,
            next_expiry_at_ms,
            expired_remaining,
        })
    }

    /// Boundedly purge messages for one deleting Queue.
    pub fn purge_queue(
        &self,
        queue_id: QueueId,
        max_rows: u32,
        max_bytes: u64,
    ) -> Result<QueueDeleteBatch, PlatformError> {
        if max_rows == 0 || max_bytes == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue purge budget is invalid",
            ));
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(queue_sql_error)?;
        let deleting: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_state WHERE queue_id = ?1 AND state = 'deleting')",
                [queue_id.to_string()],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if !deleting {
            return Err(queue_invariant());
        }
        let candidates = select_delete_candidates(&tx, Some(queue_id), None, max_rows, max_bytes)?;
        let mut bytes = 0_u64;
        for (seq, body_bytes) in &candidates {
            tx.execute(
                "DELETE FROM queue_dlq_pending
                 WHERE message_id = (SELECT id FROM queue_messages WHERE seq = ?1)",
                [seq],
            )
            .map_err(queue_sql_error)?;
            let changed = tx
                .execute(
                    "DELETE FROM queue_messages
                     WHERE seq = ?1 AND queue_id = ?2 AND state = 'ready'",
                    params![seq, queue_id.to_string()],
                )
                .map_err(queue_sql_error)?;
            if changed != 1 {
                return Err(queue_invariant());
            }
            bytes = bytes.checked_add(*body_bytes).ok_or_else(queue_invariant)?;
        }
        operations::delete_queue_operations(&tx, queue_id)?;
        let remaining: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM queue_messages WHERE queue_id = ?1)
                  OR EXISTS(SELECT 1 FROM queue_enqueue_operations WHERE queue_id = ?1)",
                [queue_id.to_string()],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        let next_expiry_at_ms = tx
            .query_row(
                "SELECT MIN(expiry) FROM (
                   SELECT MIN(expires_at_ms) AS expiry FROM queue_messages WHERE state = 'ready'
                   UNION ALL
                   SELECT MIN(expires_at_ms) FROM queue_enqueue_operations
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        tx.commit().map_err(queue_sql_error)?;
        let messages = u64::try_from(candidates.len()).map_err(|_| queue_invariant())?;
        if messages > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(QueueDeleteBatch {
            messages,
            bytes,
            next_expiry_at_ms,
            expired_remaining: remaining,
        })
    }

    /// Queue retention workload facts for P2.1 fairness and wake coordination.
    pub fn queue_workload_summary(&self, now_ms: i64) -> Result<WorkloadSummary, PlatformError> {
        let connection = self.lock()?;
        queue_workload_summary_connection(&connection, now_ms)
    }

    /// Read every Queue whose cached counters disagree with message rows.
    pub fn queue_counter_mismatches(&self) -> Result<Vec<QueueCounterMismatch>, PlatformError> {
        let connection = self.lock()?;
        counter_mismatches_connection(&connection)
    }

    /// Offline/operator repair of one exact mismatch using compare-and-set.
    pub fn repair_queue_counter(
        &self,
        mismatch: QueueCounterMismatch,
    ) -> Result<bool, PlatformError> {
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE queue_state SET message_count = ?1, message_bytes = ?2
                 WHERE queue_id = ?3 AND message_count = ?4 AND message_bytes = ?5
                   AND (SELECT COUNT(*) FROM queue_messages WHERE queue_id = ?3) = ?1
                   AND COALESCE((SELECT SUM(body_bytes) FROM queue_messages WHERE queue_id = ?3), 0) = ?2",
                params![
                    as_i64(mismatch.actual_count)?, as_i64(mismatch.actual_bytes)?,
                    mismatch.queue_id.to_string(), as_i64(mismatch.stored_count)?,
                    as_i64(mismatch.stored_bytes)?,
                ],
            )
            .map_err(map_sql_error)?;
        Ok(changed == 1)
    }
}
