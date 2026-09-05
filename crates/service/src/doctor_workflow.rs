//! Bounded Workflow authority and quota diagnostics.

use super::*;

pub(super) fn inspect(loaded: &LoadedConfig, root: &std::path::Path) -> DoctorCheck {
    match open_compute_storage::scheduler::inspect_workflow_databases(
        &root.join("control.sqlite"),
        &root.join("scheduler.sqlite"),
        loaded.config.data.sqlite_busy_timeout_ms,
        32,
    ) {
        Ok(value) if !value.is_valid() => failed(
            "workflow_authority",
            ErrorCode::WorkflowInvariantViolation,
            "Workflow immutable identity, history, or referrer mismatch",
            Some(format!(
                "identity={} history={} referrers={} operations={}",
                value.identity_mismatches,
                value.history_mismatches,
                value.referrer_mismatches,
                value.operation_mismatches
            )),
        ),
        Ok(value)
            if value.sampled
                || value.pending_creations > 0
                || value.pending_releases > 0
                || value.pending_restarts > 0
                || value.pending_purges > 0
                || value.pending_receipt_sweeps > 0 =>
        {
            warning(
                "workflow_authority",
                "bounded Workflow sample passed; inspect remaining pages or reconcile pending sagas",
                Some(format!(
                    "inspected={} creates={} releases={} restarts={} purges={} receipt_sweeps={}",
                    value.inspected_instances,
                    value.pending_creations,
                    value.pending_releases,
                    value.pending_restarts,
                    value.pending_purges,
                    value.pending_receipt_sweeps
                )),
            )
        }
        Ok(value) => ok(
            "workflow_authority",
            "Workflow frozen identities, history, and references agree",
            Some(value.inspected_instances.to_string()),
        ),
        Err(error) => failed(
            "workflow_authority",
            error.code(),
            "Workflow authority could not be inspected",
            None,
        ),
    }
}
