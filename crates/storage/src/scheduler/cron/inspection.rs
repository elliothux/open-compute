//! Exact Cron dispatch identity and operator inspection.

use super::*;

/// Secret-free per-activation runtime facts for authenticated operator inspection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CronRuntimeInspection {
    /// Whether the exact activation generation projection exists.
    pub projection_exists: bool,
    /// Scheduler projection state, if present.
    pub schedule_state: Option<String>,
    /// Next logical slot computed by the persisted parser contract.
    pub next_fire_at_ms: Option<i64>,
    /// Ready logical runs for this generation.
    pub ready_runs: u64,
    /// Claimed logical runs for this generation.
    pub claimed_runs: u64,
    /// Most recently completed terminal state, if retained.
    pub last_outcome: Option<String>,
    /// Oldest due logical-run lag in milliseconds.
    pub lag_ms: u64,
}

impl SchedulerStore {
    /// Read the immutable dispatch epoch of an exact Cron activation.
    /// HTTP route revisions do not replace an already persisted activation epoch.
    pub fn cron_execution_generation(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
    ) -> Result<Option<u64>, PlatformError> {
        let value: Option<i64> = self.lock()?.query_row(
            "SELECT execution_generation FROM cron_schedules WHERE activation_id=?1 AND activation_generation=?2",
            params![activation_id.to_string(), as_i64(activation_generation)?],
            |row| row.get(0),
        ).optional().map_err(map_sql_error)?;
        value
            .map(|value| {
                u64::try_from(value)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(cron_invariant)
            })
            .transpose()
    }

    /// Inspect one exact Cron activation without exposing expressions as metric labels.
    pub fn inspect_cron_runtime(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
        now_ms: i64,
    ) -> Result<CronRuntimeInspection, PlatformError> {
        let connection = self.lock()?;
        let schedule: Option<(String, i64)> = connection
            .query_row(
                "SELECT state, next_fire_at_ms FROM cron_schedules
                 WHERE activation_id = ?1 AND activation_generation = ?2",
                params![activation_id.to_string(), as_i64(activation_generation)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(map_sql_error)?;
        let (ready, claimed, oldest_due, last_outcome): (i64, i64, Option<i64>, Option<String>) =
            connection
                .query_row(
                    "SELECT
                   COUNT(*) FILTER (WHERE state = 'ready'),
                   COUNT(*) FILTER (WHERE state = 'claimed'),
                   MIN(next_attempt_at_ms) FILTER (WHERE state = 'ready'),
                   (SELECT state FROM cron_runs WHERE activation_id = ?1
                      AND activation_generation = ?2
                      AND state IN ('complete', 'failed', 'skipped')
                    ORDER BY completed_at_ms DESC, id DESC LIMIT 1)
                 FROM cron_runs WHERE activation_id = ?1 AND activation_generation = ?2",
                    params![activation_id.to_string(), as_i64(activation_generation)?],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(map_sql_error)?;
        Ok(CronRuntimeInspection {
            projection_exists: schedule.is_some(),
            schedule_state: schedule.as_ref().map(|value| value.0.clone()),
            next_fire_at_ms: schedule.map(|value| value.1),
            ready_runs: u64::try_from(ready).map_err(|_| cron_invariant())?,
            claimed_runs: u64::try_from(claimed).map_err(|_| cron_invariant())?,
            last_outcome,
            lag_ms: oldest_due.map_or(0, |due| now_ms.saturating_sub(due).max(0) as u64),
        })
    }
}
