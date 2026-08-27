//! Queue scheduler SQL validation, mapping, and invariant helpers.

use super::super::{corrupt, map_sql_error};
use super::{QueueCounterMismatch, QueueEnqueueRequest, QueueMetrics};
use crate::{
    QUEUE_MAX_BATCH_BYTES, QUEUE_MAX_BATCH_MESSAGES, QUEUE_MAX_DELAY_SECONDS,
    QUEUE_MAX_MESSAGE_BYTES, QueueConfig,
};
use open_compute_core::{ErrorCode, PlatformError, QueueId, WorkloadSummary};
use rusqlite::{OptionalExtension as _, Transaction, params};
use std::str::FromStr;

#[derive(Debug)]
pub(super) struct QueueStateRow {
    pub(super) lifecycle_generation: u64,
    pub(super) config_generation: u64,
    pub(super) state: String,
    pub(super) config: QueueConfig,
    pub(super) message_bytes: u64,
}

pub(super) fn read_state_tx(
    tx: &Transaction<'_>,
    queue_id: QueueId,
) -> Result<QueueStateRow, PlatformError> {
    tx.query_row(
        "SELECT lifecycle_generation, config_generation, state, delivery_delay_seconds,
                retention_seconds, max_message_bytes, max_batch_messages, max_batch_bytes,
                max_backlog_bytes, message_bytes FROM queue_state WHERE queue_id = ?1",
        [queue_id.to_string()],
        |row| {
            Ok(QueueStateRow {
                lifecycle_generation: u64::try_from(row.get::<_, i64>(0)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                config_generation: u64::try_from(row.get::<_, i64>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                state: row.get(2)?,
                config: QueueConfig {
                    delivery_delay_seconds: u32::try_from(row.get::<_, i64>(3)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    retention_seconds: u32::try_from(row.get::<_, i64>(4)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    max_message_bytes: u64::try_from(row.get::<_, i64>(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    max_batch_messages: u32::try_from(row.get::<_, i64>(6)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    max_batch_bytes: u64::try_from(row.get::<_, i64>(7)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    max_backlog_bytes: u64::try_from(row.get::<_, i64>(8)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                },
                message_bytes: u64::try_from(row.get::<_, i64>(9)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            })
        },
    )
    .optional()
    .map_err(map_sql_error)?
    .ok_or_else(|| PlatformError::new(ErrorCode::QueueNotFound, "Queue projection was not found"))
}

pub(super) fn validate_request(request: &QueueEnqueueRequest) -> Result<(), PlatformError> {
    if request.lifecycle_generation == 0
        || request.config_generation == 0
        || request.messages.is_empty()
    {
        return Err(PlatformError::new(
            ErrorCode::QueueInvalidMessage,
            "Queue batch is empty or invalid",
        ));
    }
    let count = u32::try_from(request.messages.len()).map_err(|_| queue_limit())?;
    if count > QUEUE_MAX_BATCH_MESSAGES {
        return Err(queue_limit());
    }
    validate_delay(request.batch_delay_seconds)?;
    let mut total = 0_u64;
    for message in &request.messages {
        validate_delay(message.delay_seconds)?;
        let len = u64::try_from(message.body.len()).map_err(|_| queue_limit())?;
        if len > QUEUE_MAX_MESSAGE_BYTES {
            return Err(PlatformError::new(
                ErrorCode::QueueMessageTooLarge,
                "Queue message exceeds 128000 bytes",
            ));
        }
        total = total.checked_add(len).ok_or_else(queue_limit)?;
        if total > QUEUE_MAX_BATCH_BYTES {
            return Err(queue_limit());
        }
    }
    Ok(())
}

pub(super) fn validate_dynamic_limits(
    request: &QueueEnqueueRequest,
    state: &QueueStateRow,
) -> Result<(), PlatformError> {
    let count = u32::try_from(request.messages.len()).map_err(|_| queue_limit())?;
    if count > state.config.max_batch_messages {
        return Err(queue_limit());
    }
    let mut total = 0_u64;
    for message in &request.messages {
        let len = u64::try_from(message.body.len()).map_err(|_| queue_limit())?;
        if len > state.config.max_message_bytes {
            return Err(PlatformError::new(
                ErrorCode::QueueMessageTooLarge,
                "Queue message exceeds its configured limit",
            ));
        }
        total = total.checked_add(len).ok_or_else(queue_limit)?;
    }
    if total > state.config.max_batch_bytes {
        return Err(queue_limit());
    }
    Ok(())
}

fn validate_delay(delay: Option<u32>) -> Result<(), PlatformError> {
    if delay.is_some_and(|value| value > QUEUE_MAX_DELAY_SECONDS) {
        return Err(PlatformError::new(
            ErrorCode::QueueDelayInvalid,
            "Queue delay is outside 0..86400",
        ));
    }
    Ok(())
}

pub(super) fn checked_timestamp(now_ms: i64, seconds: u32) -> Result<i64, PlatformError> {
    now_ms
        .checked_add(
            i64::from(seconds)
                .checked_mul(1000)
                .ok_or_else(queue_invariant)?,
        )
        .ok_or_else(|| PlatformError::new(ErrorCode::QueueDelayInvalid, "Queue timestamp overflow"))
}

pub(super) fn metrics_connection(
    connection: &rusqlite::Connection,
    queue_id: QueueId,
) -> Result<QueueMetrics, PlatformError> {
    let (count, bytes): (i64, i64) = connection
        .query_row(
            "SELECT message_count, message_bytes FROM queue_state WHERE queue_id = ?1",
            [queue_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sql_error)?;
    let oldest = connection
        .query_row(
            "SELECT enqueued_at_ms FROM queue_messages WHERE queue_id = ?1 ORDER BY enqueued_at_ms, seq LIMIT 1",
            [queue_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql_error)?;
    queue_metrics_values(count, bytes, oldest)
}

pub(super) fn metrics_tx(
    tx: &Transaction<'_>,
    queue_id: QueueId,
) -> Result<QueueMetrics, PlatformError> {
    let (count, bytes): (i64, i64) = tx
        .query_row(
            "SELECT message_count, message_bytes FROM queue_state WHERE queue_id = ?1",
            [queue_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sql_error)?;
    let oldest = tx
        .query_row(
            "SELECT enqueued_at_ms FROM queue_messages WHERE queue_id = ?1 ORDER BY enqueued_at_ms, seq LIMIT 1",
            [queue_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql_error)?;
    queue_metrics_values(count, bytes, oldest)
}

fn queue_metrics_values(
    count: i64,
    bytes: i64,
    oldest: Option<i64>,
) -> Result<QueueMetrics, PlatformError> {
    let backlog_count = u64::try_from(count).map_err(|_| queue_invariant())?;
    let backlog_bytes = u64::try_from(bytes).map_err(|_| queue_invariant())?;
    if (backlog_count == 0) != oldest.is_none() {
        return Err(queue_invariant());
    }
    Ok(QueueMetrics {
        backlog_count,
        backlog_bytes,
        oldest_message_timestamp_ms: oldest,
    })
}

pub(super) fn select_delete_candidates(
    tx: &Transaction<'_>,
    queue_id: Option<QueueId>,
    expires_before: Option<i64>,
    max_rows: u32,
    max_bytes: u64,
) -> Result<Vec<(i64, u64)>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT seq, body_bytes FROM queue_messages
             WHERE state = 'ready'
               AND (?1 IS NULL OR queue_id = ?1) AND (?2 IS NULL OR expires_at_ms <= ?2)
             ORDER BY expires_at_ms, queue_id, seq LIMIT ?3",
        )
        .map_err(map_sql_error)?;
    let rows = statement
        .query_map(
            params![
                queue_id.map(|id| id.to_string()),
                expires_before,
                i64::from(max_rows)
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(map_sql_error)?;
    let mut output = Vec::new();
    let mut bytes = 0_u64;
    for row in rows {
        let (seq, size) = row.map_err(map_sql_error)?;
        let size = u64::try_from(size).map_err(|_| queue_invariant())?;
        if !output.is_empty() && bytes.saturating_add(size) > max_bytes {
            break;
        }
        bytes = bytes.checked_add(size).ok_or_else(queue_invariant)?;
        output.push((seq, size));
    }
    Ok(output)
}

pub(in crate::scheduler) fn queue_workload_summary_connection(
    connection: &rusqlite::Connection,
    now_ms: i64,
) -> Result<WorkloadSummary, PlatformError> {
    let (ready, oldest, next): (i64, Option<i64>, Option<i64>) = connection
        .query_row(
            "SELECT COUNT(*) FILTER (WHERE state = 'ready' AND expires_at_ms <= ?1),
                    MIN(expires_at_ms) FILTER (WHERE state = 'ready' AND expires_at_ms <= ?1),
                    MIN(expires_at_ms) FILTER (WHERE state = 'ready')
             FROM queue_messages",
            [now_ms],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(map_sql_error)?;
    Ok(WorkloadSummary {
        ready: u64::try_from(ready).map_err(|_| corrupt())?,
        claimed: 0,
        expired: 0,
        oldest_due_at_ms: oldest,
        next_due_at_ms: next,
    })
}

pub(in crate::scheduler) fn counter_mismatches_connection(
    connection: &rusqlite::Connection,
) -> Result<Vec<QueueCounterMismatch>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT q.queue_id, q.message_count, COUNT(m.seq), q.message_bytes,
                    COALESCE(SUM(m.body_bytes), 0)
             FROM queue_state q LEFT JOIN queue_messages m ON m.queue_id = q.queue_id
             GROUP BY q.queue_id
             HAVING q.message_count != COUNT(m.seq)
                OR q.message_bytes != COALESCE(SUM(m.body_bytes), 0)",
        )
        .map_err(map_sql_error)?;
    let rows = statement
        .query_map([], |row| {
            let id: String = row.get(0)?;
            Ok(QueueCounterMismatch {
                queue_id: QueueId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                stored_count: u64::try_from(row.get::<_, i64>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                actual_count: u64::try_from(row.get::<_, i64>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                stored_bytes: u64::try_from(row.get::<_, i64>(3)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                actual_bytes: u64::try_from(row.get::<_, i64>(4)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            })
        })
        .map_err(map_sql_error)?;
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(|_| queue_invariant())?);
    }
    Ok(output)
}

pub(super) fn as_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| queue_invariant())
}

#[allow(clippy::needless_pass_by_value)] // `rusqlite::Result::map_err` passes its error by value.
pub(super) fn queue_sql_error(error: rusqlite::Error) -> PlatformError {
    let message = error.to_string();
    if message.contains("queue message authority invariant") {
        PlatformError::new(
            ErrorCode::QueueInvariantViolation,
            "Queue message authority rejected the write",
        )
    } else if message.contains("database is locked") || message.contains("database is busy") {
        PlatformError::new(
            ErrorCode::QueueStorageUnavailable,
            "Queue storage is temporarily busy",
        )
    } else {
        PlatformError::new(
            ErrorCode::QueueStorageUnavailable,
            "Queue storage operation failed",
        )
    }
}

pub(super) fn queue_limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueBatchLimitExceeded,
        "Queue batch limit exceeded",
    )
}

pub(super) fn queue_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue scheduler invariant failed",
    )
}
