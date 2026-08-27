//! Read-only scheduler and cross-database operator inspection.

use super::*;

/// Secret-free aggregate Queue authority facts for doctor and support bundles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueInspectionSummary {
    /// Scheduler Queue projection count.
    pub queues: u64,
    /// Total retained messages.
    pub backlog_messages: u64,
    /// Total retained serialized body bytes.
    pub backlog_bytes: u64,
    /// Oldest retained enqueue timestamp.
    pub oldest_enqueued_at_ms: Option<i64>,
    /// Earliest retained expiry timestamp.
    pub oldest_expires_at_ms: Option<i64>,
    /// Already-expired messages awaiting bounded retention.
    pub ready_maintenance: u64,
    /// Counter rows that disagree with the message authority.
    pub counter_mismatches: u64,
}

/// Secret-free Queue consumer authority facts for doctor and support bundles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConsumerInspectionSummary {
    /// Scheduler consumer projection count.
    pub consumers: u64,
    /// Durable claimed batch count.
    pub claimed_batches: u64,
    /// Durable claimed message count.
    pub claimed_messages: u64,
    /// Terminal messages waiting for DLQ intake.
    pub dlq_pending: u64,
    /// Batches whose persisted membership does not match their claimed rows.
    pub orphan_batches: u64,
    /// Pending DLQ rows whose target projection is unavailable or stale.
    pub unavailable_dlq_targets: u64,
}

/// Secret-free Cron projection/run facts for doctor and support bundles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CronInspectionSummary {
    /// Scheduler Cron activation projections.
    pub schedules: u64,
    /// Retained logical run rows.
    pub runs: u64,
    /// Ready logical runs.
    pub ready_runs: u64,
    /// Claimed logical runs.
    pub claimed_runs: u64,
    /// Schedules persisted with an unsupported parser contract.
    pub parser_version_mismatches: u64,
    /// Schedules with a non-positive or non-minute-aligned next fire.
    pub invalid_next_fire: u64,
}

/// Read-only scheduler database facts used by `platformd doctor`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerInspection {
    /// Applied scheduler schema version.
    pub schema_version: i64,
    /// Persisted scheduler data-format marker.
    pub data_format: String,
    /// SQLite journal mode.
    pub journal_mode: String,
    /// SQLite synchronous level (`2` means FULL).
    pub synchronous: i64,
    /// Bounded state summary.
    pub summary: SchedulerSummary,
    /// Queue producer/retention authority summary; excludes bodies and tenant identities.
    pub queue: QueueInspectionSummary,
    /// Queue consumer projection and lease authority summary.
    pub queue_consumers: QueueConsumerInspectionSummary,
    /// Cron schedule and logical-run authority summary.
    pub cron: CronInspectionSummary,
    /// Invalid claim/token invariant rows found by a bounded aggregate query.
    pub invalid_rows: u64,
}

/// Secret-free control/scheduler reconciliation facts for P2.3 doctor checks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct P23CrossDatabaseInspection {
    /// Live Queue consumer authorities missing or conflicting on either side.
    pub queue_consumer_projection_mismatches: u64,
    /// Live Cron activation authorities missing or conflicting on either side.
    pub cron_projection_mismatches: u64,
    /// Live Queue/Cron targets missing their exact deployment referrer.
    pub deployment_referrer_mismatches: u64,
}

