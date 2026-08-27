//! Bounded Workflow authority and quota diagnostics.

use super::*;

pub(super) fn inspect(loaded: &LoadedConfig, root: &std::path::Path) -> DoctorCheck {
    match open_compute_storage::scheduler::inspect_workflow_databases(
        &root.join("control.sqlite"),
        &root.join("scheduler.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
        32,
    ) {
        Ok(value) if !value.is_valid() => failed(
            "workflow_authority",
            ErrorCode::WorkflowInvariantViolation,
            "Workflow immutable identity, history, or referrer mismatch",
            Some(format!(
                "identity={} history={} referrers={}",
                value.identity_mismatches, value.history_mismatches, value.referrer_mismatches
            )),
        ),
        Ok(value) if value.sampled || value.pending_creations > 0 || value.pending_releases > 0 => {
            warning(
                "workflow_authority",
                "bounded Workflow sample passed; inspect remaining pages or reconcile pending sagas",
                Some(format!(
                    "inspected={} creates={} releases={}",
                    value.inspected_instances, value.pending_creations, value.pending_releases
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
