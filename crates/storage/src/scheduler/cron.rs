//! Durable Cron schedules, logical slots, claims, retries, and history.

use super::{SchedulerStore, map_sql_error};
use open_compute_core::{
    CronActivationId, CronRunId, CronSchedule, ErrorCode, InstanceId, PlatformError, VersionId,
    WorkerId, WorkloadSummary,
};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};

/// Schedule input for storage tests.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "test-support"))]
pub struct NewCronSchedule {
    /// Live activation identity.
    pub activation_id: CronActivationId,
    /// Owning instance.
    pub instance_id: InstanceId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen version target.
    pub version_id: VersionId,
    /// Frozen execution generation.
    pub execution_generation: u64,
    /// Exact tenant-visible expression.
    pub expression: String,
    /// Parser-normalized expression digest.
    pub expression_sha256: [u8; 32],
    /// Frozen parser contract version.
    pub parser_version: u32,
    /// First logical UTC slot.
    pub next_fire_at_ms: i64,
    /// Projection mutation time.
    pub updated_at_ms: i64,
}

/// One exact durable Cron run claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedCronRun {
    /// Durable run identity.
    pub id: CronRunId,
    /// Activation identity.
    pub activation_id: CronActivationId,
    /// Owning instance.
    pub instance_id: InstanceId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen version target.
    pub version_id: VersionId,
    /// Frozen execution generation.
    pub execution_generation: u64,
    /// Exact declared expression.
    pub expression: String,
    /// Logical UTC scheduled time.
    pub scheduled_at_ms: i64,
    /// Logical deliveries already started, including this claim.
    pub attempt: u8,
    /// Fixed Cloudflare Scheduled handler completion deadline.
    pub dispatch_deadline_at_ms: i64,
}

/// Stable classification for a Cron delivery whose result is unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CronUnknownReason {
    /// Dispatch exceeded the transport timeout.
    TransportTimeout,
    /// The runtime connection ended without a result.
    ConnectionLoss,
    /// The runtime returned an invalid result.
    MalformedResponse,
    /// The supervised runtime generation changed during dispatch.
    RuntimeGenerationLost,
}

impl CronUnknownReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TransportTimeout => "transport-timeout",
            Self::ConnectionLoss => "connection-loss",
            Self::MalformedResponse => "malformed-response",
            Self::RuntimeGenerationLost => "runtime-generation-lost",
        }
    }
}

/// Known native scheduled-handler result applied to the claimed run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CronCompletion {
    /// Handler and waitUntil work completed successfully.
    Success,
    /// Known tenant failure, optionally fenced by `controller.noRetry()`.
    Failure {
        /// Native no-retry decision.
        no_retry: bool,
        /// Stable low-cardinality error code.
        error_code: &'static str,
    },
}

/// Result of an exact Cron completion attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CronCompletionResult {
    /// Run became terminal.
    Terminal,
    /// Run returned to ready with a product retry.
    Retried,
    /// Run is no longer claimed.
    Stale,
}

/// Bounded logical slot projection counts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CronSlotSummary {
    /// Grace-window slots inserted as durable runs.
    pub projected: u64,
    /// Due schedules whose historical slots were outside the grace window.
    pub skipped: u64,
}

#[path = "cron/inspection.rs"]
mod inspection;
pub use inspection::CronRuntimeInspection;
#[path = "cron/delivery.rs"]
mod delivery;

