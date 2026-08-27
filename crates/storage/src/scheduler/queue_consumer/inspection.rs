//! Secret-free Queue consumer scheduler summaries and operator inspection.

use super::*;

impl SchedulerStore {
    /// Queue consumer workload facts for pool admission and wake coordination.
    pub fn queue_consumer_workload_summary(
        &self,
        now_ms: i64,
    ) -> Result<WorkloadSummary, PlatformError> {
        let connection = self.lock()?;
        let (ready, claimed, expired, oldest, next):
            (i64, i64, i64, Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM queue_consumer_state c
                    WHERE c.state = 'accepting'
                      AND (SELECT COUNT(*) FROM queue_delivery_batches b
                           WHERE b.consumer_id = c.consumer_id
                             AND b.consumer_generation = c.consumer_generation) < c.max_concurrency
                      AND EXISTS (
                        SELECT 1 FROM queue_messages m WHERE m.queue_id = c.queue_id
                          AND m.state = 'ready' AND m.available_at_ms <= ?1
                          AND m.expires_at_ms > ?1
                          AND NOT EXISTS (
                            SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = m.id
                          )
                        GROUP BY m.queue_id
                        HAVING COUNT(*) >= c.max_batch_size
                           OR ?1 >= MIN(m.available_at_ms) + c.max_batch_timeout_ms
                      )),
                   (SELECT COUNT(*) FROM queue_delivery_batches),
                   (SELECT COUNT(*) FROM queue_delivery_batches WHERE claim_until_ms <= ?1),
                   (SELECT MIN(m.available_at_ms) FROM queue_messages m
                    JOIN queue_consumer_state c ON c.queue_id = m.queue_id
                    WHERE c.state = 'accepting' AND m.state = 'ready'
                      AND m.available_at_ms <= ?1 AND m.expires_at_ms > ?1
                      AND NOT EXISTS (
                        SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = m.id
                      )),
                   MIN(value)
                 FROM (
                   SELECT MIN(CASE WHEN m.available_at_ms > ?1 THEN m.available_at_ms
                                          ELSE m.available_at_ms + c.max_batch_timeout_ms END) AS value
                     FROM queue_messages m
                     JOIN queue_consumer_state c ON c.queue_id = m.queue_id
                     WHERE c.state = 'accepting' AND m.state = 'ready'
                       AND m.expires_at_ms > ?1
                       AND NOT EXISTS (
                         SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = m.id
                       )
                   UNION ALL SELECT MIN(claim_until_ms) FROM queue_delivery_batches
                   UNION ALL SELECT MIN(next_attempt_at_ms) FROM queue_dlq_pending
                 )",
                [now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(map_sql_error)?;
        Ok(WorkloadSummary {
            ready: u64::try_from(ready).map_err(|_| consumer_invariant())?,
            claimed: u64::try_from(claimed).map_err(|_| consumer_invariant())?,
            expired: u64::try_from(expired).map_err(|_| consumer_invariant())?,
            oldest_due_at_ms: oldest,
            next_due_at_ms: next,
        })
    }

    /// Count terminal messages whose bounded DLQ forwarding retry is due.
    pub fn queue_dlq_pending_due(&self, now_ms: i64) -> Result<u64, PlatformError> {
        let connection = self.lock()?;
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM queue_dlq_pending WHERE next_attempt_at_ms <= ?1",
                [now_ms],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        u64::try_from(count).map_err(|_| consumer_invariant())
    }

    /// Count all terminal messages waiting for DLQ intake capacity.
    pub fn queue_dlq_pending_count(&self) -> Result<u64, PlatformError> {
        let connection = self.lock()?;
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM queue_dlq_pending", [], |row| {
                row.get(0)
            })
            .map_err(map_sql_error)?;
        u64::try_from(count).map_err(|_| consumer_invariant())
    }

    /// Inspect one exact consumer generation without returning message bodies or tokens.
    pub fn inspect_queue_consumer_runtime(
        &self,
        queue_id: QueueId,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
    ) -> Result<QueueConsumerRuntimeInspection, PlatformError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT
                   EXISTS(SELECT 1 FROM queue_consumer_state
                          WHERE consumer_id = ?1 AND queue_id = ?2
                            AND consumer_generation = ?3),
                   (SELECT COUNT(*) FROM queue_messages WHERE queue_id = ?2),
                   COALESCE((SELECT SUM(body_bytes) FROM queue_messages WHERE queue_id = ?2), 0),
                   (SELECT COUNT(*) FROM queue_messages m WHERE m.queue_id = ?2
                      AND m.state = 'ready' AND NOT EXISTS (
                        SELECT 1 FROM queue_dlq_pending p WHERE p.message_id = m.id
                      )),
                   (SELECT COUNT(*) FROM queue_delivery_batches
                      WHERE consumer_id = ?1 AND consumer_generation = ?3),
                   (SELECT COUNT(*) FROM queue_messages
                      WHERE consumer_id = ?1 AND consumer_generation = ?3
                        AND state = 'claimed'),
                   (SELECT COUNT(*) FROM queue_dlq_pending WHERE source_queue_id = ?2)",
                params![
                    consumer_id.to_string(),
                    queue_id.to_string(),
                    as_i64(consumer_generation)?,
                ],
                |row| {
                    Ok(QueueConsumerRuntimeInspection {
                        projection_exists: row.get(0)?,
                        backlog_messages: inspect_unsigned(row.get(1)?)?,
                        backlog_bytes: inspect_unsigned(row.get(2)?)?,
                        ready_messages: inspect_unsigned(row.get(3)?)?,
                        claimed_batches: inspect_unsigned(row.get(4)?)?,
                        claimed_messages: inspect_unsigned(row.get(5)?)?,
                        dlq_pending: inspect_unsigned(row.get(6)?)?,
                    })
                },
            )
            .map_err(map_sql_error)
    }

    /// Count durable in-flight batches for an exact consumer generation.
    pub fn queue_consumer_in_flight(
        &self,
        consumer_id: QueueConsumerId,
        consumer_generation: u64,
    ) -> Result<u64, PlatformError> {
        let connection = self.lock()?;
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM queue_delivery_batches
                 WHERE consumer_id = ?1 AND consumer_generation = ?2",
                params![consumer_id.to_string(), as_i64(consumer_generation)?],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        u64::try_from(count).map_err(|_| consumer_invariant())
    }
}
