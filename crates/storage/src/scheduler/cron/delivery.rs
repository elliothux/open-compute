//! Durable Cron delivery claims, retries, and unknown-outcome recovery.

use super::*;
use rand::TryRngCore as _;
use rusqlite::{TransactionBehavior, params};

impl SchedulerStore {
    /// Recover expired unknown outcomes and claim a bounded due run set atomically.
    pub fn claim_cron_runs(
        &self,
        now_ms: i64,
        lease_ms: u64,
        infrastructure_backoff_ms: u64,
        max_retries: u8,
        limit: u32,
    ) -> Result<(Vec<ClaimedCronRun>, u64), PlatformError> {
        if lease_ms == 0 || max_retries > 3 || limit == 0 {
            return Err(cron_invariant());
        }
        let proposed_claim_until_ms = add_ms(now_ms, lease_ms)?;
        let first_deadline_ms = add_ms(now_ms, 15 * 60 * 1_000)?;
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(cron_sql_error)?;
        let recovered = recover_expired_cron_runs_tx(
            &tx,
            now_ms,
            infrastructure_backoff_ms,
            max_retries,
            limit,
        )?;
        let expired_ready = tx
            .execute(
                "UPDATE cron_runs SET state = 'failed', next_attempt_at_ms = NULL,
                        error_code = CASE WHEN dispatch_deadline_at_ms <= ?1
                          THEN 'CRON_DISPATCH_DEADLINE' ELSE 'CRON_RETRY_EXHAUSTED' END,
                        completed_at_ms = ?1
                 WHERE state = 'ready' AND first_dispatched_at_ms IS NOT NULL
                   AND (dispatch_deadline_at_ms <= ?1 OR attempt >= ?2)",
                params![now_ms, i64::from(max_retries) + 1],
            )
            .map_err(cron_sql_error)?;
        let ids = {
            let mut statement = tx
                .prepare(
                    "SELECT r.id FROM cron_runs r JOIN cron_schedules s
                       ON s.activation_id = r.activation_id
                     WHERE r.state = 'ready' AND r.next_attempt_at_ms <= ?1
                       AND s.activation_generation = r.activation_generation
                       AND s.state = 'accepting' AND r.attempt < ?3
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
            let token = random_claim_token()?;
            let deadline = tx
                .query_row(
                    "SELECT COALESCE(dispatch_deadline_at_ms, ?1)
                 FROM cron_runs WHERE id = ?2",
                    params![first_deadline_ms, id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_sql_error)?;
            let claim_until_ms = proposed_claim_until_ms.min(deadline);
            if claim_until_ms <= now_ms {
                return Err(cron_invariant());
            }
            let changed = tx
                .execute(
                    "UPDATE cron_runs SET state = 'claimed', next_attempt_at_ms = NULL,
                            attempt = attempt + 1, claim_token = ?1, claimed_at_ms = ?2,
                            claim_until_ms = ?3,
                            first_dispatched_at_ms = COALESCE(first_dispatched_at_ms, ?2),
                            dispatch_deadline_at_ms = COALESCE(dispatch_deadline_at_ms, ?4)
                     WHERE id = ?5 AND state = 'ready' AND next_attempt_at_ms <= ?2",
                    params![token.as_slice(), now_ms, claim_until_ms, deadline, id],
                )
                .map_err(cron_sql_error)?;
            if changed != 1 {
                return Err(cron_invariant());
            }
            runs.push(read_claimed_run_tx(&tx, &id, token)?);
        }
        tx.commit().map_err(cron_sql_error)?;
        Ok((
            runs,
            recovered
                .checked_add(u64::try_from(expired_ready).map_err(|_| cron_invariant())?)
                .ok_or_else(cron_invariant)?,
        ))
    }

    /// Apply one known scheduled-handler result under the exact token and generation.
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
                   AND activation_id = ?2 AND activation_generation = ?3
                   AND state = 'claimed' AND claim_token = ?4)",
                params![
                    run.id.to_string(),
                    run.activation_id.to_string(),
                    as_i64(run.activation_generation)?,
                    run.claim_token.as_slice(),
                ],
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
            CronCompletion::Success => ("complete", false, None, CronCompletionResult::Terminal),
            CronCompletion::Failure { error_code, .. } if retry_at.is_some() => (
                "ready",
                false,
                Some(error_code),
                CronCompletionResult::Retried,
            ),
            CronCompletion::Failure {
                no_retry,
                error_code,
            } => (
                "failed",
                no_retry,
                Some(if !no_retry && run.attempt < max_retries + 1 {
                    "CRON_DISPATCH_DEADLINE"
                } else {
                    error_code
                }),
                CronCompletionResult::Terminal,
            ),
        };
        let completed_at_ms = (state != "ready").then_some(now_ms);
        let changed = connection
            .execute(
                "UPDATE cron_runs SET state = ?1, attempt = ?2, no_retry = ?3,
                        next_attempt_at_ms = ?4, claim_token = NULL, claimed_at_ms = NULL,
                        claim_until_ms = NULL, error_code = ?5, completed_at_ms = ?6
                 WHERE id = ?7 AND activation_id = ?8 AND activation_generation = ?9
                   AND state = 'claimed' AND claim_token = ?10",
                params![
                    state,
                    i64::from(run.attempt),
                    i64::from(no_retry),
                    retry_at,
                    error_code,
                    completed_at_ms,
                    run.id.to_string(),
                    run.activation_id.to_string(),
                    as_i64(run.activation_generation)?,
                    run.claim_token.as_slice(),
                ],
            )
            .map_err(cron_sql_error)?;
        if changed != 1 {
            return Ok(CronCompletionResult::Stale);
        }
        drop(connection);
        self.wake.notify();
        Ok(result)
    }

    /// Recover a bounded set of expired unknown Cron outcomes.
    pub fn recover_expired_cron_runs(
        &self,
        now_ms: i64,
        infrastructure_backoff_ms: u64,
        max_retries: u8,
        limit: u32,
    ) -> Result<u64, PlatformError> {
        if max_retries > 3 || limit == 0 {
            return Err(cron_invariant());
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(cron_sql_error)?;
        let recovered = recover_expired_cron_runs_tx(
            &tx,
            now_ms,
            infrastructure_backoff_ms,
            max_retries,
            limit,
        )?;
        tx.commit().map_err(cron_sql_error)?;
        if recovered > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(recovered)
    }

    /// Record why an exact claimed delivery has no trustworthy result.
    pub fn mark_cron_unknown(
        &self,
        run: &ClaimedCronRun,
        reason: CronUnknownReason,
    ) -> Result<bool, PlatformError> {
        let changed = self
            .lock()?
            .execute(
                "UPDATE cron_runs SET last_unknown_reason = ?1
             WHERE id = ?2 AND activation_id = ?3 AND activation_generation = ?4
               AND state = 'claimed' AND claim_token = ?5",
                params![
                    reason.as_str(),
                    run.id.to_string(),
                    run.activation_id.to_string(),
                    as_i64(run.activation_generation)?,
                    run.claim_token.as_slice(),
                ],
            )
            .map_err(cron_sql_error)?;
        Ok(changed == 1)
    }
}