impl SchedulerStore {
    /// Seed a schedule for product tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_cron_schedule(&self, projection: &NewCronSchedule) -> Result<(), PlatformError> {
        self.require_instance(projection.instance_id)?;
        let connection = self.lock()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO cron_schedules
                 (activation_id, worker_id, version_id, execution_generation,
                  expression, expression_sha256, parser_version,
                  next_fire_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    projection.activation_id.to_string(),
                    projection.worker_id.to_string(),
                    projection.version_id.to_string(),
                    as_i64(projection.execution_generation)?,
                    projection.expression,
                    projection.expression_sha256.as_slice(),
                    i64::from(projection.parser_version),
                    projection.next_fire_at_ms,
                    projection.updated_at_ms,
                ],
            )
            .map_err(map_sql_error)?;
        Ok(())
    }

    /// Advance due schedules and insert at most the newest grace-window slot per schedule.
    pub fn project_due_cron_slots(
        &self,
        now_ms: i64,
        misfire_grace_ms: u64,
        limit: u32,
    ) -> Result<CronSlotSummary, PlatformError> {
        if now_ms < 0 || limit == 0 {
            return Err(cron_invariant());
        }
        let grace_floor = now_ms
            .saturating_sub(i64::try_from(misfire_grace_ms).map_err(|_| cron_invariant())?)
            .max(0);
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql_error)?;
        let schedules = due_schedules_tx(&tx, now_ms, limit)?;
        let mut summary = CronSlotSummary::default();
        for schedule in schedules {
            let parsed = CronSchedule::parse(&schedule.expression)?;
            let slot =
                parsed.latest_at_or_before_ms(grace_floor.max(schedule.next_fire_at_ms), now_ms)?;
            if let Some(scheduled_at_ms) = slot {
                let inserted = tx
                    .execute(
                        "INSERT OR IGNORE INTO cron_runs
                         (id, activation_id, worker_id, scheduled_at_ms,
                          version_id, execution_generation, expression, state, attempt,
                          no_retry, next_attempt_at_ms, error_code, created_at_ms, completed_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ready', 0, 0, ?4,
                                 NULL, ?8, NULL)",
                        params![
                            CronRunId::generate().to_string(),
                            schedule.activation_id.to_string(),
                            schedule.worker_id,
                            scheduled_at_ms,
                            schedule.version_id.to_string(),
                            as_i64(schedule.execution_generation)?,
                            schedule.expression,
                            now_ms,
                        ],
                    )
                    .map_err(map_sql_error)?;
                summary.projected += u64::try_from(inserted).map_err(|_| cron_invariant())?;
            } else {
                summary.skipped += 1;
            }
            let next = parsed.next_after_ms(now_ms)?;
            let changed = tx
                .execute(
                    "UPDATE cron_schedules SET next_fire_at_ms = ?1, updated_at_ms = ?2
                     WHERE activation_id = ?3 AND next_fire_at_ms <= ?2",
                    params![next, now_ms, schedule.activation_id.to_string(),],
                )
                .map_err(map_sql_error)?;
            if changed != 1 {
                return Err(cron_invariant());
            }
        }
        tx.commit().map_err(map_sql_error)?;
        if summary.projected > 0 || summary.skipped > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(summary)
    }

    /// Boundedly retain terminal history by age and per-activation row cap.
    pub fn gc_cron_history(
        &self,
        now_ms: i64,
        retention_ms: u64,
        per_activation_limit: u32,
    ) -> Result<u64, PlatformError> {
        if retention_ms == 0 || per_activation_limit == 0 {
            return Err(cron_invariant());
        }
        let cutoff =
            now_ms.saturating_sub(i64::try_from(retention_ms).map_err(|_| cron_invariant())?);
        let connection = self.lock()?;
        let deleted = connection
            .execute(
                "DELETE FROM cron_runs WHERE state IN ('complete', 'failed', 'skipped') AND (
                   completed_at_ms < ?1 OR id IN (
                     SELECT id FROM (
                       SELECT id, ROW_NUMBER() OVER (
                         PARTITION BY activation_id ORDER BY completed_at_ms DESC, id DESC
                       ) AS ordinal
                       FROM cron_runs WHERE state IN ('complete', 'failed', 'skipped')
                     ) WHERE ordinal > ?2
                   )
                 )",
                params![cutoff, i64::from(per_activation_limit)],
            )
            .map_err(map_sql_error)?;
        u64::try_from(deleted).map_err(|_| cron_invariant())
    }

    /// Cron pool workload facts for admission and wake coordination.
    pub fn cron_workload_summary(&self, now_ms: i64) -> Result<WorkloadSummary, PlatformError> {
        let connection = self.lock()?;
        let (ready, claimed, expired, oldest, next):
            (i64, i64, i64, Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM cron_runs WHERE state = 'ready' AND next_attempt_at_ms <= ?1)
                     + (SELECT COUNT(*) FROM cron_schedules
                        WHERE next_fire_at_ms <= ?1),
                   (SELECT COUNT(*) FROM cron_runs WHERE state = 'claimed'),
                   0,
                   (SELECT MIN(value) FROM (
                      SELECT MIN(next_attempt_at_ms) AS value FROM cron_runs
                        WHERE state = 'ready' AND next_attempt_at_ms <= ?1
                      UNION ALL SELECT MIN(next_fire_at_ms) FROM cron_schedules
                        WHERE next_fire_at_ms <= ?1
                    )),
                   (SELECT MIN(value) FROM (
                      SELECT MIN(next_fire_at_ms) AS value FROM cron_schedules

                      UNION ALL SELECT MIN(next_attempt_at_ms) FROM cron_runs WHERE state = 'ready'

                    ))",
                [now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(map_sql_error)?;
        Ok(WorkloadSummary {
            ready: u64::try_from(ready).map_err(|_| cron_invariant())?,
            claimed: u64::try_from(claimed).map_err(|_| cron_invariant())?,
            expired: u64::try_from(expired).map_err(|_| cron_invariant())?,
            oldest_due_at_ms: oldest,
            next_due_at_ms: next,
        })
    }
}

