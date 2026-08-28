//! Durable sequential Workflow runs and immutable step history.

use super::SchedulerStore;
use crate::{WorkflowInstanceIdentity, WorkflowTarget};
use open_compute_core::{
    ErrorCode, PlatformError, WorkflowFence, WorkflowInstanceId, WorkflowToken, WorkflowsConfig,
};
use rand::TryRngCore as _;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

#[path = "workflow/doctor.rs"]
mod doctor;
#[path = "workflow/durable_due.rs"]
mod durable_due;
#[path = "workflow/durable_history.rs"]
mod durable_history;
#[path = "workflow/durable_lifecycle.rs"]
mod durable_lifecycle;
pub use durable_lifecycle::WorkflowInstanceAction;
#[path = "workflow/durable_gc.rs"]
mod durable_gc;
#[path = "workflow/durable_model.rs"]
mod durable_model;
#[path = "workflow/durable_operations.rs"]
mod durable_operations;
#[path = "workflow/durable_progress.rs"]
mod durable_progress;
pub(super) use durable_progress::verify_operation_progress;
#[path = "workflow/durable_settlement.rs"]
mod durable_settlement;
#[path = "workflow/durable_steps.rs"]
mod durable_steps;
#[path = "workflow/durable_waits.rs"]
mod durable_waits;
pub use durable_model::{
    WorkflowStepAttempt, WorkflowStepOutcome, WorkflowV2StepGrant, WorkflowV2StepResult,
};
#[path = "workflow/durable_runs.rs"]
mod durable_runs;
#[path = "workflow/helpers.rs"]
mod helpers;
pub use doctor::{WorkflowDatabaseInspection, inspect_workflow_databases};
#[path = "workflow/inspection.rs"]
mod inspection;
pub(super) use inspection::verify_legacy_histories;
pub(super) use inspection::{workflow_inspection_connection, workflow_invalid_rows};
#[path = "workflow/model.rs"]
mod model;
#[path = "workflow/runs.rs"]
mod runs;
#[path = "workflow/steps.rs"]
mod steps;
use helpers::*;
pub use model::*;

#[cfg(test)]
#[path = "workflow/workflow_tests.rs"]
mod tests;

impl SchedulerStore {
    /// Insert durable input after control reserved the immutable target and public identity.
    /// A repeat is accepted only if every immutable field and input byte matches.
    /// Retention must be explicitly resolved for V2 and absent for V1.
    pub fn insert_workflow(
        &self,
        identity: &WorkflowInstanceIdentity,
        input: &str,
        retention: Option<&open_compute_core::workflow::WorkflowRetention>,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        match (identity.target.capability_version, retention) {
            (1, None) => {}
            (2, Some(retention)) => {
                retention.validate()?;
                if identity.created_at_ms.unsigned_abs()
                    > open_compute_core::workflow::WORKFLOW_MAX_SAFE_INTEGER
                {
                    return Err(error(ErrorCode::WorkflowDurationInvalid));
                }
            }
            _ => return Err(error(ErrorCode::WorkflowInvariantViolation)),
        }
        open_compute_core::workflow::validate_workflow_instance_id(&identity.external_instance_id)?;
        let input =
            open_compute_core::workflow::canonical_json(input, ErrorCode::WorkflowPayloadTooLarge)?;
        if identity.instance_generation != 1
            || crate::workflows::helpers::version_digest(&identity.target)?
                != identity.target.descriptor_sha256
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if let Some(existing) = tx
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1"),
                [identity.instance_id.to_string()],
                instance_row,
            )
            .optional()
            .map_err(sql_error)?
        {
            if existing.identity != *identity
                || existing.input_json != input
                || existing.durable.as_ref().map(|state| &state.retention) != retention
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            return Ok(());
        }
        let (total,active,per_definition): (u64,u64,u64) = tx.query_row(
            "SELECT COUNT(*),coalesce(SUM(state IN ('queued','running','waiting','paused')),0),coalesce(SUM(definition_id=?2),0)
             FROM workflow_instances WHERE account_id=?1",
            params![identity.target.account_id.to_string(),identity.target.definition_id.to_string()],
            |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(sql_error)?;
        if total >= u64::from(limits.max_instances_per_account)
            || active >= u64::from(limits.max_active_per_account)
            || per_definition >= u64::from(limits.max_instances_per_definition)
        {
            return Err(error(ErrorCode::WorkflowStateQuotaExceeded));
        }
        let target = &identity.target;
        let initial_bytes = initial_state_bytes(identity, input.len());
        capacity(
            &tx,
            target.account_id,
            0,
            initial_bytes + failure_json().len(),
            false,
            limits,
        )?;
        tx.execute("INSERT INTO workflow_instances(id,account_id,definition_id,definition_name,external_instance_id,
            version_id,worker_id,deployment_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,
            class_name,creation_nonce,instance_generation,state,input_json,next_run_at_ms,state_bytes,created_at_ms,updated_at_ms,
            success_retention_ms,error_retention_ms)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,1,'queued',?15,?16,?17,?16,?16,?18,?19)",
            params![identity.instance_id.to_string(),target.account_id.to_string(),target.definition_id.to_string(),target.definition_name,
                identity.external_instance_id,target.version_id.to_string(),target.worker_id.to_string(),target.deployment_id.to_string(),
                target.worker_code_sha256.as_slice(),target.loader_schema_version,target.capability_version,target.descriptor_sha256.as_slice(),
                target.class_name,identity.creation_nonce.as_bytes().as_slice(),input.as_bytes(),identity.created_at_ms,initial_bytes,
                retention.map(|value|value.success_retention_ms),retention.map(|value|value.error_retention_ms)])
            .map_err(sql_error)?;
        tx.commit().map_err(sql_error)
    }

    /// Read authoritative scheduler state without reading any step history.
    pub fn workflow_instance(
        &self,
        id: WorkflowInstanceId,
    ) -> Result<Option<WorkflowInstanceRecord>, PlatformError> {
        self.lock()?
            .query_row(
                &format!("{INSTANCE_SELECT} WHERE id=?1"),
                [id.to_string()],
                instance_row,
            )
            .optional()
            .map_err(sql_error)
    }

    /// Read a bounded reconciliation page, including terminal history for referrer checks.
    pub fn workflow_instance_ids(
        &self,
        after: Option<WorkflowInstanceId>,
        limit: u32,
    ) -> Result<Vec<WorkflowInstanceId>, PlatformError> {
        bounded(limit)?;
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT id FROM workflow_instances WHERE id>?1 ORDER BY id LIMIT ?2")
            .map_err(sql_error)?;
        statement
            .query_map(
                params![after.map_or_else(String::new, |id| id.to_string()), limit],
                |row| {
                    row.get::<_, String>(0)?
                        .parse()
                        .map_err(|_| rusqlite::Error::InvalidQuery)
                },
            )
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)
    }

    /// Preflight state capacity before taking a control reservation; insertion rechecks atomically.
    pub fn check_workflow_create_capacity(
        &self,
        account: open_compute_core::AccountId,
        input_bytes: usize,
        limits: &WorkflowsConfig,
    ) -> Result<(), PlatformError> {
        limits.validate()?;
        let conn = self.lock()?;
        capacity(
            &conn,
            account,
            0,
            input_bytes + failure_json().len(),
            false,
            limits,
        )
    }
}
