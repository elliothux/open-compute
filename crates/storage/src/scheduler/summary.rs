//! Bounded scheduler summary queries.

use super::{SchedulerSummary, corrupt, map_sql_error};
use open_compute_core::{PlatformError, WorkloadSummary};
use rusqlite::Connection;

pub(super) fn summary_connection(
    connection: &Connection,
    now_ms: i64,
) -> Result<SchedulerSummary, PlatformError> {
    let mut summary = SchedulerSummary::default();
    let mut statement = connection
        .prepare("SELECT state, COUNT(*) FROM scheduled_jobs GROUP BY state")
        .map_err(map_sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(map_sql_error)?;
    for row in rows {
        let (state, count) = row.map_err(map_sql_error)?;
        let count = u64::try_from(count).map_err(|_| corrupt())?;
        match state.as_str() {
            "scheduled" => summary.scheduled = count,
            "claimed" => summary.claimed = count,
            "discarding" => summary.discarding = count,
            _ => return Err(corrupt()),
        }
    }
    summary.oldest_due_at_ms = connection
        .query_row(
            "SELECT MIN(due_at_ms) FROM scheduled_jobs WHERE state = 'scheduled'",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql_error)?;
    let expired: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM scheduled_jobs
             WHERE state = 'claimed' AND claim_until_ms <= ?1",
            [now_ms],
            |row| row.get(0),
        )
        .map_err(map_sql_error)?;
    summary.expired_claims = u64::try_from(expired).map_err(|_| corrupt())?;
    Ok(summary)
}

pub(super) fn workload_summary_connection(
    connection: &Connection,
    now_ms: i64,
) -> Result<WorkloadSummary, PlatformError> {
    let (ready, claimed, expired): (i64, i64, i64) = connection
        .query_row(
            "SELECT
               SUM(CASE WHEN state = 'scheduled' AND due_at_ms <= ?1 THEN 1 ELSE 0 END),
               SUM(CASE WHEN state = 'claimed' THEN 1 ELSE 0 END),
               SUM(CASE WHEN state = 'claimed' AND claim_until_ms <= ?1 THEN 1 ELSE 0 END)
             FROM scheduled_jobs",
            [now_ms],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            },
        )
        .map_err(map_sql_error)?;
    let (oldest_due_at_ms, claim_until_ms): (Option<i64>, Option<i64>) = connection
        .query_row(
            "SELECT
               MIN(CASE WHEN state = 'scheduled' THEN due_at_ms END),
               MIN(CASE WHEN state = 'claimed' THEN claim_until_ms END)
             FROM scheduled_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(map_sql_error)?;
    Ok(WorkloadSummary {
        ready: u64::try_from(ready).map_err(|_| corrupt())?,
        claimed: u64::try_from(claimed).map_err(|_| corrupt())?,
        expired: u64::try_from(expired).map_err(|_| corrupt())?,
        oldest_due_at_ms,
        next_due_at_ms: match (oldest_due_at_ms, claim_until_ms) {
            (Some(due), Some(claim)) => Some(due.min(claim)),
            (Some(due), None) => Some(due),
            (None, Some(claim)) => Some(claim),
            (None, None) => None,
        },
    })
}