fn read_claimed_run_tx(
    tx: &Transaction<'_>,
    id: &str,
    claim_token: [u8; 32],
) -> Result<ClaimedCronRun, PlatformError> {
    tx.query_row(
        "SELECT r.id, r.activation_id, r.activation_generation,
                (SELECT instance_id FROM scheduler_identity), s.worker_id,
                r.version_id, r.execution_generation, r.expression, r.scheduled_at_ms,
                r.attempt, r.claim_until_ms, r.dispatch_deadline_at_ms
                FROM cron_runs r JOIN cron_schedules s
                  ON s.activation_id = r.activation_id WHERE r.id = ?1 AND r.state = 'claimed'",
        [id],
        |row| {
            let run: String = row.get(0)?;
            let activation: String = row.get(1)?;
            let instance: String = row.get(3)?;
            let worker: String = row.get(4)?;
            let version: String = row.get(5)?;
            Ok(ClaimedCronRun {
                id: run.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                activation_id: activation
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                activation_generation: u64::try_from(row.get::<_, i64>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                instance_id: instance
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                worker_id: worker.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                version_id: version.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                execution_generation: u64::try_from(row.get::<_, i64>(6)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                expression: row.get(7)?,
                scheduled_at_ms: row.get(8)?,
                attempt: u8::try_from(row.get::<_, i64>(9)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                claim_token,
                claim_until_ms: row.get(10)?,
                dispatch_deadline_at_ms: row.get(11)?,
            })
        },
    )
    .map_err(map_sql_error)
}

fn recover_expired_cron_runs_tx(
    tx: &Transaction<'_>,
    now_ms: i64,
    infrastructure_backoff_ms: u64,
    max_retries: u8,
    limit: u32,
) -> Result<u64, PlatformError> {
    let next_attempt = add_ms(now_ms, infrastructure_backoff_ms)?;
    let changed = tx
        .execute(
            "UPDATE cron_runs SET
                    state = CASE WHEN attempt < ?4 AND dispatch_deadline_at_ms > ?1 AND EXISTS (
                      SELECT 1 FROM cron_schedules s WHERE s.activation_id = cron_runs.activation_id
                        AND s.activation_generation = cron_runs.activation_generation
                        AND s.state = 'accepting'
                    ) THEN 'ready' ELSE 'failed' END,
                    next_attempt_at_ms = CASE WHEN attempt < ?4 AND dispatch_deadline_at_ms > ?1
                      AND EXISTS (SELECT 1 FROM cron_schedules s
                        WHERE s.activation_id = cron_runs.activation_id
                          AND s.activation_generation = cron_runs.activation_generation
                          AND s.state = 'accepting') THEN ?1 ELSE NULL END,
                    claim_token = NULL, claimed_at_ms = NULL, claim_until_ms = NULL,
                    last_unknown_reason = COALESCE(last_unknown_reason, 'transport-timeout'),
                    error_code = CASE
                      WHEN NOT EXISTS (SELECT 1 FROM cron_schedules s
                        WHERE s.activation_id = cron_runs.activation_id
                          AND s.activation_generation = cron_runs.activation_generation
                          AND s.state = 'accepting') THEN 'CRON_ACTIVATION_DRAINED'
                      WHEN dispatch_deadline_at_ms <= ?1 THEN 'CRON_DISPATCH_DEADLINE'
                      WHEN attempt >= ?4 THEN 'CRON_RETRY_EXHAUSTED'
                      ELSE error_code END,
                    completed_at_ms = CASE WHEN attempt < ?4 AND dispatch_deadline_at_ms > ?1
                      AND EXISTS (SELECT 1 FROM cron_schedules s
                        WHERE s.activation_id = cron_runs.activation_id
                          AND s.activation_generation = cron_runs.activation_generation
                          AND s.state = 'accepting') THEN NULL ELSE ?2 END
             WHERE id IN (
               SELECT id FROM cron_runs WHERE state = 'claimed' AND claim_until_ms <= ?2
               ORDER BY claim_until_ms, id LIMIT ?3
             )",
            params![
                next_attempt,
                now_ms,
                i64::from(limit),
                i64::from(max_retries) + 1
            ],
        )
        .map_err(cron_sql_error)?;
    u64::try_from(changed).map_err(|_| cron_invariant())
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

fn random_claim_token() -> Result<[u8; 32], PlatformError> {
    let mut token = [0_u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut token)
        .map_err(|_| cron_invariant())?;
    Ok(token)
}
