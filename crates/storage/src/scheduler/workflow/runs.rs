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
        self.lock()?.execute("UPDATE workflow_instances SET next_run_at_ms=?3,updated_at_ms=?2 WHERE id=?1 AND state='queued' AND next_run_at_ms<=?2",params![id.to_string(),now_ms,deadline(now_ms,limits.recovery_backoff_ms)?]).map_err(sql_error)?;
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
                    params![recovered,after,now_ms], |row| row.get(0),
                ).optional().map_err(sql_error)?;
                let Some(account) = account else {
                    continue;
                };
                let mut statement = conn.prepare("SELECT id FROM workflow_instances
                    WHERE state='queued' AND has_activated=?1 AND account_id=?2 AND next_run_at_ms<=?3
                    ORDER BY next_run_at_ms,created_at_ms,id LIMIT ?4").map_err(sql_error)?;
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
        tx.execute("UPDATE workflow_instances SET state='running',run_token=?2,run_claimed_at_ms=?3,run_lease_until_ms=?4,
            next_run_at_ms=NULL,updated_at_ms=?3,has_activated=CASE WHEN capability_version=2 THEN 1 ELSE 0 END
            WHERE id=?1 AND state='queued'",params![identity.instance_id.to_string(),
            run_token.as_bytes().as_slice(),now_ms,deadline(now_ms,limits.lease_ms)?]).map_err(sql_error)?;
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
            input_json: record.input_json,
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
        let conn = self.lock()?;
        heartbeat(&conn, fence, now_ms, limits)
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
            let mut statement = tx.prepare("SELECT id,instance_generation,run_token,capability_version FROM workflow_instances
                WHERE state='running' AND run_lease_until_ms<=?1 ORDER BY run_lease_until_ms,id LIMIT ?2").map_err(sql_error)?;
            statement
                .query_map(params![now_ms, limit], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?
        };
        for (id, generation, token, capability) in &expired {
            // Reset step ownership before the parent: the schema verifies the expired parent token.
            tx.execute("UPDATE workflow_steps SET state='pending',run_token=NULL,step_token=NULL,updated_at_ms=?4
                WHERE instance_id=?1 AND instance_generation=?2 AND state='running' AND run_token=?3",
                params![id,generation,token,now_ms]).map_err(sql_error)?;
            if *capability == 2 {
                let instance = tx
                    .query_row(
                        &format!("{INSTANCE_SELECT} WHERE id=?1"),
                        [id],
                        instance_row,
                    )
                    .map_err(sql_error)?;
                durable_runs::release(
                    &tx,
                    &instance,
                    deadline(now_ms, limits.recovery_backoff_ms)?,
                    now_ms,
                )?;
                continue;
            }
            let changed = tx.execute("UPDATE workflow_instances SET state='queued',run_token=NULL,run_claimed_at_ms=NULL,
                run_lease_until_ms=NULL,next_run_at_ms=?4,updated_at_ms=?5
                WHERE id=?1 AND instance_generation=?2 AND state='running' AND run_token=?3 AND run_lease_until_ms<=?5",
                params![id,generation,token,deadline(now_ms,limits.recovery_backoff_ms)?,now_ms]).map_err(sql_error)?;
            if changed != 1 {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
        tx.commit().map_err(sql_error)?;
        if !expired.is_empty() {
            self.wake.notify();
        }
        Ok(expired.len() as u64)
    }

    /// Commit a known result under the exact live lease. Unknown transport outcomes must not call this.
    /// Returns the durable terminal state, including a failure detected during final validation.
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
        let record = running(&tx, fence, now_ms)?;
        if record.identity.target.capability_version != 1 {
            return Err(error(ErrorCode::WorkflowCapabilityMismatch));
        }
        let result = match completion {
            WorkflowCompletion::Errored { code } => Err(terminal_code(*code)?),
            WorkflowCompletion::Complete {
                output_json,
                final_ordinal,
            } => {
                let failed: Option<String> = tx
                    .query_row(
                        "SELECT error_code FROM workflow_steps
                    WHERE instance_id=?1 AND state='failed' ORDER BY ordinal LIMIT 1",
                        [fence.instance_id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql_error)?;
                let unfinished: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND state IN ('running','pending'))",
                    [fence.instance_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
                if failed.is_some() {
                    Err(ErrorCode::WorkflowExecutionFailed)
                } else if *final_ordinal != record.completed_step_count {
                    Err(ErrorCode::WorkflowNonDeterministic)
                } else if unfinished {
                    Err(ErrorCode::WorkflowParallelStepUnsupported)
                } else if *final_ordinal == 0 {
                    Err(ErrorCode::WorkflowMethodUnsupported)
                } else {
                    open_compute_core::workflow::canonical_json(
                        output_json,
                        ErrorCode::WorkflowResultTooLarge,
                    )
                    .and_then(|output| {
                        capacity(
                            &tx,
                            record.identity.target.account_id,
                            record.state_bytes,
                            output.len(),
                            true,
                            limits,
                        )?;
                        Ok(output)
                    })
                    .map_err(|err| err.code())
                }
            }
        };
        let state = match result {
            Ok(output) => {
                tx.execute("UPDATE workflow_instances SET state='complete',output_json=?4,state_bytes=state_bytes+?5,
                    terminal_at_ms=?6,updated_at_ms=?6,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL
                    WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running' AND run_lease_until_ms>?6",
                    params![fence.instance_id.to_string(),fence.instance_generation,fence.run_token.as_bytes().as_slice(),
                        output.as_bytes(),output.len(),now_ms]).map_err(sql_error)?;
                WorkflowState::Complete
            }
            Err(code) => {
                terminal_error(&tx, fence, code, now_ms)?;
                WorkflowState::Errored
            }
        };
        tx.commit().map_err(sql_error)?;
        Ok(state)
    }
}

pub(super) fn terminal_error(
    conn: &Connection,
    fence: &WorkflowFence,
    code: ErrorCode,
    now_ms: i64,
) -> Result<(), PlatformError> {
    terminal_code(code)?;
    let changed = conn.execute("UPDATE workflow_instances SET state='errored',error_json=?4,error_code=?5,state_bytes=state_bytes+?6,
        terminal_at_ms=?7,updated_at_ms=?7,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL
        WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running' AND run_lease_until_ms>?7",
        params![fence.instance_id.to_string(),fence.instance_generation,fence.run_token.as_bytes().as_slice(),
            failure_json().as_bytes(),code.as_str(),failure_json().len(),now_ms]).map_err(sql_error)?;
    if changed != 1 {
        return Err(error(ErrorCode::WorkflowRunStale));
    }
    Ok(())
}

pub(super) fn terminal_code(code: ErrorCode) -> Result<ErrorCode, PlatformError> {
    match code {
        ErrorCode::WorkflowExecutionFailed
        | ErrorCode::WorkflowNonDeterministic
        | ErrorCode::WorkflowStepConfigUnsupported
        | ErrorCode::WorkflowParallelStepUnsupported
        | ErrorCode::WorkflowMethodUnsupported
        | ErrorCode::WorkflowSerializationUnsupported
        | ErrorCode::WorkflowResultTooLarge
        | ErrorCode::WorkflowStateQuotaExceeded
        | ErrorCode::WorkflowStepLimitExceeded
        | ErrorCode::ArtifactIntegrityError => Ok(code),
        _ => Err(error(ErrorCode::WorkflowInvariantViolation)),
    }
}
