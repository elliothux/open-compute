//! Unforgeable cross-database operation decisions and bounded purge acknowledgements.

use super::*;

/// Definitive scheduler rejection; a transport failure is never this proof.
#[derive(Clone, Debug)]
pub struct WorkflowRejectedOperation {
    pub(crate) operation: WorkflowOperation,
    pub(crate) code: ErrorCode,
}
impl WorkflowRejectedOperation {
    /// Stable reason for the committed rejection.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }
}

/// A committed operation decision read from exact scheduler authority.
#[derive(Clone, Debug)]
pub enum WorkflowOperationResult {
    /// The requested restart or deletion durably committed.
    Applied(WorkflowAppliedOperation),
    /// The scheduler durably refused this operation and fenced later retries of it.
    Rejected(WorkflowRejectedOperation),
}

/// Exact scheduler deletion evidence, with its private creation nonce redacted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowGcReceipt {
    pub(crate) operation_id: WorkflowOperationId,
    pub(crate) instance_id: WorkflowInstanceId,
    pub(crate) creation_nonce: WorkflowToken,
    pub(crate) instance_generation: i64,
    pub(crate) sequence: i64,
    pub(crate) deleted_at_ms: i64,
}
impl WorkflowGcReceipt {
    /// Opaque internal identity used only for bounded reconciliation cursors.
    #[must_use]
    pub const fn instance_id(&self) -> WorkflowInstanceId {
        self.instance_id
    }
}

/// Control has released the exact old UUID and has no remaining operation or typed reference.
#[derive(Clone, Debug)]
pub struct WorkflowGcAcknowledgement {
    pub(crate) receipt: WorkflowGcReceipt,
}

impl WorkflowRepository<'_> {
    /// Cancel a prepared operation only after its durable scheduler rejection is proved.
    /// The monotonic reservation sequence is not rolled back or reused.
    pub fn cancel_instance_operation(
        &self,
        proof: &WorkflowRejectedOperation,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let operation = &proof.operation;
        self.db.with_immediate(|tx| {
            let Some(current)=read_operation(tx,operation.identity.instance_id)? else {
                return Ok(());
            };
            if current!=*operation {return Err(error(ErrorCode::WorkflowInstanceBusy));}
            if operation.kind==WorkflowOperationKind::Restart {
                tx.execute("UPDATE workflow_instance_referrers SET state=?2,updated_at_ms=?3 WHERE instance_id=?1 AND state='restarting'",
                    params![operation.identity.instance_id.to_string(),if operation.prior_state==WorkflowRefState::Live {"live"} else {"retained"},now_ms]).map_err(sql_error)?;
            }
            tx.execute("DELETE FROM workflow_instance_operations WHERE operation_id=?1",[operation.id.to_string()]).map_err(sql_error)?;
            Ok(())
        })
    }

    /// Confirm the old UUID has fully left control before sweeping its scheduler receipt.
    /// A concurrently reused public name has another UUID and cannot satisfy this proof.
    pub fn acknowledge_workflow_gc(
        &self,
        receipt: &WorkflowGcReceipt,
    ) -> Result<WorkflowGcAcknowledgement, PlatformError> {
        self.db.with_read(|conn| {
            let remaining:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM workflow_instance_referrers WHERE instance_id=?1)
                OR EXISTS(SELECT 1 FROM workflow_instance_operations WHERE instance_id=?1)
                OR EXISTS(SELECT 1 FROM version_referrers WHERE kind='workflow_instance' AND ref_id=?1)
                OR EXISTS(SELECT 1 FROM workflow_referrers WHERE referrer_kind='instance' AND referrer_id=?1)",
                [receipt.instance_id.to_string()],|row|row.get(0)).map_err(sql_error)?;
            if remaining {return Err(error(ErrorCode::WorkflowInstanceBusy));}
            Ok(WorkflowGcAcknowledgement{receipt:receipt.clone()})
        })
    }
}
