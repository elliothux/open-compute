//! Workflow catalog, version, binding, and live-deployment authority.

use crate::{ControlDb, DeploymentState};
use open_compute_core::{
    AccountId, BindingId, DeploymentId, ErrorCode, PlatformError, ResourceAvailability,
    ResourceState, WorkflowId, WorkflowInstanceId, WorkflowToken, WorkflowVersionId,
};
use rusqlite::{OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};

pub(crate) mod bindings;
pub(crate) mod helpers;
mod instances;
pub(crate) mod integrity;
mod model;
pub(crate) mod operations;
mod versions;
use helpers::*;
pub use model::*;
pub use operations::{
    WorkflowAppliedOperation, WorkflowGcAcknowledgement, WorkflowGcReceipt, WorkflowOperation,
    WorkflowOperationKind, WorkflowOperationResult, WorkflowRejectedOperation,
};

#[cfg(test)]
#[path = "workflows/workflow_tests.rs"]
mod tests;

/// Short synchronous transactions over the platform-owned control authority.
#[derive(Clone, Copy, Debug)]
pub struct WorkflowRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> WorkflowRepository<'a> {
    /// Borrow the control database owned by platformd.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Reserve an account-scoped logical definition before validating its first version.
    pub fn create_definition(
        &self,
        account: AccountId,
        name: &str,
        now_ms: i64,
    ) -> Result<WorkflowDefinition, PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        self.db.with_immediate(|tx| {
            if !tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND deleted_at_ms IS NULL)",
                [account.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowNotFound));
            }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE account_id=?1 AND name=?2 AND state!='tombstoned')",
                params![account.to_string(),name], |row| row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowNameConflict));
            }
            let id = WorkflowId::generate();
            tx.execute("INSERT INTO workflow_definitions(id,account_id,name,state,availability,availability_code,
                lifecycle_generation,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'creating','degraded','WORKFLOW_VERSION_NOT_READY',1,?4,?4)",
                params![id.to_string(),account.to_string(),name,now_ms]).map_err(sql_error)?;
            tx.query_row(&format!("{DEFINITION_SELECT} WHERE id=?1"), [id.to_string()], definition_row).map_err(sql_error)
        })
    }

    /// Read a definition only inside its owning account, including retained tombstones.
    pub fn definition(
        &self,
        account: AccountId,
        id: WorkflowId,
    ) -> Result<WorkflowDefinition, PlatformError> {
        self.db.with_read(|conn| {
            conn.query_row(
                &format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                params![account.to_string(), id.to_string()],
                definition_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| error(ErrorCode::WorkflowNotFound))
        })
    }

    /// Bounded account-scoped catalog listing for authenticated operators.
    pub fn definitions(
        &self,
        account: AccountId,
        after: Option<WorkflowId>,
        limit: u32,
    ) -> Result<Vec<WorkflowDefinition>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(error(ErrorCode::LimitInvalid));
        }
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "{DEFINITION_SELECT} WHERE account_id=?1 AND (?2 IS NULL OR id>?2) ORDER BY id LIMIT ?3"
                ))
                .map_err(sql_error)?;
            statement
                .query_map(params![account.to_string(), after.map(|id|id.to_string()), limit], definition_row)
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)
        })
    }

    /// Rename display identity without changing existing instance events or bindings.
    pub fn rename(
        &self,
        account: AccountId,
        id: WorkflowId,
        name: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        open_compute_core::workflow::validate_workflow_name(name)?;
        self.db.with_immediate(|tx| {
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_definitions WHERE account_id=?1 AND name=?2 AND id!=?3 AND state!='tombstoned')",
                params![account.to_string(),name,id.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowNameConflict));
            }
            let changed = tx.execute("UPDATE workflow_definitions SET name=?3,updated_at_ms=?4 WHERE account_id=?1 AND id=?2 AND state IN ('creating','ready')",
                params![account.to_string(),id.to_string(),name,now_ms]).map_err(sql_error)?;
            if changed != 1 { return Err(error(ErrorCode::WorkflowNotReady)); }
            Ok(())
        })
    }

    /// Tombstone only after the unified registry and pending validations are empty.
    pub fn delete(
        &self,
        account: AccountId,
        id: WorkflowId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let definition = tx.query_row(&format!("{DEFINITION_SELECT} WHERE account_id=?1 AND id=?2"),
                params![account.to_string(),id.to_string()],definition_row).optional().map_err(sql_error)?
                .ok_or_else(||error(ErrorCode::WorkflowNotFound))?;
            if definition.state == ResourceState::Tombstoned { return Ok(()); }
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_referrers WHERE definition_id=?1)
                OR EXISTS(SELECT 1 FROM workflow_versions WHERE definition_id=?1 AND state IN ('staging','validating'))",
                [id.to_string()],|row|row.get::<_,bool>(0)).map_err(sql_error)? {
                return Err(error(ErrorCode::WorkflowReferenced));
            }
            tx.execute("UPDATE workflow_definitions SET state='deleting',current_version_id=NULL,updated_at_ms=?2 WHERE id=?1",
                params![id.to_string(),now_ms]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_versions SET state='deleting' WHERE definition_id=?1 AND state IN ('ready','rejected')",
                [id.to_string()]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_versions SET state='tombstoned',deleted_at_ms=?2 WHERE definition_id=?1 AND state='deleting'",
                params![id.to_string(),now_ms]).map_err(sql_error)?;
            tx.execute("UPDATE workflow_definitions SET state='tombstoned',deleted_at_ms=?2,updated_at_ms=?2 WHERE id=?1",
                params![id.to_string(),now_ms]).map_err(sql_error)?;
            Ok(())
        })
    }

    /// Fence admission after an authority mismatch; recovery must prove integrity before clearing it.
    pub fn mark_unavailable(
        &self,
        account: AccountId,
        id: WorkflowId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx.execute("UPDATE workflow_definitions SET availability='unavailable',availability_code='WORKFLOW_INVARIANT_VIOLATION',
                updated_at_ms=?3 WHERE account_id=?1 AND id=?2 AND state IN ('creating','ready')",
                params![account.to_string(),id.to_string(),now_ms]).map_err(sql_error)?;
            if changed != 1 { return Err(error(ErrorCode::WorkflowNotFound)); }
            Ok(())
        })
    }
}
