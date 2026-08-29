//! Single-database lifecycle transitions, retaining existing outputs and attempt deadlines.

use super::*;

/// A lifecycle mutation that does not change the immutable execution generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowInstanceAction {
    /// Stop issuing grants and pause once existing callbacks have drained.
    Pause,
    /// Continue a paused instance without extending its wall-clock deadlines.
    Resume,
    /// Immediately fence further platform execution and retain diagnostic history.
    Terminate,
}

impl SchedulerStore {
    /// Apply an admitted lifecycle action to an exact current generation.
    /// Termination fences platform commits; it does not cancel external side effects.
    pub fn modify_workflow(
        &self,
        identity: &WorkflowInstanceIdentity,
        action: WorkflowInstanceAction,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        durable_deadline(now_ms, 0)?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let instance = tx
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1"),
                [identity.instance_id.to_string()],
                instance_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        if instance.identity != *identity {
            return Err(error(ErrorCode::WorkflowRunStale));
        }
        let durable = &instance.durable;
        if durable.expires_at_ms.is_some_and(|expiry| expiry <= now_ms) {
            return Err(error(ErrorCode::WorkflowInstanceNotFound));
        }
        if instance.state.is_terminal() {
            return Err(error(ErrorCode::WorkflowInstanceStateConflict));
        }
        match action {
            WorkflowInstanceAction::Pause => match instance.state {
                WorkflowState::Paused => {}
                WorkflowState::Running if durable.pause_requested => {}
                WorkflowState::Running => {
                    tx.execute("UPDATE workflow_instances SET pause_requested=1,updated_at_ms=?2 WHERE id=?1",
                        params![identity.instance_id.to_string(),now_ms]).map_err(sql_error)?;
                }
                WorkflowState::Queued | WorkflowState::Waiting => {
                    tx.execute("UPDATE workflow_instances SET state='paused',next_run_at_ms=NULL,updated_at_ms=?2 WHERE id=?1",
                        params![identity.instance_id.to_string(),now_ms]).map_err(sql_error)?;
                }
                _ => return Err(error(ErrorCode::WorkflowInstanceStateConflict)),
            },
            WorkflowInstanceAction::Resume => {
                if instance.state != WorkflowState::Paused {
                    return Err(error(ErrorCode::WorkflowInstanceStateConflict));
                }
                let due = {
                    let mut statement=tx.prepare("SELECT ordinal FROM workflow_steps WHERE instance_id=?1 AND
                        ((state='waiting' AND due_at_ms<=?2) OR (state='pending' AND attempt>0 AND attempt_deadline_at_ms<=?2))
                        ORDER BY ordinal").map_err(sql_error)?;
                    statement
                        .query_map(params![identity.instance_id.to_string(), now_ms], |row| {
                            row.get::<_, u32>(0)
                        })
                        .map_err(sql_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(sql_error)?
                };
                // Descriptor and batch limits bound this per-instance transaction. The same arbiter
                // handles maintenance, resumed deadlines and event admission.
                for ordinal in due {
                    let step = durable_model::read_step(
                        &tx,
                        identity.instance_id,
                        identity.instance_generation,
                        ordinal,
                    )?
                    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
                    if step.state == "waiting" {
                        durable_waits::settle(&tx, &instance, &step, now_ms, limits)?;
                    } else {
                        durable_settlement::fail(
                            &tx,
                            identity.instance_id,
                            identity.instance_generation,
                            &step,
                            ErrorCode::WorkflowStepTimeout,
                            now_ms,
                        )?;
                    }
                }
                let ready:bool=tx.query_row("SELECT registered_step_count=settled_step_count OR EXISTS(
                    SELECT 1 FROM workflow_steps WHERE instance_id=?1 AND (state='pending' OR (state='retry_wait' AND due_at_ms<=?2)))
                    FROM workflow_instances WHERE id=?1",params![identity.instance_id.to_string(),now_ms],|row|row.get(0)).map_err(sql_error)?;
                tx.execute("UPDATE workflow_instances SET state=?2,next_run_at_ms=?3,updated_at_ms=?4 WHERE id=?1",
                    params![identity.instance_id.to_string(),if ready {"queued"} else {"waiting"},ready.then_some(now_ms),now_ms]).map_err(sql_error)?;
            }
            WorkflowInstanceAction::Terminate => {
                cancel_unfinished(&tx, identity.instance_id, now_ms)?;
                tx.execute("UPDATE workflow_instances SET state='terminated',next_run_at_ms=NULL,run_token=NULL,run_claimed_at_ms=NULL,
                    run_lease_until_ms=NULL,pause_requested=0,yield_requested=0,terminal_at_ms=?2,expires_at_ms=?3,updated_at_ms=?2 WHERE id=?1",
                    params![identity.instance_id.to_string(),now_ms,durable.retention.expires_at(now_ms,false)?]).map_err(sql_error)?;
            }
        }
        tx.commit().map_err(sql_error)?;
        self.wake.notify();
        Ok(())
    }
}

pub(super) fn cancel_unfinished(
    conn: &Connection,
    id: WorkflowInstanceId,
    now_ms: i64,
) -> Result<(), PlatformError> {
    conn.execute(
        "UPDATE workflow_steps SET state='cancelled',run_token=NULL,step_token=NULL,due_at_ms=NULL,
        error_json=NULL,error_code=NULL,cancelled_at_ms=?2,updated_at_ms=?2
        WHERE instance_id=?1 AND state IN ('pending','running','waiting','retry_wait')",
        params![id.to_string(), now_ms],
    )
    .map_err(sql_error)?;
    Ok(())
}
