//! Authoritative Queue enqueue idempotency keyed by the producer operation identity.

use super::super::{SchedulerStore, map_sql_error};
use super::helpers::*;
use super::{QueueEnqueueRequest, QueueEnqueueResult, QueueMetrics};
use open_compute_core::{ErrorCode, PlatformError, QueueId, QueueMessageId};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::str::FromStr as _;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredEnqueue {
    message_ids: Vec<String>,
    metrics: QueueMetrics,
}

#[derive(Clone, Copy, Debug)]
struct EnqueueOperationLifetime {
    created_at_ms: i64,
    retention_seconds: u32,
    expires_at_ms: i64,
}

impl SchedulerStore {
    /// Enqueue a non-empty batch atomically and return the original outcome on retry.
    pub fn enqueue_queue(
        &self,
        request: &QueueEnqueueRequest,
        now_ms: i64,
    ) -> Result<QueueEnqueueResult, PlatformError> {
        validate_request(request)?;
        let fingerprint = enqueue_fingerprint(request);
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(queue_sql_error)?;
        if let Some(replay) = replay_enqueue(&tx, request, &fingerprint)? {
            tx.commit().map_err(queue_sql_error)?;
            return Ok(replay);
        }
        let authority = read_state_tx(&tx, request.queue_id)?;
        if authority.lifecycle_generation != request.lifecycle_generation
            || authority.config_generation != request.config_generation
        {
            return Err(queue_invariant());
        }
        match authority.state.as_str() {
            "accepting" => {}
            "configuring" => {
                return Err(PlatformError::new(
                    ErrorCode::QueueConfigPending,
                    "Queue config projection is pending",
                ));
            }
            _ => {
                return Err(PlatformError::new(
                    ErrorCode::QueueNotReady,
                    "Queue does not accept producer messages",
                ));
            }
        }
        validate_dynamic_limits(request, &authority)?;
        let total = request.messages.iter().try_fold(0_u64, |sum, message| {
            sum.checked_add(u64::try_from(message.body.len()).map_err(|_| queue_limit())?)
                .ok_or_else(queue_limit)
        })?;
        if authority.message_bytes.saturating_add(total) > authority.config.max_backlog_bytes {
            return Err(PlatformError::new(
                ErrorCode::QueueBacklogLimitExceeded,
                "Queue backlog byte limit would be exceeded",
            ));
        }
        let expires_at_ms = checked_timestamp(now_ms, authority.config.retention_seconds)?;
        let mut message_ids = Vec::with_capacity(request.messages.len());
        for message in &request.messages {
            let delay = message
                .delay_seconds
                .or(request.batch_delay_seconds)
                .unwrap_or(authority.config.delivery_delay_seconds);
            let available_at_ms = checked_timestamp(now_ms, delay)?;
            let id = QueueMessageId::generate();
            tx.execute(
                "INSERT INTO queue_messages
                 (id, queue_id, queue_generation, enqueued_at_ms, available_at_ms,
                  expires_at_ms, content_type, body, body_bytes, state, attempts,
                  claim_token, claim_until_ms, claimed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'ready', 0, NULL, NULL, NULL)",
                params![
                    id.to_string(),
                    request.queue_id.to_string(),
                    as_i64(request.lifecycle_generation)?,
                    now_ms,
                    available_at_ms,
                    expires_at_ms,
                    message.content_type.as_str(),
                    message.body,
                    as_i64(u64::try_from(message.body.len()).map_err(|_| queue_limit())?)?,
                ],
            )
            .map_err(queue_sql_error)?;
            message_ids.push(id);
        }
        let metrics = metrics_tx(&tx, request.queue_id)?;
        insert_enqueue_operation(
            &tx,
            request,
            &fingerprint,
            &message_ids,
            metrics,
            EnqueueOperationLifetime {
                created_at_ms: now_ms,
                retention_seconds: authority.config.retention_seconds,
                expires_at_ms,
            },
        )?;
        tx.commit().map_err(queue_sql_error)?;
        drop(connection);
        self.wake.notify();
        Ok(QueueEnqueueResult {
            message_ids,
            metrics,
        })
    }
}

pub(super) fn delete_expired_operations(
    tx: &Transaction<'_>,
    now_ms: i64,
    max_rows: u32,
) -> Result<u64, PlatformError> {
    let changed = tx
        .execute(
            "DELETE FROM queue_enqueue_operations WHERE request_id IN (
               SELECT request_id FROM queue_enqueue_operations
                WHERE expires_at_ms <= ?1
                ORDER BY expires_at_ms, request_id
                LIMIT ?2
             )",
            params![now_ms, i64::from(max_rows)],
        )
        .map_err(queue_sql_error)?;
    u64::try_from(changed).map_err(|_| queue_invariant())
}

pub(super) fn delete_queue_operations(
    tx: &Transaction<'_>,
    queue_id: QueueId,
) -> Result<(), PlatformError> {
    tx.execute(
        "DELETE FROM queue_enqueue_operations
         WHERE queue_id = ?1 AND finalized_at_ms IS NOT NULL",
        [queue_id.to_string()],
    )
    .map_err(queue_sql_error)?;
    Ok(())
}

