//! Workflow create, claim eligibility, and cross-database recovery orchestration.

use open_compute_core::{
    AccountId, ErrorCode, PlatformError, ResourceAvailability, ResourceState, WorkflowId,
    WorkflowInstanceId, WorkflowsConfig,
};
use open_compute_storage::scheduler::{ClaimedWorkflowRun, WorkflowFailure, WorkflowState};
use open_compute_storage::{
    PlatformStorage, SchedulerStore, WorkflowInstanceIdentity, WorkflowRefState, WorkflowRepository,
};
use serde::Serialize;

#[path = "workflow_lifecycle.rs"]
mod lifecycle;

/// Input and optional resolved retention at the instance-creation authority boundary.
#[derive(Clone, Copy)]
pub struct WorkflowCreateInput<'a> {
    /// Serialized supported JSON input; normalized before either database is mutated.
    pub payload_json: &'a str,
    /// Explicit retention override.
    pub retention: Option<&'a open_compute_core::workflow::WorkflowRetention>,
}
impl std::fmt::Debug for WorkflowCreateInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkflowCreateInput([REDACTED])")
    }
}

/// Tenant-visible status with no internal identifiers, tokens, or execution topology.
#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// Durably created and waiting for admission.
    Queued,
    /// Currently running, with private lease metadata omitted.
    Running,
    /// Pending a durable event or deadline, without a run lease.
    Waiting,
    /// An active callback is draining in response to a pause request.
    #[serde(rename = "waitingForPause")]
    WaitingForPause,
    /// Explicitly paused without a run lease.
    Paused,
    /// Explicitly terminated; history remains readable until expiry.
    Terminated,
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
            Self::Waiting => "Waiting",
            Self::WaitingForPause => "WaitingForPause",
            Self::Paused => "Paused",
            Self::Terminated => "Terminated",
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
    /// Last inspected cross-database operation intent.
    pub operation: Option<open_compute_core::WorkflowOperationId>,
    /// Last inspected scheduler GC receipt.
    pub gc_receipt: Option<WorkflowInstanceId>,
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
        input: WorkflowCreateInput<'_>,
        now_ms: i64,
    ) -> Result<WorkflowInstanceIdentity, PlatformError> {
        let retention = input.retention.unwrap_or(&self.limits.default_retention);
        retention.validate()?;
        let payload = open_compute_core::workflow::canonical_json(
            input.payload_json,
            ErrorCode::WorkflowPayloadTooLarge,
        )?;
        self.scheduler
            .check_workflow_create_capacity(account, payload.len(), self.limits)?;
        let _reservation = self
            .storage
            .reserve_mutation(payload.len() as u64 + 64 * 1024)?;
        let repository = WorkflowRepository::new(self.storage.db());
        let reservation = match repository.reserve_instance(
            account,
            definition,
            external_id,
            self.limits,
            now_ms,
        ) {
            Ok(reservation) => reservation,
            Err(failure) if failure.code() == ErrorCode::WorkflowInstanceAlreadyExists => {
                return Err(error(self.creation_conflict(
                    definition,
                    external_id,
                    now_ms,
                )?));
            }
            Err(failure) => return Err(failure),
        };
        let identity = reservation.identity;
        if let Err(err) =
            self.scheduler
                .insert_workflow(&identity, &payload, Some(retention), self.limits)
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
        Ok(identity)
    }

    /// Read an exact instance in the caller's definition scope, without loading step history.
    pub fn status(
        &self,
        account: AccountId,
        definition: WorkflowId,
        instance_id: WorkflowInstanceId,
        now_ms: i64,
    ) -> Result<WorkflowStatus, PlatformError> {
        let instance = self.current_instance(account, definition, instance_id, now_ms)?;
        match instance.state {
            WorkflowState::Queued => Ok(WorkflowStatus::Queued),
            WorkflowState::Running => Ok(if instance.durable.pause_requested {
                WorkflowStatus::WaitingForPause
            } else {
                WorkflowStatus::Running
            }),
            WorkflowState::Waiting => Ok(WorkflowStatus::Waiting),
            WorkflowState::Paused => Ok(WorkflowStatus::Paused),
            WorkflowState::Terminated => Ok(WorkflowStatus::Terminated),
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

    /// Admit an event only for the caller's exact live, non-restarting instance identity.
    pub fn send_event(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        event_type: &str,
        payload: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let instance = self.current_instance(account, definition, id, now_ms)?;
        let _admission = self
            .storage
            .reserve_mutation(payload.len() as u64 + 64 * 1024)?;
        self.scheduler.send_workflow_event(
            &instance.identity,
            event_type,
            payload,
            now_ms,
            self.limits,
        )
    }

    /// Admit one due exact identity only after checking live catalog and typed artifact references.
    pub fn claim(
        &self,
        now_ms: i64,
        cursor: &mut open_compute_storage::scheduler::WorkflowClaimCursor,
    ) -> Result<Option<ClaimedWorkflowRun>, PlatformError> {
        self.storage.admission_snapshot()?.admit(64 * 1024)?;
        let repository = WorkflowRepository::new(self.storage.db());
        self.scheduler.recover_workflows(now_ms, self.limits, 32)?;
        self.scheduler
            .maintain_workflow_due(now_ms, self.limits, 32)?;
        for id in self.scheduler.due_workflows(now_ms, 32, cursor)? {
            let instance = self
                .scheduler
                .workflow_instance(id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            let identity = &instance.identity;
            let reservation = repository
                .reservation(id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            if repository.instance_operation(id)?.is_some() {
                continue;
            }
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
        self.reconcile_operations(cursor, limit, now_ms)?;
        let reservations = repository.live_reservations(cursor.control, limit)?;
        let next_control = reservations.last().map(|row| row.identity.instance_id);
        for reservation in reservations {
            let identity = &reservation.identity;
            if repository
                .instance_operation(identity.instance_id)?
                .is_some()
            {
                continue;
            }
            match self.scheduler.workflow_instance(identity.instance_id)? {
                Some(instance) => {
                    if instance.identity != *identity {
                        return self.corrupt(identity, now_ms);
                    }
                    if instance.state.is_terminal() {
                        repository.retain_instance(identity, now_ms)?;
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
            if repository.instance_operation(id)?.is_some() {
                continue;
            }
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
        self.scheduler
            .maintain_workflow_due(now_ms, self.limits, limit)?;
        self.collect_expired(limit, now_ms)?;
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