#[derive(Debug)]
struct DueSchedule {
    activation_id: CronActivationId,
    worker_id: WorkerId,
    version_id: VersionId,
    execution_generation: u64,
    expression: String,
    next_fire_at_ms: i64,
}

fn due_schedules_tx(
    tx: &Transaction<'_>,
    now_ms: i64,
    limit: u32,
) -> Result<Vec<DueSchedule>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT activation_id, worker_id, version_id,
                    execution_generation, expression, next_fire_at_ms FROM cron_schedules
             WHERE next_fire_at_ms <= ?1
             ORDER BY next_fire_at_ms, activation_id LIMIT ?2",
        )
        .map_err(map_sql_error)?;
    statement
        .query_map(params![now_ms, i64::from(limit)], |row| {
            let activation: String = row.get(0)?;
            let version: String = row.get(2)?;
            Ok(DueSchedule {
                activation_id: activation
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                worker_id: row.get(1)?,
                version_id: version.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                execution_generation: u64::try_from(row.get::<_, i64>(3)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                expression: row.get(4)?,
                next_fire_at_ms: row.get(5)?,
            })
        })
        .map_err(map_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sql_error)
}

fn add_ms(now_ms: i64, delta_ms: u64) -> Result<i64, PlatformError> {
    now_ms
        .checked_add(i64::try_from(delta_ms).map_err(|_| cron_invariant())?)
        .ok_or_else(cron_invariant)
}

fn as_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| cron_invariant())
}

fn cron_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "Cron scheduler invariant failed",
    )
}

/// A present schedule accepts new logical slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum CronScheduleState {
    #[serde(rename = "accepting")]
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CronRunState {
    Ready,
    Claimed,
    Complete,
    Failed,
    Skipped,
}
impl rusqlite::types::ToSql for CronRunState {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(match self {
            Self::Ready => "ready",
            Self::Claimed => "claimed",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
        .into())
    }
}
impl rusqlite::types::FromSql for CronRunState {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "ready" => Ok(Self::Ready),
            "claimed" => Ok(Self::Claimed),
            "complete" => Ok(Self::Complete),
            "failed" => Ok(Self::Failed),
            "skipped" => Ok(Self::Skipped),
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}
