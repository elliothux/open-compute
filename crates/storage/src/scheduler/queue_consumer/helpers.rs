//! Transaction-local Queue consumer claim, completion, and DLQ helpers.

use super::*;

pub(super) fn eligible_consumers_tx(
    tx: &Transaction<'_>,
    now_ms: i64,
    limit: u32,
    after_queue_id: Option<QueueId>,
) -> Result<Vec<ConsumerRow>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT c.consumer_id, c.queue_id, q.account_id, c.consumer_generation, c.version_id,
                    c.worker_id, c.execution_generation, c.entrypoint, c.max_batch_size,
                    c.max_batch_timeout_ms, c.max_retries, c.retry_delay_seconds,
                    c.max_concurrency, c.dlq_queue_id, c.dlq_queue_generation
             FROM queue_consumer_state c JOIN queue_state q ON q.queue_id = c.queue_id
             WHERE c.state = 'accepting'
               AND (SELECT COUNT(*) FROM queue_delivery_batches b
                    WHERE b.consumer_id = c.consumer_id
                      AND b.consumer_generation = c.consumer_generation) < c.max_concurrency
               AND EXISTS (
                 SELECT 1 FROM queue_messages m WHERE m.queue_id = c.queue_id
                   AND m.state = 'ready' AND m.available_at_ms <= ?1 AND m.expires_at_ms > ?1
                   AND NOT EXISTS (SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = m.id)
                 GROUP BY m.queue_id
                 HAVING COUNT(*) >= c.max_batch_size
                    OR ?1 >= MIN(m.available_at_ms) + c.max_batch_timeout_ms
               )
             ORDER BY CASE WHEN ?2 IS NULL OR c.queue_id > ?2 THEN 0 ELSE 1 END,
                      c.queue_id, c.consumer_id LIMIT ?3",
        )
        .map_err(map_sql_error)?;
    let rows = statement
        .query_map(
            params![
                now_ms,
                after_queue_id.map(|queue_id| queue_id.to_string()),
                i64::from(limit),
            ],
            map_consumer,
        )
        .map_err(map_sql_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| consumer_invariant())
}

