//! Workflow create, claim eligibility, and cross-database recovery orchestration.

use open_compute_core::{
    AccountId, ErrorCode, OperationClass, PlatformError, ResourceAvailability, ResourceState,
    WorkflowId, WorkflowInstanceId, WorkflowsConfig,
};
use open_compute_storage::scheduler::{ClaimedWorkflowRun, WorkflowFailure, WorkflowState};
use open_compute_storage::{
    PlatformStorage, SchedulerStore, WorkflowInstanceIdentity, WorkflowRefState, WorkflowRepository,
};
use serde::Serialize;

/// Tenant-visible status with no internal identifiers, tokens, or execution topology.
#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// Durably created and waiting for admission.
    Queued,
    /// Currently running, with private lease metadata omitted.
    Running,
    /// Durable parsed output.
    Complete {
        /// User result, never included in diagnostic formatting.
        output: serde_json::Value,
    },
    /// Durable sanitized failure.
    Errored {
        /// Safe name and message without tenant exception values.
        error: WorkflowFailure,
    },
}

impl std::fmt::Debug for WorkflowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Complete { .. } => "Complete",
            Self::Errored { .. } => "Errored",
        })
    }
}

/// Bounded independent cursors for control reservations and scheduler authority.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkflowReconcileCursor {
    /// Last inspected control reservation.
    pub control: Option<WorkflowInstanceId>,
    /// Last inspected scheduler instance.
    pub scheduler: Option<WorkflowInstanceId>,
}

/// Workflow lifecycle owner; execution authority never moves into process memory.
#[derive(Debug)]
pub struct WorkflowController<'a> {
    storage: &'a PlatformStorage,
    scheduler: &'a SchedulerStore,
    limits: &'a WorkflowsConfig,
}

impl<'a> WorkflowController<'a> {
    /// Borrow platform-owned authorities and already validated local policy.
    #[must_use]
    pub const fn new(
        storage: &'a PlatformStorage,
        scheduler: &'a SchedulerStore,
        limits: &'a WorkflowsConfig,
    ) -> Self {
        Self {
            storage,
            scheduler,
            limits,
        }
    }

    /// Create a durable handle only after both databases commit their exact shared identity.
    pub fn create(
        &self,
        account: AccountId,
        definition: WorkflowId,
        external_id: Option<&str>,
        payload: &str,
        now_ms: i64,
    ) -> Result<String, PlatformError> {
        let payload = open_compute_core::workflow::canonical_json(
            payload,
            ErrorCode::WorkflowPayloadTooLarge,
        )?;
        self.scheduler
            .check_workflow_create_capacity(account, payload.len(), self.limits)?;
        let _reservation = self
            .storage
            .reserve_mutation(OperationClass::Scheduler, payload.len() as u64 + 64 * 1024)?;
        let repository = WorkflowRepository::new(self.storage.db());
        let reservation =
            repository.reserve_instance(account, definition, external_id, self.limits, now_ms)?;
        let identity = reservation.identity;
        if let Err(err) = self
            .scheduler
            .insert_workflow(&identity, &payload, self.limits)
        {
            // A failed commit response is not proof of absence. Never delete a possibly durable instance pin.
            if self
                .scheduler
                .workflow_instance(identity.instance_id)?
                .is_none()
            {
                repository.abandon_creation(&identity)?;
            }
            return Err(err);
        }
        repository.finalize_instance(&identity, now_ms)?;
        self.scheduler.wake_signal().notify();
        Ok(identity.external_instance_id)
    }

