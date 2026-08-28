//! Bounded read-only cross-database Workflow diagnostics.

use super::*;
use crate::{
    ControlDb, WorkflowOperation, WorkflowOperationKind, WorkflowOperationResult, WorkflowRefState,
    WorkflowRepository,
};
use serde::Serialize;

/// Secret-free sampled authority and retained history facts.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDatabaseInspection {
    /// Instances whose two immutable identities disagree or whose control row is absent.
    pub identity_mismatches: u64,
    /// Nonterminal control rows missing a typed artifact/definition reference.
    pub referrer_mismatches: u64,
    /// Histories whose descriptor, frontier, or byte counter is invalid.
    pub history_mismatches: u64,
    /// Recoverable creates not yet finalized in both databases.
    pub pending_creations: u64,
    /// Durable terminal instances awaiting control reference release.
    pub pending_releases: u64,
    /// Restart intents awaiting their exact scheduler/control convergence.
    pub pending_restarts: u64,
    /// Purge intents that still own the old external identity and artifact references.
    pub pending_purges: u64,
    /// Proven deletions whose control side is already absent, ready for receipt sweeping.
    pub pending_receipt_sweeps: u64,
    /// Terminal capability-two instances with healthy retained references.
    pub retained_instances: u64,
    /// Invalid operation results, deletion receipts or escaped transaction contexts.
    pub operation_mismatches: u64,
    /// Number of sampled scheduler histories.
    pub inspected_instances: u64,
    /// A full page was returned; operators must not interpret this as exhaustive validation.
    pub sampled: bool,
}

impl WorkflowDatabaseInspection {
    /// Whether an inspected persisted invariant failed; pending sagas are not corruption.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.identity_mismatches == 0
            && self.referrer_mismatches == 0
            && self.history_mismatches == 0
            && self.operation_mismatches == 0
    }
}

/// Inspect committed WAL-aware authority without migrations, recovery, checkpoints, or repairs.
/// SQLite may maintain ordinary WAL coordination sidecars, but database contents are read-only.
pub fn inspect_workflow_databases(
    control_path: &std::path::Path,
    scheduler_path: &std::path::Path,
    busy_timeout_ms: u64,
    limit: u32,
) -> Result<WorkflowDatabaseInspection, PlatformError> {
    bounded(limit)?;
    let control = ControlDb::open_readonly_wal_aware(control_path, busy_timeout_ms)?;
    let path = crate::control_db::leaf_nofollow_path(scheduler_path)?;
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(sql_error)?;
    connection
        .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
        .map_err(sql_error)?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(sql_error)?;
    let scheduler = SchedulerStore {
        connection: std::sync::Mutex::new(connection),
        wake: std::sync::Arc::new(crate::SchedulerWakeSignal::default()),
    };
    let repository = WorkflowRepository::new(&control);
    let reservations = repository.live_reservations(None, limit)?;
    let ids = scheduler.workflow_instance_ids(None, limit)?;
    let receipts = scheduler.workflow_gc_receipts(None, limit)?;
    let mut result = WorkflowDatabaseInspection {
        sampled: reservations.len() == limit as usize
            || ids.len() == limit as usize
            || receipts.len() == limit as usize,
        ..WorkflowDatabaseInspection::default()
    };
    match durable_progress::sample_operation_progress(&*scheduler.lock()?, limit) {
        Ok(sampled) => result.sampled |= sampled,
        Err(failure) if failure.code() == ErrorCode::WorkflowInvariantViolation => {
            result.operation_mismatches += 1;
        }
        Err(failure) => return Err(failure),
    }
    for reservation in reservations {
        let identity = &reservation.identity;
        let instance = scheduler.workflow_instance(identity.instance_id)?;
        if let Some(operation) = repository.instance_operation(identity.instance_id)? {
            match operation.kind() {
                WorkflowOperationKind::Restart => result.pending_restarts += 1,
                WorkflowOperationKind::Purge => result.pending_purges += 1,
            }
            if !operation_matches(&scheduler, &operation, instance.as_ref())? {
                result.identity_mismatches += 1;
            }
        } else {
            match instance {
                Some(instance) if instance.identity != *identity => result.identity_mismatches += 1,
                Some(instance) if instance.state.is_terminal() => {
                    if reservation.state == WorkflowRefState::Retained {
                        result.retained_instances += 1;
                    } else {
                        result.pending_releases += 1;
                    }
                }
                Some(_) | None if reservation.state == WorkflowRefState::Creating => {
                    result.pending_creations += 1;
                }
                Some(_) if reservation.state != WorkflowRefState::Live => {
                    result.identity_mismatches += 1;
                }
                Some(_) => {}
                None => result.identity_mismatches += 1,
            }
        }
        if !repository.instance_referrers_intact(identity)? {
            result.referrer_mismatches += 1;
        }
    }
    for id in ids {
        let instance = scheduler
            .workflow_instance(id)?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        if let Some(operation) = repository.instance_operation(id)? {
            if !operation_matches(&scheduler, &operation, Some(&instance))? {
                result.identity_mismatches += 1;
            }
        } else {
            match repository.reservation(id)? {
                Some(reservation)
                    if reservation.identity == instance.identity
                        && (instance.state.is_terminal()
                            || matches!(
                                reservation.state,
                                WorkflowRefState::Creating | WorkflowRefState::Live
                            )) => {}
                _ => result.identity_mismatches += 1,
            }
        }
        match scheduler.verify_workflow_history(id) {
            Ok(()) => {}
            Err(error) if error.code() == ErrorCode::WorkflowInvariantViolation => {
                result.history_mismatches += 1;
            }
            Err(error) => return Err(error),
        }
        result.inspected_instances += 1;
    }
    for receipt in receipts {
        match repository.acknowledge_workflow_gc(&receipt) {
            Ok(_) => result.pending_receipt_sweeps += 1,
            Err(failure) if failure.code() == ErrorCode::WorkflowInstanceBusy => {}
            Err(failure) => return Err(failure),
        }
    }
    Ok(result)
}

fn operation_matches(
    scheduler: &SchedulerStore,
    operation: &WorkflowOperation,
    instance: Option<&WorkflowInstanceRecord>,
) -> Result<bool, PlatformError> {
    match scheduler.workflow_operation_result(operation)? {
        Some(WorkflowOperationResult::Applied(_)) => {
            if operation.kind() == WorkflowOperationKind::Purge {
                return Ok(instance.is_none());
            }
            let mut expected = operation.identity().clone();
            expected.instance_generation = operation.target_generation();
            Ok(instance.is_some_and(|instance| instance.identity == expected))
        }
        None | Some(WorkflowOperationResult::Rejected(_)) => {
            Ok(instance.is_some_and(|instance| instance.identity == *operation.identity()))
        }
    }
}
