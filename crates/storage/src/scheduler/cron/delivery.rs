//! Durable Cron delivery claims, retries, and unknown-outcome recovery.

use super::*;
use rusqlite::{TransactionBehavior, params};

impl SchedulerStore {
    /// Claim a bounded due run set atomically.
    pub fn claim_cron_runs(
        &self,
        now_ms: i64,
        max_retries: u8,
        limit: u32,
    ) -> Result<Vec<ClaimedCronRun>, PlatformError> {
        if max_retries > 3 || limit == 0 {
            return Err(cron_invariant());
        }
        let first_deadline_ms = add_ms(now_ms, 15 * 60 * 1_000)?;
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql_error)?;
        tx.execute(
            "UPDATE cron_runs SET state = 'failed', next_attempt_at_ms = NULL,
                        error_code = CASE WHEN dispatch_deadline_at_ms <= ?1
                          THEN 'CRON_DISPATCH_DEADLINE' ELSE 'CRON_RETRY_EXHAUSTED' END,
                        completed_at_ms = ?1
                 WHERE state = 'ready' AND first_dispatched_at_ms IS NOT NULL
                   AND (dispatch_deadline_at_ms <= ?1 OR attempt >= ?2)",
            params![now_ms, i64::from(max_retries) + 1],
        )
        .map_err(map_sql_error)?;
        let ids = {
            let mut statement = tx
                .prepare(
                    "SELECT r.id FROM cron_runs r
                     WHERE r.state = 'ready' AND r.next_attempt_at_ms <= ?1
                       AND r.attempt < ?3
                       AND (r.dispatch_deadline_at_ms IS NULL OR r.dispatch_deadline_at_ms > ?1)
                     ORDER BY r.next_attempt_at_ms, r.scheduled_at_ms, r.id LIMIT ?2",
                )
                .map_err(map_sql_error)?;
            statement
                .query_map(
                    params![now_ms, i64::from(limit), i64::from(max_retries) + 1],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sql_error)?
        };
        let mut runs = Vec::with_capacity(ids.len());
        for id in ids {
            let deadline = tx
                .query_row(
                    "SELECT COALESCE(dispatch_deadline_at_ms, ?1)
                 FROM cron_runs WHERE id = ?2",
                    params![first_deadline_ms, id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_sql_error)?;
            let changed = tx
                .execute(
                    "UPDATE cron_runs SET state = 'claimed', next_attempt_at_ms = NULL,
                            attempt = attempt + 1, claimed_at_ms = ?1,
                            first_dispatched_at_ms = COALESCE(first_dispatched_at_ms, ?1),
                            dispatch_deadline_at_ms = COALESCE(dispatch_deadline_at_ms, ?2)
                     WHERE id = ?3 AND state = 'ready' AND next_attempt_at_ms <= ?1",
                    params![now_ms, deadline, id],
                )
                .map_err(map_sql_error)?;
            if changed != 1 {
                return Err(cron_invariant());
            }
            runs.push(read_claimed_run_tx(&tx, &id)?);
        }
        tx.commit().map_err(map_sql_error)?;
        Ok(runs)
    }

