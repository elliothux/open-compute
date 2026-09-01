//! Capability activation release after drain or expired-run recovery.

use super::*;

impl SchedulerStore {
    /// Back off an unclaimable queued identity so unavailable definitions cannot starve other work.
    pub fn defer_workflow(
        &self,
        id: WorkflowInstanceId,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        self.lock()?
            .execute(
                "UPDATE workflow_instances SET next_run_at_ms=?3,updated_at_ms=?2
             WHERE id=?1 AND state='queued' AND next_run_at_ms<=?2",
                params![
                    id.to_string(),
                    now_ms,
                    deadline(now_ms, limits.recovery_backoff_ms)?
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Page one ready account in round-robin order, checking fresh work after at most three recoveries.
    /// The cursor is process-local; all eligibility remains in the durable ready index.
    pub fn due_workflows(
        &self,
        now_ms: i64,
        limit: u32,
        cursor: &mut WorkflowClaimCursor,
    ) -> Result<Vec<WorkflowInstanceId>, PlatformError> {
        bounded(limit)?;
        let conn = self.lock()?;
        let preferred = cursor.recovered_streak < 3;
        for recovered in [preferred, !preferred] {
            for after in [
                cursor.account.map_or_else(String::new, |id| id.to_string()),
                String::new(),
            ] {
                let account: Option<String> = conn.query_row(
                    "SELECT account_id FROM workflow_instances WHERE state='queued' AND has_activated=?1
                     AND account_id>?2 AND next_run_at_ms<=?3 ORDER BY account_id LIMIT 1",
                    params![recovered, after, now_ms],
                    |row| row.get(0),
                ).optional().map_err(sql_error)?;
                let Some(account) = account else {
                    continue;
                };
                let mut statement = conn.prepare(
                    "SELECT id FROM workflow_instances
                     WHERE state='queued' AND has_activated=?1 AND account_id=?2 AND next_run_at_ms<=?3
                     ORDER BY next_run_at_ms,created_at_ms,id LIMIT ?4",
                ).map_err(sql_error)?;
                let ids = statement
                    .query_map(params![recovered, account, now_ms, limit], |row| {
                        parse(row, 0)
                    })
                    .map_err(sql_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(sql_error)?;
                cursor.account = Some(
                    account
                        .parse()
                        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?,
                );
                cursor.recovered_streak = if recovered {
                    cursor.recovered_streak.saturating_add(1).min(3)
                } else {
                    0
                };
                return Ok(ids);
            }
        }
        Ok(Vec::new())
    }

    /// Claim only the exact identity whose live control references the caller has validated.
    /// A missing or changed projection is never repaired by this execution path.
    pub fn claim_workflow(
        &self,
        identity: &WorkflowInstanceIdentity,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<Option<ClaimedWorkflowRun>, PlatformError> {
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let Some(record) = tx
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1 AND state='queued' AND next_run_at_ms<=?2"),
                params![identity.instance_id.to_string(), now_ms],
                instance_row,
            )
            .optional()
            .map_err(sql_error)?
        else {
            return Ok(None);
        };
        if record.identity != *identity {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        let run_token = token()?;
        tx.execute(
            "UPDATE workflow_instances SET state='running',run_token=?2,run_claimed_at_ms=?3,
             run_lease_until_ms=?4,next_run_at_ms=NULL,updated_at_ms=?3,has_activated=1
             WHERE id=?1 AND state='queued'",
            params![
                identity.instance_id.to_string(),
                run_token.as_bytes().as_slice(),
                now_ms,
                deadline(now_ms, limits.lease_ms)?
            ],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(Some(ClaimedWorkflowRun {
            fence: WorkflowFence {
                instance_id: identity.instance_id,
                instance_generation: identity.instance_generation,
                run_token,
            },
            target: identity.target.clone(),
            external_instance_id: identity.external_instance_id.clone(),
            created_at_ms: identity.created_at_ms,
            schedule: identity.schedule.clone(),
            input_json: record.input_json,
            recovered: record.durable.has_activated,
            rollback: record.durable.rollback_requested,
        }))
    }

    /// Extend a still-live lease. Old tokens and exactly expired leases cannot be revived.
    pub fn heartbeat_workflow(
        &self,
        fence: &WorkflowFence,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        let connection = self.lock()?;
        heartbeat(&connection, fence, now_ms, limits)
    }

    /// Recover a bounded set of expired activations without increasing product attempt.
    pub fn recover_workflows(
        &self,
        now_ms: i64,
        limits: &WorkflowsConfig,
        limit: u32,
    ) -> Result<u64, PlatformError> {
        limits.validate()?;
        bounded(limit)?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let expired = {
            let mut statement = tx
                .prepare(
                    "SELECT id,instance_generation,run_token FROM workflow_instances
                 WHERE state='running' AND run_lease_until_ms<=?1
                 ORDER BY run_lease_until_ms,id LIMIT ?2",
                )
                .map_err(sql_error)?;
            statement
                .query_map(params![now_ms, limit], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?
        };
        for (id, generation, token) in &expired {
            // Reset step ownership before the parent: the schema verifies the expired parent token.
            tx.execute(
                "UPDATE workflow_steps SET state='pending',run_token=NULL,step_token=NULL,updated_at_ms=?4
                 WHERE instance_id=?1 AND instance_generation=?2 AND state='running' AND run_token=?3",
                params![id, generation, token, now_ms],
            ).map_err(sql_error)?;
            let instance = tx
                .query_row(
                    &format!("{INSTANCE_SELECT} WHERE id=?1"),
                    [id],
                    instance_row,
                )
                .map_err(sql_error)?;
            release(
                &tx,
                &instance,
                deadline(now_ms, limits.recovery_backoff_ms)?,
                now_ms,
            )?;
        }
        tx.commit().map_err(sql_error)?;
        if !expired.is_empty() {
            self.wake.notify();
        }
        Ok(expired.len() as u64)
    }

    /// Commit terminal state after the trusted host drained the activation.
    /// Settled business failures may be caught; protocol failures remain a permanent latch.
    pub fn finish_workflow(
        &self,
        fence: &WorkflowFence,
        completion: &WorkflowCompletion,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowState, PlatformError> {
        limits.validate()?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        let metadata = &instance.durable;
        if metadata.pause_requested || metadata.yield_requested {
            let state = release(&tx, &instance, now_ms, now_ms)?;
            tx.commit().map_err(sql_error)?;
            self.wake.notify();
            return Ok(state);
        }
        if let WorkflowCompletion::Terminated { final_ordinal } = completion {
            if !metadata.rollback_requested || *final_ordinal != metadata.registered_step_count {
                return Err(error(ErrorCode::WorkflowNonDeterministic));
            }
            let unfinished: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1
                     AND state IN ('pending','running','delay_pending','retry_wait','waiting'))",
                    [fence.instance_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            if unfinished {
                return Err(error(ErrorCode::WorkflowNonDeterministic));
            }
            tx.execute(
                "UPDATE workflow_instances SET state='terminated',output_json=NULL,error_json=NULL,error_code=NULL,
                 rollback_requested=0,terminal_at_ms=?4,updated_at_ms=?4,expires_at_ms=?5,
                 run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL
                 WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running'",
                params![
                    fence.instance_id.to_string(),
                    fence.instance_generation,
                    fence.run_token.as_bytes().as_slice(),
                    now_ms,
                    metadata.retention.expires_at(now_ms, false)?
                ],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            return Ok(WorkflowState::Terminated);
        }
        if metadata.rollback_requested {
            return Err(error(ErrorCode::WorkflowNonDeterministic));
        }
        let platform_failure:Option<String>=tx.query_row("SELECT error_code FROM workflow_steps WHERE instance_id=?1 AND state='failed'
            AND error_code NOT IN ('WORKFLOW_STEP_TIMEOUT','WORKFLOW_STEP_RETRIES_EXHAUSTED','WORKFLOW_NON_RETRYABLE','WORKFLOW_EVENT_TIMEOUT')
            ORDER BY ordinal LIMIT 1",[fence.instance_id.to_string()],|row|row.get(0)).optional().map_err(sql_error)?;
        let result = if let Some(code) = platform_failure {
            Err(open_compute_core::workflow::terminal_error_code(&code)?)
        } else {
            match completion {
                WorkflowCompletion::Errored { code } => Err(
                    open_compute_core::workflow::terminal_error_code(code.as_str())?,
                ),
                WorkflowCompletion::Complete {
                    output_json,
                    final_ordinal,
                } => {
                    if *final_ordinal != metadata.registered_step_count
                        || metadata.registered_step_count != metadata.settled_step_count
                    {
                        Err(ErrorCode::WorkflowNonDeterministic)
                    } else {
                        open_compute_core::workflow::durable_value_base64(
                            output_json,
                            ErrorCode::WorkflowResultTooLarge,
                        )
                        .and_then(|output| {
                            capacity_change(&tx, &instance, output.len() as i64, -1, limits)?;
                            Ok(output)
                        })
                        .map_err(|error| error.code())
                    }
                }
                WorkflowCompletion::Terminated { .. } => unreachable!("handled above"),
            }
        };
        let (state, encoded, output, failure, code) = match result {
            Ok(output) => (
                WorkflowState::Complete,
                "complete",
                Some(output),
                None,
                None,
            ),
            Err(code) => {
                // All remaining grants are logically fenced by the terminal transaction.
                durable_lifecycle::cancel_unfinished(&tx, fence.instance_id, now_ms)?;
                (
                    WorkflowState::Errored,
                    "errored",
                    None,
                    Some(failure_json()),
                    Some(code.as_str()),
                )
            }
        };
        let added = output.as_ref().map_or(0, String::len) + failure.map_or(0, str::len);
        tx.execute("UPDATE workflow_instances SET state=?4,output_json=?5,error_json=?6,error_code=?7,state_bytes=state_bytes+?8,
            rollback_requested=0,terminal_at_ms=?9,updated_at_ms=?9,expires_at_ms=?10,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL
            WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running'",
            params![fence.instance_id.to_string(),fence.instance_generation,fence.run_token.as_bytes().as_slice(),encoded,
                output.as_ref().map(String::as_bytes),failure.map(str::as_bytes),code,added,now_ms,
                metadata.retention.expires_at(now_ms,state==WorkflowState::Complete)?]).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(state)
    }

    /// Release an activation only after every granted callback has durably relinquished its token.
    /// Registration/request and release are separate phases; persisted pause takes precedence.
    pub fn yield_workflow(
        &self,
        fence: &WorkflowFence,
        now_ms: i64,
    ) -> Result<WorkflowState, PlatformError> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        let durable = &instance.durable;
        if !durable.yield_requested && !durable.pause_requested {
            return Err(error(ErrorCode::WorkflowInstanceStateConflict));
        }
        let state = release(&tx, &instance, now_ms, now_ms)?;
        tx.commit().map_err(sql_error)?;
        self.wake.notify();
        Ok(state)
    }
}

/// Called inside the owner's transaction after validating the live or expired exact run fence.
pub(super) fn release(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    queued_at_ms: i64,
    now_ms: i64,
) -> Result<WorkflowState, PlatformError> {
    let durable = &instance.durable;
    let (running, pending): (bool, bool) = conn
        .query_row(
            "SELECT
        EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND state='running'),
        EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND state IN ('pending','delay_pending'))",
            [instance.identity.instance_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    if running {
        return Err(error(ErrorCode::WorkflowInstanceBusy));
    }
    let (state, encoded, next_run) = if durable.pause_requested {
        (WorkflowState::Paused, "paused", None)
    } else if pending || durable.registered_step_count == durable.settled_step_count {
        (WorkflowState::Queued, "queued", Some(queued_at_ms))
    } else if durable.next_wake_at_ms.is_some() {
        (WorkflowState::Waiting, "waiting", None)
    } else {
        return Err(error(ErrorCode::WorkflowInvariantViolation));
    };
    let token = instance
        .run_token
        .as_ref()
        .ok_or_else(|| error(ErrorCode::WorkflowRunStale))?;
    let changed = conn.execute("UPDATE workflow_instances SET state=?4,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL,
        pause_requested=0,yield_requested=0,next_run_at_ms=?5,updated_at_ms=?6
        WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running' AND capability_version=1",
        params![instance.identity.instance_id.to_string(),instance.identity.instance_generation,token.as_bytes().as_slice(),
            encoded,next_run,now_ms]).map_err(sql_error)?;
    if changed != 1 {
        return Err(error(ErrorCode::WorkflowRunStale));
    }
    Ok(state)
}
