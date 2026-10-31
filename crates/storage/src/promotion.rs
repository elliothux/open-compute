//! Transaction-local Queue and Cron deployment replacement.

use open_compute_core::{CronActivationId, CronSchedule, PlatformError, VersionId, WorkerId};
use rusqlite::{Transaction, params};

/// Replace both product authorities and the scheduler rows they drive on the control transaction.
pub(crate) fn replace_products(
    tx: &Transaction<'_>,
    worker_id: WorkerId,
    version_id: Option<VersionId>,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let conflict: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM version_queue_consumers d JOIN queue_consumers c ON c.queue_id=d.queue_id
             WHERE d.version_id=?1 AND d.origin='version' AND c.worker_id!=?2)",
            params![version_id, worker_id], |row| row.get(0),
        ).map_err(sql_error)?;
    if conflict {
        return Err(PlatformError::new(
            open_compute_core::ErrorCode::QueueConsumerConflict,
            "Queue already has a live consumer owned by another Worker",
        ));
    }
    // Preserve attachment identity and pause state for Queues retained by the new version.
    tx.execute(
        "DELETE FROM queue_consumer_state WHERE worker_id=?1",
        [worker_id],
    )
    .map_err(sql_error)?;
    tx.execute(
            "DELETE FROM queue_consumers WHERE worker_id=?1 AND queue_id NOT IN (
                SELECT queue_id FROM version_queue_consumers WHERE version_id=?2 AND origin='version')",
            params![worker_id, version_id],
        ).map_err(sql_error)?;
    tx.execute(
            "INSERT INTO queue_consumers
             (id, queue_id, worker_id, declaration_id, version_id, consumer_generation,
              state, created_at_ms, updated_at_ms)
             SELECT id, queue_id, ?1, id, version_id, 1,
                    CASE WHEN (SELECT delivery_paused FROM queues WHERE id=queue_id) THEN 'paused' ELSE 'active' END,
                    ?3, ?3
             FROM version_queue_consumers WHERE version_id=?2 AND origin='version'
             ON CONFLICT(queue_id) DO UPDATE SET
                declaration_id=excluded.declaration_id, version_id=excluded.version_id,
                consumer_generation=queue_consumers.consumer_generation+1, updated_at_ms=?3
             WHERE queue_consumers.worker_id=?1",
            params![worker_id, version_id, now_ms],
        ).map_err(sql_error)?;
    super::queue_consumers::insert_scheduler_targets_tx(tx, worker_id).map_err(sql_error)?;
    tx.execute("DELETE FROM cron_schedules WHERE worker_id=?1", [worker_id])
        .map_err(sql_error)?;
    tx.execute(
        "DELETE FROM cron_activations WHERE worker_id=?1",
        [worker_id],
    )
    .map_err(sql_error)?;
    let declarations = {
        let mut statement = tx.prepare(
            "SELECT expression, expression_sha256, parser_version, scheduled_handler, workflow_bindings_json
             FROM version_cron_declarations WHERE version_id=?1 ORDER BY expression",
        ).map_err(sql_error)?;
        statement
            .query_map([version_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?
    };
    for (expression, digest, parser, handler, workflows) in declarations {
        let id = CronActivationId::generate();
        let next = CronSchedule::parse(&expression)?.next_after_ms(now_ms)?;
        tx.execute(
            "INSERT INTO cron_activations
             (id, worker_id, version_id, expression, expression_sha256, parser_version,
              scheduled_handler, workflow_bindings_json, created_at_ms, updated_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",
            params![
                id, worker_id, version_id, expression, digest, parser, handler, workflows, now_ms
            ],
        )
        .map_err(sql_error)?;
        tx.execute(
            "INSERT INTO cron_schedules
             (activation_id, worker_id, version_id, execution_generation, expression,
              expression_sha256, parser_version, next_fire_at_ms, updated_at_ms)
             SELECT ?1, id, ?2, route_generation, ?3, ?4, ?5, ?6, ?7 FROM workers WHERE id=?8",
            params![
                id, version_id, expression, digest, parser, next, now_ms, worker_id
            ],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

fn sql_error(error: rusqlite::Error) -> PlatformError {
    tracing::error!(%error, "Queue/Cron promotion transaction failed");
    PlatformError::new(
        open_compute_core::ErrorCode::VersionInvariantViolation,
        "Queue/Cron deployment replacement failed",
    )
}
