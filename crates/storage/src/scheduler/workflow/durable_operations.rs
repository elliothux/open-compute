//! Exact scheduler restart/purge commits under a persisted control intent.

use super::*;
use crate::{WorkflowOperation, WorkflowOperationKind, WorkflowOperationResult};

impl SchedulerStore {
    /// Apply or replay an exact prepared operation, recording both success and definitive rejection.
    /// Unknown I/O failures leave the control intent intact; callers must reconcile this same intent.
    pub fn apply_workflow_operation(
        &self,
        operation: &WorkflowOperation,
        now_ms: i64,
        limits: &WorkflowsConfig,
    ) -> Result<WorkflowOperationResult, PlatformError> {
        limits.validate()?;
        durable_deadline(now_ms, 0)?;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if let Some(decision) = durable_progress::read_decision(&tx, operation)? {
            return Ok(decision);
        }
        let instance = tx
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1"),
                [operation.identity.instance_id.to_string()],
                instance_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        if instance.identity != operation.identity || operation.sequence < 1 {
            return Err(error(ErrorCode::WorkflowRunStale));
        }
        let metadata = instance
            .durable
            .as_ref()
            .ok_or_else(|| error(ErrorCode::WorkflowMethodUnsupported))?;
        inspection::verify_history_connection(&tx, operation.identity.instance_id)?;
        let rejection = match operation.kind {
            WorkflowOperationKind::Restart
                if metadata
                    .expires_at_ms
                    .is_some_and(|expiry| expiry <= now_ms) =>
            {
                Some(ErrorCode::WorkflowInstanceNotFound)
            }
            WorkflowOperationKind::Purge
                if !instance.state.is_terminal()
                    || metadata.expires_at_ms.is_none_or(|expiry| expiry > now_ms) =>
            {
                Some(ErrorCode::WorkflowInstanceStateConflict)
            }
            WorkflowOperationKind::Restart => restart_capacity(&tx, &instance, limits)
                .err()
                .map(|error| error.code()),
            WorkflowOperationKind::Purge => None,
        };
        if let Some(code) = rejection {
            if !matches!(
                code,
                ErrorCode::WorkflowInstanceNotFound
                    | ErrorCode::WorkflowInstanceStateConflict
                    | ErrorCode::WorkflowStateQuotaExceeded
            ) {
                return Err(error(code));
            }
            let decision = durable_progress::decide(&tx, operation, Some(code), now_ms)?;
            tx.commit().map_err(sql_error)?;
            return Ok(decision);
        }
        insert_context(&tx, operation, now_ms)?;
        for table in [
            "workflow_step_dependencies",
            "workflow_steps",
            "workflow_events",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE instance_id=?1 AND instance_generation=?2"),
                params![
                    operation.identity.instance_id.to_string(),
                    operation.identity.instance_generation
                ],
            )
            .map_err(sql_error)?;
        }
        match operation.kind {
            WorkflowOperationKind::Restart => {
                tx.execute("UPDATE workflow_instances SET instance_generation=?2,state='queued',next_run_at_ms=?3,
                    output_json=NULL,error_json=NULL,error_code=NULL,run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL,
                    pause_requested=0,yield_requested=0,terminal_at_ms=NULL,expires_at_ms=NULL,next_event_seq=1,has_activated=0,
                    last_restart_operation_id=?4,state_bytes=?5,updated_at_ms=?3 WHERE id=?1",
                    params![operation.identity.instance_id.to_string(),operation.target_generation,now_ms,operation.id.to_string(),
                        initial_state_bytes(&instance.identity,instance.input_json.len())]).map_err(sql_error)?;
            }
            WorkflowOperationKind::Purge => {
                tx.execute(
                    "DELETE FROM workflow_instances WHERE id=?1",
                    [operation.identity.instance_id.to_string()],
                )
                .map_err(sql_error)?;
                tx.execute("INSERT INTO workflow_gc_receipts(operation_id,instance_id,creation_nonce,instance_generation,deleted_at_ms)
                    VALUES(?1,?2,?3,?4,?5)",params![operation.id.to_string(),operation.identity.instance_id.to_string(),
                        operation.identity.creation_nonce.as_bytes().as_slice(),operation.identity.instance_generation,now_ms]).map_err(sql_error)?;
            }
        }
        let decision = durable_progress::decide(&tx, operation, None, now_ms)?;
        tx.execute(
            "DELETE FROM workflow_mutation_context WHERE instance_id=?1",
            [operation.identity.instance_id.to_string()],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
        self.wake.notify();
        Ok(decision)
    }

    /// Read an exact committed operation result without applying, repairing or deleting anything.
    pub fn workflow_operation_result(
        &self,
        operation: &WorkflowOperation,
    ) -> Result<Option<WorkflowOperationResult>, PlatformError> {
        let conn = self.lock()?;
        durable_progress::read_decision(&conn, operation)
    }
}

fn insert_context(
    conn: &Connection,
    operation: &WorkflowOperation,
    now_ms: i64,
) -> Result<(), PlatformError> {
    conn.execute("INSERT INTO workflow_mutation_context(instance_id,operation_id,creation_nonce,expected_generation,target_generation,kind,authorized_at_ms)
        VALUES(?1,?2,?3,?4,?5,?6,?7)",params![operation.identity.instance_id.to_string(),operation.id.to_string(),
            operation.identity.creation_nonce.as_bytes().as_slice(),operation.identity.instance_generation,operation.target_generation,
            operation.kind.as_str(),now_ms]).map_err(sql_error)?;
    Ok(())
}

fn restart_capacity(
    conn: &Connection,
    instance: &WorkflowInstanceRecord,
    limits: &WorkflowsConfig,
) -> Result<(), PlatformError> {
    if instance.state.is_terminal() {
        let active:u64=conn.query_row("SELECT COUNT(*) FROM workflow_instances WHERE account_id=?1 AND state IN ('queued','running','waiting','paused')",
            [instance.identity.target.account_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
        if active >= u64::from(limits.max_active_per_account) {
            return Err(error(ErrorCode::WorkflowStateQuotaExceeded));
        }
    }
    let reserved:i64=conn.query_row("SELECT (state IN ('queued','running','waiting','paused')) + (SELECT COUNT(*) FROM workflow_steps
        WHERE instance_id=?1 AND kind IN ('do','wait_event') AND state IN ('pending','running','waiting')) FROM workflow_instances WHERE id=?1",
        [instance.identity.instance_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
    let base = i64::try_from(initial_state_bytes(
        &instance.identity,
        instance.input_json.len(),
    ))
    .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
    let current = i64::try_from(instance.state_bytes)
        .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?;
    capacity_v2(conn, instance, base - current, 1 - reserved, limits)
}