/// Compare P2.3 control and scheduler authority without mutating either database.
pub fn inspect_p23_cross_database(
    control_path: &std::path::Path,
    scheduler_path: &std::path::Path,
    busy_timeout_ms: u64,
) -> Result<P23CrossDatabaseInspection, PlatformError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let control_path = crate::control_db::leaf_nofollow_path(control_path)?;
    let scheduler_path = crate::control_db::leaf_nofollow_path(scheduler_path)?;
    let control = Connection::open_with_flags(control_path, flags).map_err(map_open_error)?;
    let scheduler = Connection::open_with_flags(scheduler_path, flags).map_err(map_open_error)?;
    for connection in [&control, &scheduler] {
        connection
            .busy_timeout(Duration::from_millis(busy_timeout_ms))
            .map_err(map_sql_error)?;
    }
    let control_consumers = inspect_authority_set(
        &control,
        "SELECT id, consumer_generation, deployment_id FROM queue_consumers
         WHERE state != 'tombstoned'",
    )?;
    let scheduler_consumers = inspect_authority_set(
        &scheduler,
        "SELECT consumer_id, consumer_generation, deployment_id FROM queue_consumer_state",
    )?;
    let control_crons = inspect_authority_set(
        &control,
        "SELECT id, activation_generation, deployment_id FROM cron_activations
         WHERE state != 'tombstoned'",
    )?;
    let scheduler_crons = inspect_authority_set(
        &scheduler,
        "SELECT activation_id, activation_generation, deployment_id FROM cron_schedules",
    )?;
    let deployment_referrer_mismatches: i64 = control
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM queue_consumers c
                WHERE c.state != 'tombstoned' AND NOT EXISTS (
                  SELECT 1 FROM deployment_referrers r
                  WHERE r.deployment_id = c.deployment_id
                    AND r.kind = 'queue_consumer' AND r.ref_id = c.id
                )) +
               (SELECT COUNT(*) FROM cron_activations c
                WHERE c.state != 'tombstoned' AND NOT EXISTS (
                  SELECT 1 FROM deployment_referrers r
                  WHERE r.deployment_id = c.deployment_id
                    AND r.kind = 'cron_activation' AND r.ref_id = c.id
                )) +
               (SELECT COUNT(*) FROM queue_consumers c
                WHERE c.pending_deployment_id IS NOT NULL AND NOT EXISTS (
                  SELECT 1 FROM deployment_referrers r
                  WHERE r.deployment_id = c.pending_deployment_id
                    AND r.kind = 'queue_consumer_pending' AND r.ref_id = c.id
                ))",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql_error)?;
    Ok(P23CrossDatabaseInspection {
        queue_consumer_projection_mismatches: symmetric_difference_len(
            &control_consumers,
            &scheduler_consumers,
        )?,
        cron_projection_mismatches: symmetric_difference_len(&control_crons, &scheduler_crons)?,
        deployment_referrer_mismatches: u64::try_from(deployment_referrer_mismatches)
            .map_err(|_| corrupt())?,
    })
}

fn inspect_authority_set(
    connection: &Connection,
    sql: &str,
) -> Result<HashSet<(String, i64, String)>, PlatformError> {
    const MAX_AUTHORITIES: usize = 100_000;
    let mut statement = connection.prepare(sql).map_err(map_sql_error)?;
    let mut rows = statement.query([]).map_err(map_sql_error)?;
    let mut values = HashSet::new();
    while let Some(row) = rows.next().map_err(map_sql_error)? {
        if values.len() >= MAX_AUTHORITIES {
            return Err(corrupt());
        }
        values.insert((
            row.get(0).map_err(map_sql_error)?,
            row.get(1).map_err(map_sql_error)?,
            row.get(2).map_err(map_sql_error)?,
        ));
    }
    Ok(values)
}

fn symmetric_difference_len<T: Eq + std::hash::Hash>(
    left: &HashSet<T>,
    right: &HashSet<T>,
) -> Result<u64, PlatformError> {
    u64::try_from(left.symmetric_difference(right).count()).map_err(|_| corrupt())
}