    /// Resolve a definition-scoped handle and return scheduler authority without loading step history.
    pub fn status(
        &self,
        account: AccountId,
        definition: WorkflowId,
        external_id: &str,
    ) -> Result<WorkflowStatus, PlatformError> {
        let repository = WorkflowRepository::new(self.storage.db());
        repository.definition(account, definition)?;
        let reservation = repository.find_instance(definition, external_id)?;
        let instance = self
            .scheduler
            .workflow_instance(reservation.identity.instance_id)?
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        if reservation.identity != instance.identity {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        match instance.state {
            WorkflowState::Queued => Ok(WorkflowStatus::Queued),
            WorkflowState::Running => Ok(WorkflowStatus::Running),
            WorkflowState::Complete => Ok(WorkflowStatus::Complete {
                output: open_compute_core::workflow::decode_json(
                    instance
                        .output_json
                        .as_deref()
                        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
                    ErrorCode::WorkflowInvariantViolation,
                )
                .map_err(|_| error(ErrorCode::WorkflowInvariantViolation))?,
            }),
            WorkflowState::Errored => Ok(WorkflowStatus::Errored {
                error: instance
                    .error
                    .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?,
            }),
        }
    }

    /// Admit one due exact identity only after checking live catalog and typed artifact references.
    pub fn claim(&self, now_ms: i64) -> Result<Option<ClaimedWorkflowRun>, PlatformError> {
        self.storage.admission_snapshot()?.admit(64 * 1024)?;
        let repository = WorkflowRepository::new(self.storage.db());
        self.scheduler.recover_workflows(now_ms, self.limits, 32)?;
        for id in self.scheduler.due_workflows(now_ms, 32)? {
            let instance = self
                .scheduler
                .workflow_instance(id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            let identity = &instance.identity;
            let reservation = repository
                .reservation(id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            if reservation.identity != *identity {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            if reservation.state == WorkflowRefState::Creating {
                self.scheduler.defer_workflow(id, now_ms, self.limits)?;
                continue;
            }
            if reservation.state != WorkflowRefState::Live
                || !repository.instance_referrers_intact(identity)?
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            let definition =
                repository.definition(identity.target.account_id, identity.target.definition_id)?;
            if definition.state != ResourceState::Ready
                || definition.availability != ResourceAvailability::Healthy
            {
                self.scheduler.defer_workflow(id, now_ms, self.limits)?;
                continue;
            }
            // Admission must not outrun a bounded background integrity sample.
            // Never execute a callback against a corrupt frozen descriptor or frontier.
            if let Err(error) = self.scheduler.verify_workflow_history(id) {
                if error.code() == ErrorCode::WorkflowRuntimeUnavailable {
                    return Err(error);
                }
                return self.corrupt(identity, now_ms);
            }
            if let Some(run) = self
                .scheduler
                .claim_workflow(identity, now_ms, self.limits)?
            {
                return Ok(Some(run));
            }
        }
        Ok(None)
    }

    /// Boundedly reconcile creation/release sagas and verify history; never invent missing output.
    pub fn reconcile(
        &self,
        cursor: &mut WorkflowReconcileCursor,
        limit: u32,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let repository = WorkflowRepository::new(self.storage.db());
        let reservations = repository.live_reservations(cursor.control, limit)?;
        let next_control = reservations.last().map(|row| row.identity.instance_id);
        for reservation in reservations {
            let identity = &reservation.identity;
            match self.scheduler.workflow_instance(identity.instance_id)? {
                Some(instance) => {
                    if instance.identity != *identity {
                        return self.corrupt(identity, now_ms);
                    }
                    if instance.state.is_terminal() {
                        repository.release_instance(identity, now_ms)?;
                    } else {
                        if !matches!(
                            reservation.state,
                            WorkflowRefState::Creating | WorkflowRefState::Live
                        ) {
                            return self.corrupt(identity, now_ms);
                        }
                        repository.repair_instance_referrers(identity)?;
                        repository.finalize_instance(identity, now_ms)?;
                    }
                }
                None if reservation.state == WorkflowRefState::Creating => {
                    if now_ms.saturating_sub(identity.created_at_ms)
                        >= i64::try_from(self.limits.creation_grace_ms)
                            .map_err(|_| error(ErrorCode::LimitInvalid))?
                    {
                        repository.abandon_creation(identity)?;
                    }
                }
                None => return self.corrupt(identity, now_ms),
            }
        }
        cursor.control = next_control;
        let ids = self
            .scheduler
            .workflow_instance_ids(cursor.scheduler, limit)?;
        let next_scheduler = ids.last().copied();
        for id in ids {
            let instance = self
                .scheduler
                .workflow_instance(id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            let identity = &instance.identity;
            let reservation = repository.reservation(id)?;
            if reservation.as_ref().is_none_or(|row| {
                row.identity != *identity
                    || (!instance.state.is_terminal() && row.state == WorkflowRefState::Released)
            }) {
                return self.corrupt(identity, now_ms);
            }
            if let Err(err) = self.scheduler.verify_workflow_history(id) {
                if err.code() == ErrorCode::WorkflowRuntimeUnavailable {
                    return Err(err);
                }
                return self.corrupt(identity, now_ms);
            }
        }
        cursor.scheduler = next_scheduler;
        self.scheduler
            .recover_workflows(now_ms, self.limits, limit)?;
        repository.retire_unused_versions(limit, now_ms)?;
        self.scheduler.wake_signal().notify();
        Ok(())
    }

    fn corrupt<T>(
        &self,
        identity: &WorkflowInstanceIdentity,
        now_ms: i64,
    ) -> Result<T, PlatformError> {
        WorkflowRepository::new(self.storage.db()).mark_unavailable(
            identity.target.account_id,
            identity.target.definition_id,
            now_ms,
        )?;
        Err(error(ErrorCode::WorkflowInvariantViolation))
    }
}

fn error(code: ErrorCode) -> PlatformError {
    PlatformError::new(code, "Workflow operation failed")
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
