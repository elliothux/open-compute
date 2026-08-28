//! Bounded retention enumeration and proof-gated receipt sweeping.

use super::*;
use crate::{WorkflowGcAcknowledgement, WorkflowGcReceipt};
use open_compute_core::WorkflowOperationId;

impl SchedulerStore {
    /// Enumerate one bounded page of logically expired capability-two identities.
    pub fn expired_workflows_v2(
        &self,
        now_ms: i64,
        limit: u32,
    ) -> Result<Vec<WorkflowInstanceIdentity>, PlatformError> {
        bounded(limit)?;
        let conn = self.lock()?;
        let mut statement=conn.prepare(&format!("{INSTANCE_SELECT} WHERE capability_version=2 AND state IN ('complete','errored','terminated')
            AND expires_at_ms<=?1 ORDER BY expires_at_ms,id LIMIT ?2")).map_err(sql_error)?;
        statement
            .query_map(params![now_ms, limit], instance_row)
            .map_err(sql_error)?
            .map(|row| row.map(|record| record.identity))
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)
    }

    /// Read a bounded receipt page, requiring the matching committed purge decision.
    pub fn workflow_gc_receipts(
        &self,
        after: Option<WorkflowInstanceId>,
        limit: u32,
    ) -> Result<Vec<WorkflowGcReceipt>, PlatformError> {
        bounded(limit)?;
        let conn = self.lock()?;
        let mut statement=conn.prepare("SELECT instance_id FROM workflow_gc_receipts WHERE instance_id>?1 ORDER BY instance_id LIMIT ?2").map_err(sql_error)?;
        let ids = statement
            .query_map(
                params![after.map_or_else(String::new, |id| id.to_string()), limit],
                |row| parse::<WorkflowInstanceId>(row, 0),
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        ids.into_iter()
            .map(|id| {
                read_receipt(&conn, id)?.ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))
            })
            .collect()
    }

    /// Sweep a receipt only after the control owner proves all references to this UUID are gone.
    pub fn sweep_workflow_gc(
        &self,
        acknowledgement: &WorkflowGcAcknowledgement,
    ) -> Result<(), PlatformError> {
        let receipt = &acknowledgement.receipt;
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let Some(current) = read_receipt(&tx, receipt.instance_id)? else {
            return Ok(());
        };
        if current != *receipt {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        tx.execute("INSERT INTO workflow_mutation_context(instance_id,operation_id,creation_nonce,expected_generation,target_generation,kind,authorized_at_ms)
            VALUES(?1,?2,?3,?4,?4,'acknowledge_purge',?5)",params![receipt.instance_id.to_string(),receipt.operation_id.to_string(),
                receipt.creation_nonce.as_bytes().as_slice(),receipt.instance_generation,receipt.deleted_at_ms]).map_err(sql_error)?;
        tx.execute(
            "DELETE FROM workflow_operation_progress WHERE instance_id=?1",
            [receipt.instance_id.to_string()],
        )
        .map_err(sql_error)?;
        tx.execute(
            "DELETE FROM workflow_gc_receipts WHERE instance_id=?1",
            [receipt.instance_id.to_string()],
        )
        .map_err(sql_error)?;
        tx.execute(
            "DELETE FROM workflow_mutation_context WHERE instance_id=?1",
            [receipt.instance_id.to_string()],
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)
    }
}

fn read_receipt(
    conn: &Connection,
    id: WorkflowInstanceId,
) -> Result<Option<WorkflowGcReceipt>, PlatformError> {
    let row=conn.query_row("SELECT operation_id,creation_nonce,instance_generation,deleted_at_ms FROM workflow_gc_receipts WHERE instance_id=?1",
        [id.to_string()],|row|Ok((parse::<WorkflowOperationId>(row,0)?,WorkflowToken::from_bytes(digest(row,1)?),row.get::<_,i64>(2)?,row.get::<_,i64>(3)?))).optional().map_err(sql_error)?;
    let Some((operation_id, nonce, generation, deleted)) = row else {
        return Ok(None);
    };
    let sequence:Option<i64>=conn.query_row("SELECT operation_sequence FROM workflow_operation_progress WHERE instance_id=?1 AND operation_id=?2
        AND creation_nonce=?3 AND expected_generation=?4 AND target_generation=?4 AND kind='purge' AND outcome='applied'
        AND NOT EXISTS(SELECT 1 FROM workflow_instances WHERE id=?1)",params![id.to_string(),operation_id.to_string(),nonce.as_bytes().as_slice(),generation],|row|row.get(0)).optional().map_err(sql_error)?;
    Ok(Some(WorkflowGcReceipt {
        operation_id,
        instance_id: id,
        creation_nonce: nonce,
        instance_generation: generation,
        sequence: sequence.ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
        deleted_at_ms: deleted,
    }))
}