/// Inspect an existing scheduler database without migrating or mutating it.
pub fn inspect_scheduler_db(
    path: &std::path::Path,
    busy_timeout_ms: u64,
    now_ms: i64,
) -> Result<SchedulerInspection, PlatformError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let open_path = crate::control_db::leaf_nofollow_path(path)?;
    let connection = Connection::open_with_flags(open_path, flags).map_err(map_open_error)?;
    connection
        .busy_timeout(Duration::from_millis(busy_timeout_ms))
        .map_err(map_sql_error)?;
    let quick: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(map_sql_error)?;
    if quick != "ok" {
        return Err(corrupt());
    }
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sql_error)?;
    let data_format: String = connection
        .query_row(
            "SELECT data_format FROM scheduler_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql_error)?;
    if schema_version != SCHEMA_VERSION || data_format != DATA_FORMAT {
        return Err(corrupt());
    }
    verify_applied(&connection, schema_version)?;
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(map_sql_error)?;
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(map_sql_error)?;
    let summary = summary_connection(&connection, now_ms)?;
    let queue_workload = queue::helpers::queue_workload_summary_connection(&connection, now_ms)?;
    let (queue_count, backlog_messages, backlog_bytes, oldest_enqueued_at_ms, oldest_expires_at_ms): (
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
    ) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM queue_state),
                    COALESCE((SELECT SUM(message_count) FROM queue_state), 0),
                    COALESCE((SELECT SUM(message_bytes) FROM queue_state), 0),
                    (SELECT MIN(enqueued_at_ms) FROM queue_messages),
                    (SELECT MIN(expires_at_ms) FROM queue_messages)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(map_sql_error)?;
    let counter_mismatches = queue::helpers::counter_mismatches_connection(&connection)?.len();
    let queue = QueueInspectionSummary {
        queues: u64::try_from(queue_count).map_err(|_| corrupt())?,
        backlog_messages: u64::try_from(backlog_messages).map_err(|_| corrupt())?,
        backlog_bytes: u64::try_from(backlog_bytes).map_err(|_| corrupt())?,
        oldest_enqueued_at_ms,
        oldest_expires_at_ms,
        ready_maintenance: queue_workload.ready,
        counter_mismatches: u64::try_from(counter_mismatches).map_err(|_| corrupt())?,
    };
    let queue_consumers = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM queue_consumer_state),
               (SELECT COUNT(*) FROM queue_delivery_batches),
               (SELECT COUNT(*) FROM queue_messages WHERE state = 'claimed'),
               (SELECT COUNT(*) FROM queue_dlq_pending),
               (SELECT COUNT(*) FROM queue_delivery_batches b
                WHERE b.message_count != (
                  SELECT COUNT(*) FROM queue_messages m
                  WHERE m.claim_batch_id = b.id AND m.state = 'claimed'
                    AND m.consumer_id = b.consumer_id
                    AND m.consumer_generation = b.consumer_generation
                    AND m.claim_token = b.claim_token
                )),
               (SELECT COUNT(*) FROM queue_dlq_pending p
                LEFT JOIN queue_state q ON q.queue_id = p.target_queue_id
                WHERE q.queue_id IS NULL OR q.state != 'accepting'
                   OR q.lifecycle_generation != p.target_queue_generation)",
            [],
            |row| {
                Ok(QueueConsumerInspectionSummary {
                    consumers: unsigned_inspect(row.get(0)?)?,
                    claimed_batches: unsigned_inspect(row.get(1)?)?,
                    claimed_messages: unsigned_inspect(row.get(2)?)?,
                    dlq_pending: unsigned_inspect(row.get(3)?)?,
                    orphan_batches: unsigned_inspect(row.get(4)?)?,
                    unavailable_dlq_targets: unsigned_inspect(row.get(5)?)?,
                })
            },
        )
        .map_err(map_sql_error)?;
    let cron = connection
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM cron_schedules),
               (SELECT COUNT(*) FROM cron_runs),
               (SELECT COUNT(*) FROM cron_runs WHERE state = 'ready'),
               (SELECT COUNT(*) FROM cron_runs WHERE state = 'claimed'),
               (SELECT COUNT(*) FROM cron_schedules WHERE parser_version != 1),
               (SELECT COUNT(*) FROM cron_schedules
                WHERE next_fire_at_ms <= 0 OR next_fire_at_ms % 60000 != 0)",
            [],
            |row| {
                Ok(CronInspectionSummary {
                    schedules: unsigned_inspect(row.get(0)?)?,
                    runs: unsigned_inspect(row.get(1)?)?,
                    ready_runs: unsigned_inspect(row.get(2)?)?,
                    claimed_runs: unsigned_inspect(row.get(3)?)?,
                    parser_version_mismatches: unsigned_inspect(row.get(4)?)?,
                    invalid_next_fire: unsigned_inspect(row.get(5)?)?,
                })
            },
        )
        .map_err(map_sql_error)?;
    let alarm_invalid: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM scheduled_jobs WHERE
               (state = 'claimed') != (claim_token IS NOT NULL) OR
               (state = 'claimed') != (claim_until_ms IS NOT NULL) OR
               length(object_id) != 64 OR object_id != lower(object_id) OR
               retry_count < 0 OR retry_count > 6",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql_error)?;
    let invalid_rows = u64::try_from(alarm_invalid)
        .map_err(|_| corrupt())?
        .saturating_add(queue_consumers.orphan_batches)
        .saturating_add(queue_consumers.unavailable_dlq_targets)
        .saturating_add(cron.parser_version_mismatches)
        .saturating_add(cron.invalid_next_fire);
    Ok(SchedulerInspection {
        schema_version,
        data_format,
        journal_mode,
        synchronous,
        summary,
        queue,
        queue_consumers,
        cron,
        invalid_rows,
    })
}

fn unsigned_inspect(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn inspect_scheduler_schema_version(
    path: &std::path::Path,
    busy_timeout_ms: u64,
) -> Result<i64, PlatformError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let open_path = crate::control_db::leaf_nofollow_path(path)?;
    let connection = Connection::open_with_flags(open_path, flags).map_err(map_open_error)?;
    connection
        .busy_timeout(Duration::from_millis(busy_timeout_ms))
        .map_err(map_sql_error)?;
    let quick: String = connection
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(map_sql_error)?;
    if quick != "ok" {
        return Err(corrupt());
    }
    let schema_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sql_error)?;
    if schema_version > SCHEMA_VERSION {
        return Err(PlatformError::new(
            ErrorCode::SchemaTooNew,
            "scheduler database schema is newer than this binary",
        ));
    }
    let marker: (i64, String) = connection
        .query_row(
            "SELECT schema_version, data_format
             FROM scheduler_meta WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sql_error)?;
    if marker != (schema_version, DATA_FORMAT.to_owned()) {
        return Err(corrupt());
    }
    verify_applied(&connection, schema_version)?;
    Ok(schema_version)
}