    /// Apply one known scheduled-handler result under the claimed run id.
    pub fn complete_cron_run(
        &self,
        run: &ClaimedCronRun,
        completion: CronCompletion,
        now_ms: i64,
        max_retries: u8,
    ) -> Result<CronCompletionResult, PlatformError> {
        if max_retries > 3 {
            return Err(cron_invariant());
        }
        let connection = self.lock()?;
        let exact: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cron_runs WHERE id = ?1
                   AND activation_id = ?2
                   AND state = 'claimed')",
                params![run.id.to_string(), run.activation_id.to_string(),],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if !exact {
            return Ok(CronCompletionResult::Stale);
        }
        let retry_at = match completion {
            CronCompletion::Failure {
                no_retry: false, ..
            } if run.attempt < max_retries + 1 => Some(cron_retry_at(run.id, run.attempt, now_ms)?)
                .filter(|retry_at| *retry_at < run.dispatch_deadline_at_ms),
            CronCompletion::Success | CronCompletion::Failure { .. } => None,
        };
        let (state, no_retry, error_code, result) = match completion {
            CronCompletion::Success => (
                CronRunState::Complete,
                false,
                None,
                CronCompletionResult::Terminal,
            ),
            CronCompletion::Failure { error_code, .. } if retry_at.is_some() => (
                CronRunState::Ready,
                false,
                Some(error_code),
                CronCompletionResult::Retried,
            ),
            CronCompletion::Failure {
                no_retry,
                error_code,
            } => (
                CronRunState::Failed,
                no_retry,
                Some(if !no_retry && run.attempt < max_retries + 1 {
                    "CRON_DISPATCH_DEADLINE"
                } else {
                    error_code
                }),
                CronCompletionResult::Terminal,
            ),
        };
        let completed_at_ms = (state != CronRunState::Ready).then_some(now_ms);
        let changed = connection
            .execute(
                "UPDATE cron_runs SET state = ?1, attempt = ?2, no_retry = ?3,
                        next_attempt_at_ms = ?4, claimed_at_ms = NULL, error_code = ?5, completed_at_ms = ?6
                 WHERE id = ?7 AND activation_id = ?8
                   AND state = 'claimed'",
                params![
                    state,
                    i64::from(run.attempt),
                    i64::from(no_retry),
                    retry_at,
                    error_code,
                    completed_at_ms,
                    run.id.to_string(),
                    run.activation_id.to_string(),
                ],
            )
            .map_err(map_sql_error)?;
        if changed != 1 {
            return Ok(CronCompletionResult::Stale);
        }
        drop(connection);
        self.wake.notify();
        Ok(result)
    }

    /// Record why an exact claimed delivery has no trustworthy result.
    pub fn mark_cron_unknown(
        &self,
        run: &ClaimedCronRun,
        reason: CronUnknownReason,
        now_ms: i64,
        max_retries: u8,
    ) -> Result<bool, PlatformError> {
        if max_retries > 3 {
            return Err(cron_invariant());
        }
        let next_attempt = add_ms(now_ms, 250)?;
        let changed = self
            .lock()?
            .execute(
                "UPDATE cron_runs SET last_unknown_reason = ?1,
               state = CASE WHEN attempt < ?6 AND dispatch_deadline_at_ms > ?5 THEN 'ready' ELSE 'failed' END,
               next_attempt_at_ms = CASE WHEN attempt < ?6 AND dispatch_deadline_at_ms > ?5 THEN ?5 ELSE NULL END,
               claimed_at_ms = NULL,
               error_code = CASE WHEN dispatch_deadline_at_ms <= ?5 THEN 'CRON_DISPATCH_DEADLINE'
                 WHEN attempt >= ?6 THEN 'CRON_RETRY_EXHAUSTED' ELSE error_code END,
               completed_at_ms = CASE WHEN attempt < ?6 AND dispatch_deadline_at_ms > ?5 THEN NULL ELSE ?4 END
             WHERE id = ?2 AND activation_id = ?3
               AND state = 'claimed'",
                params![
                    reason.as_str(),
                    run.id.to_string(),
                    run.activation_id.to_string(),
                    now_ms,
                    next_attempt,
                    i64::from(max_retries) + 1,
                ],
            )
            .map_err(map_sql_error)?;
        if changed > 0 {
            self.wake.notify();
        }
        Ok(changed == 1)
    }
}

fn read_claimed_run_tx(tx: &Transaction<'_>, id: &str) -> Result<ClaimedCronRun, PlatformError> {
    tx.query_row(
        "SELECT r.id, r.activation_id,
                (SELECT instance_id FROM scheduler_identity), r.worker_id,
                r.version_id, r.execution_generation, r.expression, r.scheduled_at_ms,
                r.attempt, r.dispatch_deadline_at_ms
                FROM cron_runs r WHERE r.id = ?1 AND r.state = 'claimed'",
        [id],
        |row| {
            let run: String = row.get(0)?;
            let activation: String = row.get(1)?;
            let instance: String = row.get(2)?;
            let worker: String = row.get(3)?;
            let version: String = row.get(4)?;
            Ok(ClaimedCronRun {
                id: run.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                activation_id: activation
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                instance_id: instance
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                worker_id: worker.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                version_id: version.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                execution_generation: u64::try_from(row.get::<_, i64>(5)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                expression: row.get(6)?,
                scheduled_at_ms: row.get(7)?,
                attempt: u8::try_from(row.get::<_, i64>(8)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                dispatch_deadline_at_ms: row.get(9)?,
            })
        },
    )
    .map_err(map_sql_error)
}

fn cron_retry_at(id: CronRunId, attempt: u8, now_ms: i64) -> Result<i64, PlatformError> {
    let exponent = u32::from(attempt.saturating_sub(1));
    let seconds = 2_u64.checked_shl(exponent).ok_or_else(cron_invariant)?;
    let uuid = id.as_uuid();
    let bytes = uuid.as_bytes();
    let jitter_ms = u64::from(u16::from_be_bytes([bytes[14], bytes[15]])) % 1000;
    add_ms(
        now_ms,
        seconds
            .checked_mul(1000)
            .and_then(|value| value.checked_add(jitter_ms))
            .ok_or_else(cron_invariant)?,
    )
}
