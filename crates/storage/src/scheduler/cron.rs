//! Durable Cron schedule projection, logical slots, leases, retries, and history.

use super::{SchedulerStore, map_sql_error};
use open_compute_core::{
    AccountId, CronActivationId, CronRunId, CronSchedule, DeploymentId, ErrorCode, PlatformError,
    WorkerId, WorkloadSummary,
};
use rand::TryRngCore as _;
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};

/// Exact control-authoritative Cron activation copied into the scheduler database.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronScheduleProjection {
    /// Live activation identity.
    pub activation_id: CronActivationId,
    /// Owning account.
    pub account_id: AccountId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen deployment target.
    pub deployment_id: DeploymentId,
    /// Frozen execution generation.
    pub execution_generation: u64,
    /// Monotonic activation set generation.
    pub activation_generation: u64,
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
    /// Activation generation fence.
    pub activation_generation: u64,
    /// Owning account.
    pub account_id: AccountId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Frozen deployment target.
    pub deployment_id: DeploymentId,
    /// Frozen execution generation.
    pub execution_generation: u64,
    /// Exact declared expression.
    pub expression: String,
    /// Logical UTC scheduled time.
    pub scheduled_at_ms: i64,
    /// Product retries already consumed.
    pub attempt: u8,
    /// Secret scheduler-only completion fence.
    pub claim_token: [u8; 32],
    /// Persisted lease expiry.
    pub claim_until_ms: i64,
}