pub(super) fn map_consumer(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConsumerRow> {
    let consumer_id: String = row.get(0)?;
    let queue_id: String = row.get(1)?;
    let account_id: String = row.get(2)?;
    let version_id: String = row.get(4)?;
    let worker_id: String = row.get(5)?;
    let dlq: Option<String> = row.get(13)?;
    let dlq_generation: Option<i64> = row.get(14)?;
    Ok(ConsumerRow {
        consumer_id: consumer_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        queue_id: queue_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: account_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        consumer_generation: u64::try_from(row.get::<_, i64>(3)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_id: version_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: worker_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        execution_generation: u64::try_from(row.get::<_, i64>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        entrypoint: row.get(7)?,
        config: QueueConsumerConfig {
            max_batch_size: u32::try_from(row.get::<_, i64>(8)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_batch_timeout_seconds: u32::try_from(row.get::<_, i64>(9)? / 1000)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_retries: u32::try_from(row.get::<_, i64>(10)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            retry_delay_seconds: u32::try_from(row.get::<_, i64>(11)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_concurrency: u32::try_from(row.get::<_, i64>(12)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        },
        dead_letter_queue: dlq
            .zip(dlq_generation)
            .map(|(id, generation)| {
                Ok::<(QueueId, u64), rusqlite::Error>((
                    id.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                    u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
                ))
            })
            .transpose()?,
    })
}

pub(super) fn read_consumer_tx(
    tx: &Transaction<'_>,
    consumer_id: QueueConsumerId,
) -> Result<ConsumerRow, PlatformError> {
    tx.query_row(
        "SELECT c.consumer_id, c.queue_id, q.account_id, c.consumer_generation,
                c.version_id, c.worker_id,
                execution_generation, entrypoint, max_batch_size, max_batch_timeout_ms,
                max_retries, retry_delay_seconds, max_concurrency, dlq_queue_id,
                dlq_queue_generation FROM queue_consumer_state c
         JOIN queue_state q ON q.queue_id = c.queue_id WHERE consumer_id = ?1",
        [consumer_id.to_string()],
        map_consumer,
    )
    .map_err(map_sql_error)
}

pub(super) fn due_messages_tx(
    tx: &Transaction<'_>,
    consumer: &ConsumerRow,
    now_ms: i64,
) -> Result<Vec<MessageRow>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT seq, id, enqueued_at_ms, expires_at_ms, content_type, body, attempts
             FROM queue_messages m WHERE queue_id = ?1 AND state = 'ready'
               AND available_at_ms <= ?2 AND expires_at_ms > ?2
               AND NOT EXISTS (SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = m.id)
             ORDER BY available_at_ms, seq LIMIT ?3",
        )
        .map_err(map_sql_error)?;
    statement
        .query_map(
            params![
                consumer.queue_id.to_string(),
                now_ms,
                i64::from(consumer.config.max_batch_size)
            ],
            map_message,
        )
        .map_err(map_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| consumer_invariant())
}

pub(super) fn map_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    let id: String = row.get(1)?;
    let content_type: String = row.get(4)?;
    Ok(MessageRow {
        seq: row.get(0)?,
        id: id.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        enqueued_at_ms: row.get(2)?,
        expires_at_ms: row.get(3)?,
        content_type: content_type
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        body: row.get(5)?,
        attempts: u16::try_from(row.get::<_, i64>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

pub(super) fn claimed_messages_tx(
    tx: &Transaction<'_>,
    batch: &ClaimedQueueBatch,
) -> Result<Vec<MessageRow>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT seq, id, enqueued_at_ms, expires_at_ms, content_type, body, attempts
             FROM queue_messages WHERE claim_batch_id = ?1 AND consumer_id = ?2
               AND consumer_generation = ?3 AND claim_token = ?4 AND state = 'claimed'
             ORDER BY seq",
        )
        .map_err(map_sql_error)?;
    statement
        .query_map(
            params![
                batch.id.to_string(),
                batch.consumer_id.to_string(),
                as_i64(batch.consumer_generation)?,
                batch.claim_token.as_slice()
            ],
            map_message,
        )
        .map_err(map_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| consumer_invariant())
}

pub(super) fn ready_message_by_id_tx(
    tx: &Transaction<'_>,
    id: &str,
) -> Result<MessageRow, PlatformError> {
    tx.query_row(
        "SELECT seq, id, enqueued_at_ms, expires_at_ms, content_type, body, attempts
         FROM queue_messages WHERE id = ?1 AND state = 'ready'",
        [id],
        map_message,
    )
    .map_err(map_sql_error)
}

pub(super) fn recover_expired_batches_tx(
    tx: &Transaction<'_>,
    now_ms: i64,
    infrastructure_backoff_ms: u64,
    limit: u32,
) -> Result<u64, PlatformError> {
    let ids = {
        let mut statement = tx
            .prepare(
                "SELECT id FROM queue_delivery_batches WHERE claim_until_ms <= ?1
                 ORDER BY claim_until_ms, id LIMIT ?2",
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
    let available_at_ms = add_ms(now_ms, infrastructure_backoff_ms)?;
    for id in &ids {
        tx.execute(
            "UPDATE queue_messages SET state = 'ready', available_at_ms = ?1,
                    claim_token = NULL, claim_until_ms = NULL, claimed_at_ms = NULL,
                    claim_batch_id = NULL, consumer_id = NULL, consumer_generation = NULL
             WHERE claim_batch_id = ?2 AND state = 'claimed'",
            params![available_at_ms, id],
        )
        .map_err(consumer_sql_error)?;
        let deleted = tx
            .execute("DELETE FROM queue_delivery_batches WHERE id = ?1", [id])
            .map_err(consumer_sql_error)?;
        if deleted != 1 {
            return Err(consumer_invariant());
        }
    }
    u64::try_from(ids.len()).map_err(|_| consumer_invariant())
}

pub(super) fn retry_message_tx(
    tx: &Transaction<'_>,
    message: &MessageRow,
    batch: &ClaimedQueueBatch,
    attempts: u16,
    now_ms: i64,
    delay_seconds: u32,
) -> Result<(), PlatformError> {
    let available_at_ms = now_ms
        .checked_add(i64::from(delay_seconds) * 1000)
        .ok_or_else(consumer_invariant)?;
    let changed = tx
        .execute(
            "UPDATE queue_messages SET state = 'ready', attempts = ?1, available_at_ms = ?2,
                    claim_token = NULL, claim_until_ms = NULL, claimed_at_ms = NULL,
                    claim_batch_id = NULL, consumer_id = NULL, consumer_generation = NULL
             WHERE seq = ?3 AND claim_batch_id = ?4 AND consumer_id = ?5
               AND consumer_generation = ?6 AND claim_token = ?7 AND state = 'claimed'",
            params![
                i64::from(attempts),
                available_at_ms,
                message.seq,
                batch.id.to_string(),
                batch.consumer_id.to_string(),
                as_i64(batch.consumer_generation)?,
                batch.claim_token.as_slice(),
            ],
        )
        .map_err(consumer_sql_error)?;
    if changed != 1 {
        return Err(consumer_invariant());
    }
    Ok(())
}

pub(super) fn delete_claimed_message_tx(
    tx: &Transaction<'_>,
    message: &MessageRow,
    batch: &ClaimedQueueBatch,
) -> Result<(), PlatformError> {
    let changed = tx
        .execute(
            "DELETE FROM queue_messages WHERE seq = ?1 AND claim_batch_id = ?2
               AND consumer_id = ?3 AND consumer_generation = ?4
               AND claim_token = ?5 AND state = 'claimed'",
            params![
                message.seq,
                batch.id.to_string(),
                batch.consumer_id.to_string(),
                as_i64(batch.consumer_generation)?,
                batch.claim_token.as_slice(),
            ],
        )
        .map_err(consumer_sql_error)?;
    if changed != 1 {
        return Err(consumer_invariant());
    }
    Ok(())
}

pub(super) fn move_to_dlq_tx(
    tx: &Transaction<'_>,
    message: &MessageRow,
    batch: &ClaimedQueueBatch,
    target: (QueueId, u64),
    attempts: u16,
    now_ms: i64,
) -> Result<bool, PlatformError> {
    if !dlq_accepts_tx(tx, target, message.body.len())? {
        return Ok(false);
    }
    delete_claimed_message_tx(tx, message, batch)?;
    insert_dlq_message_tx(tx, message, target, attempts, now_ms)?;
    Ok(true)
}

pub(super) fn move_ready_to_dlq_tx(
    tx: &Transaction<'_>,
    message: &MessageRow,
    target: (QueueId, u64),
    now_ms: i64,
) -> Result<bool, PlatformError> {
    if !dlq_accepts_tx(tx, target, message.body.len())? {
        return Ok(false);
    }
    let changed = tx
        .execute(
            "DELETE FROM queue_messages WHERE seq = ?1 AND state = 'ready'",
            [message.seq],
        )
        .map_err(consumer_sql_error)?;
    if changed != 1 {
        return Err(consumer_invariant());
    }
    insert_dlq_message_tx(tx, message, target, message.attempts, now_ms)?;
    Ok(true)
}

pub(super) fn dlq_accepts_tx(
    tx: &Transaction<'_>,
    target: (QueueId, u64),
    body_len: usize,
) -> Result<bool, PlatformError> {
    let authority: Option<(String, i64, i64)> = tx
        .query_row(
            "SELECT state, message_bytes, max_backlog_bytes FROM queue_state
             WHERE queue_id = ?1 AND lifecycle_generation = ?2",
            params![target.0.to_string(), as_i64(target.1)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_sql_error)?;
    Ok(authority.is_some_and(|(state, bytes, maximum)| {
        state == "accepting"
            && bytes
                .checked_add(i64::try_from(body_len).unwrap_or(i64::MAX))
                .is_some_and(|value| value <= maximum)
    }))
}

pub(super) fn insert_dlq_message_tx(
    tx: &Transaction<'_>,
    message: &MessageRow,
    target: (QueueId, u64),
    _terminal_attempts: u16,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let retention_seconds: i64 = tx
        .query_row(
            "SELECT retention_seconds FROM queue_state
             WHERE queue_id = ?1 AND lifecycle_generation = ?2 AND state = 'accepting'",
            params![target.0.to_string(), as_i64(target.1)?],
            |row| row.get(0),
        )
        .map_err(map_sql_error)?;
    let expires_at_ms = now_ms
        .checked_add(
            retention_seconds
                .checked_mul(1000)
                .ok_or_else(consumer_invariant)?,
        )
        .ok_or_else(consumer_invariant)?;
    tx.execute(
        "INSERT INTO queue_messages
         (id, queue_id, queue_generation, enqueued_at_ms, available_at_ms, expires_at_ms,
          content_type, body, body_bytes, state, attempts, claim_token, claim_until_ms,
          claimed_at_ms, claim_batch_id, consumer_id, consumer_generation)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, 'ready', 0, NULL, NULL, NULL,
                 NULL, NULL, NULL)",
        params![
            message.id.to_string(),
            target.0.to_string(),
            as_i64(target.1)?,
            now_ms,
            expires_at_ms,
            message.content_type.as_str(),
            message.body,
            i64::try_from(message.body.len()).map_err(|_| consumer_invariant())?,
        ],
    )
    .map_err(consumer_sql_error)?;
    Ok(())
}

pub(super) fn random_claim_token() -> Result<[u8; 32], PlatformError> {
    let mut token = [0_u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut token)
        .map_err(|_| consumer_invariant())?;
    Ok(token)
}

pub(super) fn add_ms(now_ms: i64, delta_ms: u64) -> Result<i64, PlatformError> {
    now_ms
        .checked_add(i64::try_from(delta_ms).map_err(|_| consumer_invariant())?)
        .ok_or_else(consumer_invariant)
}

pub(super) fn as_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| consumer_invariant())
}

pub(super) fn inspect_unsigned(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(super) fn disposition_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueDispositionInvalid,
        "Queue disposition does not exactly match the claimed batch",
    )
}

#[allow(clippy::needless_pass_by_value)]
pub(super) fn consumer_sql_error(error: rusqlite::Error) -> PlatformError {
    let message = error.to_string();
    if message.contains("digest conflict") {
        PlatformError::new(
            ErrorCode::QueueConsumerProjectionPending,
            "Queue consumer projection digest conflicts with its generation",
        )
    } else if message.contains("database is locked") || message.contains("database is busy") {
        PlatformError::new(ErrorCode::SchedulerBusy, "scheduler database is busy")
    } else {
        consumer_invariant()
    }
}

pub(super) fn consumer_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue consumer scheduler invariant failed",
    )
}
