//! Bounded durable deadline maintenance, without activating paused instances.

use super::*;

impl SchedulerStore {
    /// Settle at most one bounded page of due waits/recovered timeouts and enqueue eligible continuations.
    /// Business attempt counters change only in claim, never during maintenance or infrastructure recovery.
    pub fn maintain_workflow_due(
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
        let due = {
            let mut statement = tx.prepare(DUE_STEPS).map_err(sql_error)?;
            statement
                .query_map(params![now_ms, limit], |row| {
                    Ok((parse::<WorkflowInstanceId>(row, 0)?, row.get::<_, u32>(1)?))
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?
        };
        for (id, ordinal) in &due {
            let instance = tx
                .query_row(
                    &format!("{INSTANCE_SELECT} WHERE id=?1"),
                    [id.to_string()],
                    instance_row,
                )
                .map_err(sql_error)?;
            let step = durable_model::read_step(
                &tx,
                *id,
                instance.identity.instance_generation,
                *ordinal,
            )?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            let ready = match step.state.as_str() {
                "waiting" => !matches!(
                    durable_waits::settle(&tx, &instance, &step, now_ms, limits)?,
                    WorkflowStepResult::Suspended
                ),
                "pending" => !matches!(
                    durable_settlement::timeout(
                        &tx,
                        *id,
                        instance.identity.instance_generation,
                        &step,
                        now_ms,
                    )?,
                    WorkflowStepResult::Suspended
                ),
                "delay_pending" => true,
                "retry_wait" => true,
                _ => return Err(error(ErrorCode::WorkflowInvariantViolation)),
            };
            if ready {
                durable_waits::wake(&tx, *id, now_ms)?;
            }
        }
        tx.commit().map_err(sql_error)?;
        if !due.is_empty() {
            self.wake.notify();
        }
        Ok(due.len() as u64)
    }
}

// Each indexed source contributes at most one page, including after a large clock jump.
// The final merge never sorts all retained or due step history.
pub(super) const DUE_STEPS: &str = "SELECT instance_id,ordinal FROM (
    SELECT * FROM (SELECT s.instance_id,s.ordinal,s.due_at_ms AS deadline FROM workflow_steps s
      JOIN workflow_instances i ON i.id=s.instance_id WHERE s.state='waiting' AND s.due_at_ms<=?1
        AND i.capability_version=1 AND i.state IN ('queued','running','waiting','paused')
      ORDER BY s.due_at_ms,s.instance_id,s.ordinal LIMIT ?2)
    UNION ALL
    SELECT * FROM (SELECT s.instance_id,s.ordinal,s.attempt_deadline_at_ms AS deadline FROM workflow_steps s
      JOIN workflow_instances i ON i.id=s.instance_id WHERE s.state='pending' AND s.attempt>0 AND s.attempt_deadline_at_ms<=?1
        AND i.capability_version=1 AND i.state IN ('queued','running','waiting','paused')
      ORDER BY s.attempt_deadline_at_ms,s.instance_id,s.ordinal LIMIT ?2)
    UNION ALL
    SELECT * FROM (SELECT s.instance_id,s.ordinal,s.updated_at_ms AS deadline FROM workflow_steps s
      JOIN workflow_instances i ON i.id=s.instance_id WHERE s.state='delay_pending' AND s.updated_at_ms<=?1
        AND i.capability_version=1 AND i.state IN ('queued','running','waiting','paused')
      ORDER BY s.updated_at_ms,s.instance_id,s.ordinal LIMIT ?2)
    UNION ALL
    SELECT * FROM (SELECT s.instance_id,s.ordinal,s.due_at_ms AS deadline FROM workflow_steps s
      JOIN workflow_instances i ON i.id=s.instance_id WHERE s.state='retry_wait' AND s.due_at_ms<=?1
        AND i.capability_version=1 AND i.state='waiting'
      ORDER BY s.due_at_ms,s.instance_id,s.ordinal LIMIT ?2)
) ORDER BY deadline,instance_id,ordinal LIMIT ?2";
