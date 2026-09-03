//! Workflow domain composition shared by the scheduler, runtime binding, and v4 adapter.

use crate::runtime_bridge::WorkerdTransport;
use open_compute_core::{AccountId, ErrorCode, PlatformError, VersionId, WorkflowId};
use open_compute_storage::{
    PlatformStorage, SchedulerStore, WorkflowDefinitionReservation, WorkflowRepository,
    WorkflowVersion,
};
use std::sync::Arc;

/// Workflow composition over the one catalog, scheduler, and runtime probe authority.
#[derive(Clone, Debug)]
pub struct WorkflowApiState {
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    transport: WorkerdTransport,
    limits: open_compute_core::WorkflowsConfig,
}

impl WorkflowApiState {
    /// Bind the catalog, scheduler authority, and verified runtime class probe.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        scheduler: Arc<SchedulerStore>,
        transport: WorkerdTransport,
        limits: open_compute_core::WorkflowsConfig,
    ) -> Self {
        Self {
            storage,
            scheduler,
            transport,
            limits,
        }
    }

    /// Borrow the platform storage authority.
    #[must_use]
    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    /// Borrow the durable scheduler authority.
    #[must_use]
    pub(crate) fn scheduler(&self) -> &Arc<SchedulerStore> {
        &self.scheduler
    }

    /// Borrow the validated local Workflow policy.
    #[must_use]
    pub(crate) const fn limits(&self) -> &open_compute_core::WorkflowsConfig {
        &self.limits
    }

    /// Freeze a target before probing; an unknown result remains validating for recovery.
    pub async fn create_version(
        &self,
        account: AccountId,
        definition: WorkflowId,
        version: VersionId,
        class_name: String,
    ) -> Result<WorkflowVersion, PlatformError> {
        let storage = self.storage.clone();
        let version = tokio::task::spawn_blocking(move || {
            let _admission = storage.reserve_mutation(64 * 1024)?;
            WorkflowRepository::new(storage.db()).stage_version(
                account,
                definition,
                version,
                &class_name,
                now_ms(),
            )
        })
        .await
        .map_err(|_| unavailable())??;
        validate_version(self.storage.clone(), &self.transport, version).await
    }

    /// Freeze a target through the exact fenced upload-before-PUT reservation.
    pub async fn create_reserved_version(
        &self,
        account: AccountId,
        definition: WorkflowId,
        version: VersionId,
        class_name: String,
        reservation: WorkflowDefinitionReservation,
    ) -> Result<WorkflowVersion, PlatformError> {
        let storage = self.storage.clone();
        let version = tokio::task::spawn_blocking(move || {
            let _admission = storage.reserve_mutation(64 * 1024)?;
            WorkflowRepository::new(storage.db()).stage_reserved_version(
                account,
                definition,
                version,
                &class_name,
                &reservation,
                now_ms(),
            )
        })
        .await
        .map_err(|_| unavailable())??;
        validate_version(self.storage.clone(), &self.transport, version).await
    }
}

/// Validate only a frozen class, then atomically select the newest proven version.
pub(crate) async fn validate_version(
    storage: Arc<PlatformStorage>,
    transport: &WorkerdTransport,
    version: WorkflowVersion,
) -> Result<WorkflowVersion, PlatformError> {
    let probe = transport.probe_workflow(&version.target).await;
    let accepted = match probe {
        Ok(()) => true,
        Err(error)
            if matches!(
                error.code(),
                ErrorCode::WorkflowVersionNotReady
                    | ErrorCode::ArtifactIntegrityError
                    | ErrorCode::WorkflowInvariantViolation
            ) =>
        {
            false
        }
        Err(_) => return Ok(version),
    };
    tokio::task::spawn_blocking(move || {
        WorkflowRepository::new(storage.db()).finish_version(
            version.target.account_id,
            version.target.workflow_version_id,
            accepted,
            now_ms(),
        )
    })
    .await
    .map_err(|_| unavailable())?
}

fn now_ms() -> i64 {
    use open_compute_core::SchedulerClock as _;
    open_compute_core::SystemSchedulerClock.wall_time_ms()
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::WorkflowRuntimeUnavailable,
        "Workflow operation failed",
    )
}

#[cfg(test)]
#[path = "workflow_test_support.rs"]
pub(crate) mod tests;
