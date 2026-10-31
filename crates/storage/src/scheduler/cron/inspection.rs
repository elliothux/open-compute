//! Exact Cron dispatch identity and operator inspection.

use super::*;

type CronRuntimeRow = (
    i64,
    i64,
    Option<i64>,
    Option<CronRunState>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

/// Secret-free per-activation runtime facts for authenticated operator inspection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CronRuntimeInspection {
    /// Whether the exact activation generation projection exists.
    pub projection_exists: bool,
    /// Scheduler projection state, if present.
    pub schedule_state: Option<CronScheduleState>,
    /// Next logical slot computed by the persisted parser contract.
    pub next_fire_at_ms: Option<i64>,
    /// Ready logical runs for this generation.
    pub ready_runs: u64,
    /// Claimed logical runs for this generation.
    pub claimed_runs: u64,
    /// Most recently completed terminal state, if retained.
    pub last_outcome: Option<CronRunState>,
    /// Stable terminal error classification for the last retained run.
    pub last_error_code: Option<String>,
    /// Stable transport classification for the last retained unknown result.
    pub last_unknown_reason: Option<String>,
    /// Fixed completion deadline for the oldest live delivery.
    pub oldest_dispatch_deadline_at_ms: Option<i64>,
    /// Oldest due logical-run lag in milliseconds.
    pub lag_ms: u64,
}

impl SchedulerStore {
    /// Runtime dispatch epoch pinned when this schedule was installed.
    pub fn cron_execution_generation(
        &self,
        id: CronActivationId,
    ) -> Result<Option<u64>, PlatformError> {
        self.lock()?
            .query_row(
                "SELECT execution_generation FROM cron_schedules WHERE activation_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql_error)
    }

    /// Inspect one exact Cron activation without exposing expressions as metric labels.
    pub fn inspect_cron_runtime(
        &self,
        activation_id: CronActivationId,
        now_ms: i64,
    ) -> Result<CronRuntimeInspection, PlatformError> {
        let connection = self.lock()?;
        let schedule: Option<i64> = connection
            .query_row(
                "SELECT next_fire_at_ms FROM cron_schedules
                 WHERE activation_id = ?1",
                params![activation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql_error)?;
        let (ready, claimed, oldest_due, last_outcome, last_error_code, last_unknown_reason, oldest_deadline): CronRuntimeRow =
            connection
                .query_row(
                    "SELECT
                   COUNT(*) FILTER (WHERE state = 'ready'),
                   COUNT(*) FILTER (WHERE state = 'claimed'),
                   MIN(next_attempt_at_ms) FILTER (WHERE state = 'ready'),
                   (SELECT state FROM cron_runs WHERE activation_id = ?1

                      AND state IN ('complete', 'failed', 'skipped')
                    ORDER BY completed_at_ms DESC, id DESC LIMIT 1),
                   (SELECT error_code FROM cron_runs WHERE activation_id = ?1

                      AND state IN ('complete', 'failed', 'skipped')
                    ORDER BY completed_at_ms DESC, id DESC LIMIT 1),
                   (SELECT last_unknown_reason FROM cron_runs WHERE activation_id = ?1
                      AND last_unknown_reason IS NOT NULL
                    ORDER BY COALESCE(completed_at_ms, claimed_at_ms, created_at_ms) DESC, id DESC LIMIT 1),
                   MIN(dispatch_deadline_at_ms) FILTER (WHERE state IN ('ready', 'claimed'))
                 FROM cron_runs WHERE activation_id = ?1",
                    params![activation_id],
                    |row| {
                        Ok((
                            row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                            row.get(4)?, row.get(5)?, row.get(6)?,
                        ))
                    },
                )
                .map_err(map_sql_error)?;
        Ok(CronRuntimeInspection {
            projection_exists: schedule.is_some(),
            schedule_state: schedule.map(|_| CronScheduleState::Active),
            next_fire_at_ms: schedule,
            ready_runs: u64::try_from(ready).map_err(|_| cron_invariant())?,
            claimed_runs: u64::try_from(claimed).map_err(|_| cron_invariant())?,
            last_outcome,
            last_error_code,
            last_unknown_reason,
            oldest_dispatch_deadline_at_ms: oldest_deadline,
            lag_ms: oldest_due.map_or(0, |due| now_ms.saturating_sub(due).max(0) as u64),
        })
    }
}