/// Known native scheduled-handler result applied under the exact lease.
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
    /// Token or activation generation was already stale.
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
    /// Idempotently stage or verify one exact Cron schedule projection.
    pub fn ensure_cron_schedule_projection(
        &self,
        projection: &CronScheduleProjection,
    ) -> Result<(), PlatformError> {
        validate_projection(projection)?;
        let connection = self.lock()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO cron_schedules
                 (activation_id, account_id, worker_id, deployment_id, execution_generation,
                  activation_generation, expression, expression_sha256, parser_version, state,
                  next_fire_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'staged', ?10, ?11)",
                params![
                    projection.activation_id.to_string(),
                    projection.account_id.to_string(),
                    projection.worker_id.to_string(),
                    projection.deployment_id.to_string(),
                    as_i64(projection.execution_generation)?,
                    as_i64(projection.activation_generation)?,
                    projection.expression,
                    projection.expression_sha256.as_slice(),
                    i64::from(projection.parser_version),
                    projection.next_fire_at_ms,
                    projection.updated_at_ms,
                ],
            )
            .map_err(cron_sql_error)?;
        let exact: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cron_schedules WHERE activation_id = ?1
                   AND account_id = ?2 AND worker_id = ?3 AND deployment_id = ?4
                   AND execution_generation = ?5 AND activation_generation = ?6
                   AND expression = ?7 AND expression_sha256 = ?8 AND parser_version = ?9)",
                params![
                    projection.activation_id.to_string(),
                    projection.account_id.to_string(),
                    projection.worker_id.to_string(),
                    projection.deployment_id.to_string(),
                    as_i64(projection.execution_generation)?,
                    as_i64(projection.activation_generation)?,
                    projection.expression,
                    projection.expression_sha256.as_slice(),
                    i64::from(projection.parser_version),
                ],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if !exact {
            return Err(PlatformError::new(
                ErrorCode::CronProjectionPending,
                "Cron projection conflicts with frozen activation authority",
            ));
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Enable logical slot projection for one exact activation generation.
    pub fn activate_cron_schedule(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.set_cron_schedule_state(
            activation_id,
            activation_generation,
            "accepting",
            &["staged", "accepting"],
            now_ms,
        )
    }

    /// Stop creating new slots while already-created runs drain.
    pub fn drain_cron_schedule(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        self.set_cron_schedule_state(
            activation_id,
            activation_generation,
            "draining",
            &["staged", "accepting", "draining"],
            now_ms,
        )?;
        self.cron_activation_in_flight(activation_id, activation_generation)
    }

    /// Delete one drained schedule after all nonterminal runs are gone.
    pub fn delete_cron_schedule_projection(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
    ) -> Result<(), PlatformError> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(cron_sql_error)?;
        let nonterminal: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cron_runs WHERE activation_id = ?1
                   AND activation_generation = ?2 AND state IN ('ready', 'claimed'))",
                params![activation_id.to_string(), as_i64(activation_generation)?],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        if nonterminal {
            return Err(PlatformError::new(
                ErrorCode::CronProjectionPending,
                "Cron activation still has nonterminal runs",
            ));
        }
        tx.execute(
            "DELETE FROM cron_runs WHERE activation_id = ?1 AND activation_generation = ?2",
            params![activation_id.to_string(), as_i64(activation_generation)?],
        )
        .map_err(cron_sql_error)?;
        tx.execute(
            "DELETE FROM cron_schedules WHERE activation_id = ?1
               AND activation_generation = ?2 AND state IN ('staged', 'draining', 'deleting')",
            params![activation_id.to_string(), as_i64(activation_generation)?],
        )
        .map_err(cron_sql_error)?;
        tx.commit().map_err(cron_sql_error)?;
        drop(connection);
        self.wake.notify();
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
            .map_err(cron_sql_error)?;
        let schedules = due_schedules_tx(&tx, now_ms, limit)?;
        let mut summary = CronSlotSummary::default();
        for schedule in schedules {
            let parsed = CronSchedule::parse(&schedule.expression)?;
            let slot = parsed.latest_at_or_before_ms(grace_floor, now_ms)?;
            if let Some(scheduled_at_ms) = slot {
                let inserted = tx
                    .execute(
                        "INSERT OR IGNORE INTO cron_runs
                         (id, activation_id, activation_generation, scheduled_at_ms,
                          deployment_id, execution_generation, expression, state, attempt,
                          no_retry, next_attempt_at_ms, claim_token, claimed_at_ms,
                          claim_until_ms, error_code, created_at_ms, completed_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ready', 0, 0, ?4,
                                 NULL, NULL, NULL, NULL, ?8, NULL)",
                        params![
                            CronRunId::generate().to_string(),
                            schedule.activation_id.to_string(),
                            as_i64(schedule.activation_generation)?,
                            scheduled_at_ms,
                            schedule.deployment_id.to_string(),
                            as_i64(schedule.execution_generation)?,
                            schedule.expression,
                            now_ms,
                        ],
                    )
                    .map_err(cron_sql_error)?;
                summary.projected += u64::try_from(inserted).map_err(|_| cron_invariant())?;
            } else {
                summary.skipped += 1;
            }
            let next = parsed.next_after_ms(now_ms)?;
            let changed = tx
                .execute(
                    "UPDATE cron_schedules SET next_fire_at_ms = ?1, updated_at_ms = ?2
                     WHERE activation_id = ?3 AND activation_generation = ?4
                       AND state = 'accepting' AND next_fire_at_ms <= ?2",
                    params![
                        next,
                        now_ms,
                        schedule.activation_id.to_string(),
                        as_i64(schedule.activation_generation)?,
                    ],
                )
                .map_err(cron_sql_error)?;
            if changed != 1 {
                return Err(cron_invariant());
            }
        }
        tx.commit().map_err(cron_sql_error)?;
        if summary.projected > 0 || summary.skipped > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(summary)
    }

    /// Recover expired unknown outcomes and claim a bounded due run set atomically.
    pub fn claim_cron_runs(
        &self,
        now_ms: i64,
        lease_ms: u64,
        infrastructure_backoff_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClaimedCronRun>, PlatformError> {
        if lease_ms == 0 || limit == 0 {
            return Err(cron_invariant());
        }
        let claim_until_ms = add_ms(now_ms, lease_ms)?;
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(cron_sql_error)?;
        recover_expired_cron_runs_tx(&tx, now_ms, infrastructure_backoff_ms, limit)?;
        let ids = {
            let mut statement = tx
                .prepare(
                    "SELECT r.id FROM cron_runs r JOIN cron_schedules s
                       ON s.activation_id = r.activation_id
                     WHERE r.state = 'ready' AND r.next_attempt_at_ms <= ?1
                       AND s.activation_generation = r.activation_generation
                       AND s.state IN ('accepting', 'draining')
                     ORDER BY r.next_attempt_at_ms, r.scheduled_at_ms, r.id LIMIT ?2",
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
        let mut runs = Vec::with_capacity(ids.len());
        for id in ids {
            let token = random_claim_token()?;
            let changed = tx
                .execute(
                    "UPDATE cron_runs SET state = 'claimed', next_attempt_at_ms = NULL,
                            claim_token = ?1, claimed_at_ms = ?2, claim_until_ms = ?3
                     WHERE id = ?4 AND state = 'ready' AND next_attempt_at_ms <= ?2",
                    params![token.as_slice(), now_ms, claim_until_ms, id],
                )
                .map_err(cron_sql_error)?;
            if changed != 1 {
                return Err(cron_invariant());
            }
            runs.push(read_claimed_run_tx(&tx, &id, token, claim_until_ms)?);
        }
        tx.commit().map_err(cron_sql_error)?;
        Ok(runs)
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
        let (state, attempt, no_retry, error_code, result) = match completion {
            CronCompletion::Success => (
                "complete",
                run.attempt,
                false,
                None,
                CronCompletionResult::Terminal,
            ),
            CronCompletion::Failure {
                no_retry,
                error_code,
            } if !no_retry && run.attempt < max_retries => (
                "ready",
                run.attempt + 1,
                false,
                Some(error_code),
                CronCompletionResult::Retried,
            ),
            CronCompletion::Failure {
                no_retry,
                error_code,
            } => (
                "failed",
                run.attempt,
                no_retry,
                Some(error_code),
                CronCompletionResult::Terminal,
            ),
        };
        let next_attempt_at_ms = (state == "ready")
            .then(|| cron_retry_at(run.id, attempt, now_ms))
            .transpose()?;
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
                    i64::from(attempt),
                    i64::from(no_retry),
                    next_attempt_at_ms,
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

    /// Recover a bounded set of expired unknown Cron outcomes without product retry cost.
    pub fn recover_expired_cron_runs(
        &self,
        now_ms: i64,
        infrastructure_backoff_ms: u64,
        limit: u32,
    ) -> Result<u64, PlatformError> {
        if limit == 0 {
            return Err(cron_invariant());
        }
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(cron_sql_error)?;
        let recovered =
            recover_expired_cron_runs_tx(&tx, now_ms, infrastructure_backoff_ms, limit)?;
        tx.commit().map_err(cron_sql_error)?;
        if recovered > 0 {
            drop(connection);
            self.wake.notify();
        }
        Ok(recovered)
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
            .map_err(cron_sql_error)?;
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
                        WHERE state = 'accepting' AND next_fire_at_ms <= ?1),
                   (SELECT COUNT(*) FROM cron_runs WHERE state = 'claimed'),
                   (SELECT COUNT(*) FROM cron_runs WHERE state = 'claimed' AND claim_until_ms <= ?1),
                   (SELECT MIN(value) FROM (
                      SELECT MIN(next_attempt_at_ms) AS value FROM cron_runs
                        WHERE state = 'ready' AND next_attempt_at_ms <= ?1
                      UNION ALL SELECT MIN(next_fire_at_ms) FROM cron_schedules
                        WHERE state = 'accepting' AND next_fire_at_ms <= ?1
                    )),
                   (SELECT MIN(value) FROM (
                      SELECT MIN(next_fire_at_ms) AS value FROM cron_schedules
                        WHERE state = 'accepting'
                      UNION ALL SELECT MIN(next_attempt_at_ms) FROM cron_runs WHERE state = 'ready'
                      UNION ALL SELECT MIN(claim_until_ms) FROM cron_runs WHERE state = 'claimed'
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

    /// Count nonterminal runs for one activation generation.
    pub fn cron_activation_in_flight(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
    ) -> Result<u64, PlatformError> {
        let connection = self.lock()?;
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM cron_runs WHERE activation_id = ?1
                   AND activation_generation = ?2 AND state IN ('ready', 'claimed')",
                params![activation_id.to_string(), as_i64(activation_generation)?],
                |row| row.get(0),
            )
            .map_err(map_sql_error)?;
        u64::try_from(count).map_err(|_| cron_invariant())
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

    fn set_cron_schedule_state(
        &self,
        activation_id: CronActivationId,
        activation_generation: u64,
        target: &str,
        sources: &[&str],
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let placeholders = (0..sources.len())
            .map(|index| format!("?{}", index + 5))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE cron_schedules SET state = ?1, updated_at_ms = ?2
             WHERE activation_id = ?3 AND activation_generation = ?4
               AND state IN ({placeholders})"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![
            target.to_owned().into(),
            now_ms.into(),
            activation_id.to_string().into(),
            as_i64(activation_generation)?.into(),
        ];
        values.extend(sources.iter().map(|source| (*source).to_owned().into()));
        let connection = self.lock()?;
        let changed = connection
            .execute(&sql, rusqlite::params_from_iter(values))
            .map_err(cron_sql_error)?;
        if changed != 1 {
            return Err(PlatformError::new(
                ErrorCode::CronActivationStale,
                "Cron activation generation or state is stale",
            ));
        }
        drop(connection);
        self.wake.notify();
        Ok(())
    }
}

#[derive(Debug)]
struct DueSchedule {
    activation_id: CronActivationId,
    activation_generation: u64,
    deployment_id: DeploymentId,
    execution_generation: u64,
    expression: String,
}

fn due_schedules_tx(
    tx: &Transaction<'_>,
    now_ms: i64,
    limit: u32,
) -> Result<Vec<DueSchedule>, PlatformError> {
    let mut statement = tx
        .prepare(
            "SELECT activation_id, activation_generation, deployment_id,
                    execution_generation, expression FROM cron_schedules
             WHERE state = 'accepting' AND next_fire_at_ms <= ?1
             ORDER BY next_fire_at_ms, activation_id LIMIT ?2",
        )
        .map_err(map_sql_error)?;
    statement
        .query_map(params![now_ms, i64::from(limit)], |row| {
            let activation: String = row.get(0)?;
            let deployment: String = row.get(2)?;
            Ok(DueSchedule {
                activation_id: activation
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                activation_generation: u64::try_from(row.get::<_, i64>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                deployment_id: deployment
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                execution_generation: u64::try_from(row.get::<_, i64>(3)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                expression: row.get(4)?,
            })
        })
        .map_err(map_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sql_error)
}

fn read_claimed_run_tx(
    tx: &Transaction<'_>,
    id: &str,
    claim_token: [u8; 32],
    claim_until_ms: i64,
) -> Result<ClaimedCronRun, PlatformError> {
    tx.query_row(
        "SELECT r.id, r.activation_id, r.activation_generation, s.account_id, s.worker_id,
                r.deployment_id, r.execution_generation, r.expression, r.scheduled_at_ms,
                r.attempt FROM cron_runs r JOIN cron_schedules s
                  ON s.activation_id = r.activation_id WHERE r.id = ?1 AND r.state = 'claimed'",
        [id],
        |row| {
            let run: String = row.get(0)?;
            let activation: String = row.get(1)?;
            let account: String = row.get(3)?;
            let worker: String = row.get(4)?;
            let deployment: String = row.get(5)?;
            Ok(ClaimedCronRun {
                id: run.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                activation_id: activation
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                activation_generation: u64::try_from(row.get::<_, i64>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                account_id: account.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                worker_id: worker.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                deployment_id: deployment
                    .parse()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                execution_generation: u64::try_from(row.get::<_, i64>(6)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                expression: row.get(7)?,
                scheduled_at_ms: row.get(8)?,
                attempt: u8::try_from(row.get::<_, i64>(9)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                claim_token,
                claim_until_ms,
            })
        },
    )
    .map_err(map_sql_error)
}

fn recover_expired_cron_runs_tx(
    tx: &Transaction<'_>,
    now_ms: i64,
    infrastructure_backoff_ms: u64,
    limit: u32,
) -> Result<u64, PlatformError> {
    let next_attempt = add_ms(now_ms, infrastructure_backoff_ms)?;
    let changed = tx
        .execute(
            "UPDATE cron_runs SET state = 'ready', next_attempt_at_ms = ?1,
                    claim_token = NULL, claimed_at_ms = NULL, claim_until_ms = NULL
             WHERE id IN (
               SELECT id FROM cron_runs WHERE state = 'claimed' AND claim_until_ms <= ?2
               ORDER BY claim_until_ms, id LIMIT ?3
             )",
            params![next_attempt, now_ms, i64::from(limit)],
        )
        .map_err(cron_sql_error)?;
    u64::try_from(changed).map_err(|_| cron_invariant())
}

fn validate_projection(projection: &CronScheduleProjection) -> Result<(), PlatformError> {
    if projection.execution_generation == 0
        || projection.activation_generation == 0
        || projection.parser_version == 0
        || projection.next_fire_at_ms < 0
    {
        return Err(cron_invariant());
    }
    let parsed = CronSchedule::parse(&projection.expression)?;
    if parsed.normalized().is_empty() {
        return Err(cron_invariant());
    }
    Ok(())
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

fn add_ms(now_ms: i64, delta_ms: u64) -> Result<i64, PlatformError> {
    now_ms
        .checked_add(i64::try_from(delta_ms).map_err(|_| cron_invariant())?)
        .ok_or_else(cron_invariant)
}

fn as_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| cron_invariant())
}

#[allow(clippy::needless_pass_by_value)]
fn cron_sql_error(error: rusqlite::Error) -> PlatformError {
    let message = error.to_string();
    if message.contains("digest conflict") {
        PlatformError::new(
            ErrorCode::CronProjectionPending,
            "Cron projection digest conflicts with its activation generation",
        )
    } else if message.contains("database is locked") || message.contains("database is busy") {
        PlatformError::new(ErrorCode::SchedulerBusy, "scheduler database is busy")
    } else {
        cron_invariant()
    }
}

fn cron_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "Cron scheduler invariant failed",
    )
}
