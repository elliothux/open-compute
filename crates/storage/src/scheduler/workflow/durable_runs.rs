//! Capability V2 activation release after drain or expired-run recovery.

use super::*;

impl SchedulerStore {
    /// Commit V2 terminal state after the trusted host drained the activation.
    /// Settled business failures may be caught; protocol failures remain a permanent latch.
    pub fn finish_workflow_v2(
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
        let metadata = instance
            .durable
            .as_ref()
            .ok_or_else(|| error(ErrorCode::WorkflowCapabilityMismatch))?;
        if metadata.pause_requested || metadata.yield_requested {
            let state = release(&tx, &instance, now_ms, now_ms)?;
            tx.commit().map_err(sql_error)?;
            self.wake.notify();
            return Ok(state);
        }
        let platform_failure:Option<String>=tx.query_row("SELECT error_code FROM workflow_steps WHERE instance_id=?1 AND state='failed'
            AND error_code NOT IN ('WORKFLOW_STEP_TIMEOUT','WORKFLOW_STEP_RETRIES_EXHAUSTED','WORKFLOW_NON_RETRYABLE','WORKFLOW_EVENT_TIMEOUT')
            ORDER BY ordinal LIMIT 1",[fence.instance_id.to_string()],|row|row.get(0)).optional().map_err(sql_error)?;
        let result = if let Some(code) = platform_failure {
            Err(open_compute_core::workflow::terminal_error_code_v2(&code)?)
        } else {
            match completion {
                WorkflowCompletion::Errored { code } => Err(
                    open_compute_core::workflow::terminal_error_code_v2(code.as_str())?,
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
                        open_compute_core::workflow::canonical_json(
                            output_json,
                            ErrorCode::WorkflowResultTooLarge,
                        )
                        .and_then(|output| {
                            capacity_v2(&tx, &instance, output.len() as i64, -1, limits)?;
                            Ok(output)
                        })
                        .map_err(|error| error.code())
                    }
                }
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
            terminal_at_ms=?9,updated_at_ms=?9,expires_at_ms=?10,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL
            WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running'",
            params![fence.instance_id.to_string(),fence.instance_generation,fence.run_token.as_bytes().as_slice(),encoded,
                output.as_ref().map(String::as_bytes),failure.map(str::as_bytes),code,added,now_ms,
                metadata.retention.expires_at(now_ms,state==WorkflowState::Complete)?]).map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        Ok(state)
    }

    /// Release a V2 activation only after every granted callback has durably relinquished its token.
    /// Registration/request and release are separate phases; persisted pause takes precedence.
    pub fn yield_workflow_v2(
        &self,
        fence: &WorkflowFence,
        now_ms: i64,
    ) -> Result<WorkflowState, PlatformError> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = running(&tx, fence, now_ms)?;
        let durable = instance
            .durable
            .as_ref()
            .ok_or_else(|| error(ErrorCode::WorkflowCapabilityMismatch))?;
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
    let durable = instance
        .durable
        .as_ref()
        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
    let (running, pending): (bool, bool) = conn
        .query_row(
            "SELECT
        EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND state='running'),
        EXISTS(SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND state='pending')",
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
        WHERE id=?1 AND instance_generation=?2 AND run_token=?3 AND state='running' AND capability_version=2",
        params![instance.identity.instance_id.to_string(),instance.identity.instance_generation,token.as_bytes().as_slice(),
            encoded,next_run,now_ms]).map_err(sql_error)?;
    if changed != 1 {
        return Err(error(ErrorCode::WorkflowRunStale));
    }
    Ok(state)
}