fn enqueue_fingerprint(request: &QueueEnqueueRequest) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(request.queue_id.to_string().as_bytes());
    hasher.update(request.lifecycle_generation.to_be_bytes());
    hasher.update(request.config_generation.to_be_bytes());
    hasher.update([u8::from(request.output_gate)]);
    hasher.update(
        u64::try_from(request.messages.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    hasher.update(i32::from(request.batch_delay_seconds.is_some()).to_be_bytes());
    if let Some(delay) = request.batch_delay_seconds {
        hasher.update(delay.to_be_bytes());
    }
    for message in &request.messages {
        hasher.update(message.content_type.as_str().as_bytes());
        hasher.update([0xff]);
        hasher.update(
            u64::try_from(message.body.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(&message.body);
        match message.delay_seconds {
            Some(delay) => {
                hasher.update([1]);
                hasher.update(delay.to_be_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hasher.finalize().into()
}

fn replay_enqueue(
    tx: &Transaction<'_>,
    request: &QueueEnqueueRequest,
    fingerprint: &[u8; 32],
) -> Result<Option<QueueEnqueueResult>, PlatformError> {
    let existing: Option<(String, i64, Vec<u8>, String)> = tx
        .query_row(
            "SELECT queue_id, queue_generation, fingerprint, response_json
             FROM queue_enqueue_operations WHERE request_id = ?1",
            [request.request_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(map_sql_error)?;
    let Some((queue_id, generation, stored_fingerprint, response_json)) = existing else {
        return Ok(None);
    };
    let generation = u64::try_from(generation).map_err(|_| queue_invariant())?;
    if queue_id != request.queue_id.to_string()
        || generation != request.lifecycle_generation
        || stored_fingerprint.as_slice() != fingerprint.as_slice()
    {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "Queue enqueue operation identity was reused with a different request",
        ));
    }
    let stored: StoredEnqueue =
        serde_json::from_str(&response_json).map_err(|_| queue_invariant())?;
    let mut message_ids = Vec::with_capacity(stored.message_ids.len());
    for id in stored.message_ids {
        message_ids.push(QueueMessageId::from_str(&id).map_err(|_| queue_invariant())?);
    }
    Ok(Some(QueueEnqueueResult {
        message_ids,
        metrics: stored.metrics,
    }))
}

fn insert_enqueue_operation(
    tx: &Transaction<'_>,
    request: &QueueEnqueueRequest,
    fingerprint: &[u8; 32],
    message_ids: &[QueueMessageId],
    metrics: QueueMetrics,
    lifetime: EnqueueOperationLifetime,
) -> Result<(), PlatformError> {
    let stored = StoredEnqueue {
        message_ids: message_ids.iter().map(ToString::to_string).collect(),
        metrics,
    };
    let response_json = serde_json::to_string(&stored).map_err(|_| queue_invariant())?;
    tx.execute(
        "INSERT INTO queue_enqueue_operations
         (request_id, queue_id, queue_generation, fingerprint, response_json,
          output_gate, retention_seconds, created_at_ms, finalized_at_ms, expires_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                 CASE WHEN ?6 = 0 THEN ?8 ELSE NULL END,
                 CASE WHEN ?6 = 0 THEN ?9 ELSE NULL END)",
        params![
            request.request_id.to_string(),
            request.queue_id.to_string(),
            as_i64(request.lifecycle_generation)?,
            fingerprint.as_slice(),
            response_json,
            i64::from(request.output_gate),
            i64::from(lifetime.retention_seconds),
            lifetime.created_at_ms,
            lifetime.expires_at_ms,
        ],
    )
    .map_err(queue_sql_error)?;
    Ok(())
}

impl SchedulerStore {
    /// Finalize one Durable Object enqueue after its local published marker is durable.
    pub fn finalize_queue_enqueue(
        &self,
        request_id: uuid::Uuid,
        queue_id: QueueId,
        lifecycle_generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(queue_sql_error)?;
        let existing: Option<(String, i64, i64, i64, Option<i64>)> = tx
            .query_row(
                "SELECT queue_id, queue_generation, output_gate, retention_seconds, finalized_at_ms
                 FROM queue_enqueue_operations WHERE request_id = ?1",
                [request_id.to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sql_error)?;
        let Some((stored_queue, stored_generation, output_gate, retention, finalized)) = existing
        else {
            tx.commit().map_err(queue_sql_error)?;
            return Ok(());
        };
        if stored_queue != queue_id.to_string()
            || u64::try_from(stored_generation).map_err(|_| queue_invariant())?
                != lifecycle_generation
            || output_gate != 1
        {
            return Err(PlatformError::new(
                ErrorCode::IdempotencyConflict,
                "Queue finalize operation identity does not match its enqueue",
            ));
        }
        if finalized.is_none() {
            let retention = u32::try_from(retention).map_err(|_| queue_invariant())?;
            let expires_at_ms = checked_timestamp(now_ms, retention)?;
            let changed = tx
                .execute(
                    "UPDATE queue_enqueue_operations
                     SET finalized_at_ms = ?1, expires_at_ms = ?2
                     WHERE request_id = ?3 AND finalized_at_ms IS NULL",
                    params![now_ms, expires_at_ms, request_id.to_string()],
                )
                .map_err(queue_sql_error)?;
            if changed != 1 {
                return Err(queue_invariant());
            }
        }
        tx.commit().map_err(queue_sql_error)?;
        Ok(())
    }
}
