//! Workflow create, claim eligibility, and cross-database recovery orchestration.

use open_compute_core::{
    AccountId, ErrorCode, PlatformError, ResourceAvailability, ResourceState, WorkflowId,
    WorkflowInstanceId, WorkflowOperationId, WorkflowsConfig,
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
    /// Canonical standard-base64 durable structured-clone input.
    pub payload_base64: &'a str,
    /// Explicit retention override.
    pub retention: Option<&'a open_compute_core::workflow::WorkflowRetention>,
    /// Direct-cron metadata, absent for programmatic and REST-created instances.
    pub schedule: Option<&'a open_compute_core::WorkflowCronSchedule>,
}
impl std::fmt::Debug for WorkflowCreateInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkflowCreateInput([REDACTED])")
    }
}

/// One canonical event admission request with an idempotent operation identity.
#[derive(Clone, Copy)]
pub struct WorkflowEventInput<'a> {
    /// Caller-generated idempotency identity.
    pub operation_id: WorkflowOperationId,
    /// Validated event type.
    pub event_type: &'a str,
    /// Canonical standard-base64 durable structured-clone payload.
    pub payload_base64: &'a str,
}

impl std::fmt::Debug for WorkflowEventInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowEventInput")
            .field("operation_id", &self.operation_id)
            .field("event_type", &self.event_type)
            .field("payload", &"[REDACTED]")
            .finish()
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
    /// Durable structured-clone output, decoded only in the tenant isolate.
    Complete {
        /// Canonical standard-base64 bytes, never included in diagnostics.
        #[serde(rename = "outputBase64")]
        output_base64: String,
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
    pub operation: Option<WorkflowOperationId>,
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
        operation_id: WorkflowOperationId,
        external_id: Option<&str>,
        input: WorkflowCreateInput<'_>,
        now_ms: i64,
    ) -> Result<WorkflowInstanceIdentity, PlatformError> {
        self.create_batch(
            account,
            definition,
            operation_id,
            &[(operation_id, external_id, input)],
            now_ms,
        )?
        .pop()
        .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))
    }

    /// Create a batch through atomic control reservations, one scheduler transaction, and atomic publication.
    pub fn create_batch(
        &self,
        account: AccountId,
        definition: WorkflowId,
        batch_operation_id: WorkflowOperationId,
        requests: &[(WorkflowOperationId, Option<&str>, WorkflowCreateInput<'_>)],
        now_ms: i64,
    ) -> Result<Vec<WorkflowInstanceIdentity>, PlatformError> {
        if requests.is_empty() || requests.len() > 100 {
            return Err(error(ErrorCode::WorkflowMethodUnsupported));
        }
        let mut payloads = Vec::with_capacity(requests.len());
        let mut retentions = Vec::with_capacity(requests.len());
        let mut mutation_bytes = 64 * 1024_u64;
        for (_, _, input) in requests {
            let retention = input.retention.unwrap_or(&self.limits.default_retention);
            retention.validate()?;
            let payload = open_compute_core::workflow::durable_value_base64(
                input.payload_base64,
                ErrorCode::WorkflowPayloadTooLarge,
            )?;
            mutation_bytes = mutation_bytes
                .checked_add(
                    u64::try_from(payload.len())
                        .map_err(|_| error(ErrorCode::WorkflowStateQuotaExceeded))?,
                )
                .ok_or_else(|| error(ErrorCode::WorkflowStateQuotaExceeded))?;
            payloads.push(payload);
            retentions.push(retention.clone());
        }
        self.scheduler.check_workflow_create_batch_capacity(
            account,
            &payloads.iter().map(String::len).collect::<Vec<_>>(),
            self.limits,
        )?;
        let _reservation = self.storage.reserve_mutation(mutation_bytes)?;
        let repository = WorkflowRepository::new(self.storage.db());
        let reservation_requests = requests
            .iter()
            .map(|(operation, external, input)| (*operation, *external, input.schedule))
            .collect::<Vec<_>>();
        let reservations = match repository.reserve_instances_with_schedules(
            account,
            definition,
            batch_operation_id,
            &reservation_requests,
            self.limits,
            now_ms,
        ) {
            Ok(reservations) => reservations,
            Err(failure) if failure.code() == ErrorCode::WorkflowInstanceAlreadyExists => {
                if requests.len() == 1 {
                    return Err(error(self.creation_conflict(
                        definition,
                        requests[0].1,
                        now_ms,
                    )?));
                }
                return Err(failure);
            }
            Err(failure) => return Err(failure),
        };
        let identities = reservations
            .into_iter()
            .map(|reservation| reservation.identity)
            .collect::<Vec<_>>();
        let scheduler_requests = identities
            .iter()
            .zip(&payloads)
            .zip(&retentions)
            .map(|((identity, payload), retention)| (identity, payload.as_str(), Some(retention)))
            .collect::<Vec<_>>();
        if let Err(failure) = self
            .scheduler
            .insert_workflows(&scheduler_requests, self.limits)
        {
            let mut committed = 0_usize;
            for ((identity, payload), retention) in
                identities.iter().zip(&payloads).zip(&retentions)
            {
                if let Some(existing) = self.scheduler.workflow_instance(identity.instance_id)? {
                    if existing.identity != *identity
                        || existing.input_json != *payload
                        || existing.durable.retention != *retention
                    {
                        return Err(error(ErrorCode::WorkflowInvariantViolation));
                    }
                    committed += 1;
                }
            }
            if committed == 0 {
                let removed = repository.abandon_creations(&identities)?;
                if removed != identities.len() {
                    return Err(error(ErrorCode::WorkflowInvariantViolation));
                }
                return Err(failure);
            }
            if committed != identities.len() {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
        repository.finalize_instances(&identities, now_ms)?;
        self.scheduler.wake_signal().notify();
        Ok(identities)
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
                output_base64: open_compute_core::workflow::durable_value_base64(
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
        event: WorkflowEventInput<'_>,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let instance = self.current_instance(account, definition, id, now_ms)?;
        let _admission = self
            .storage
            .reserve_mutation(event.payload_base64.len() as u64 + 64 * 1024)?;
        self.scheduler.send_workflow_event(
            &instance.identity,
            event.operation_id,
            event.event_type,
            event.payload_base64,
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
        let mut recovered_creation_batches = std::collections::HashSet::new();
        for reservation in reservations {
            let identity = &reservation.identity;
            if repository
                .instance_operation(identity.instance_id)?
                .is_some()
            {
                continue;
            }
            if reservation.state == WorkflowRefState::Creating {
                if !recovered_creation_batches.insert(identity.creation_batch_id) {
                    continue;
                }
                self.reconcile_creation_batch(repository, identity.creation_batch_id, now_ms)?;
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
                        if reservation.state != WorkflowRefState::Live {
                            return self.corrupt(identity, now_ms);
                        }
                        repository.repair_instance_referrers(identity)?;
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

    fn reconcile_creation_batch(
        &self,
        repository: WorkflowRepository<'_>,
        batch: WorkflowOperationId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let reservations = repository.creation_batch_reservations(batch)?;
        if reservations.is_empty()
            || reservations
                .iter()
                .any(|row| row.identity.creation_batch_id != batch)
        {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        let identities = reservations
            .iter()
            .map(|row| row.identity.clone())
            .collect::<Vec<_>>();
        let mut committed = 0_usize;
        for reservation in &reservations {
            let Some(instance) = self
                .scheduler
                .workflow_instance(reservation.identity.instance_id)?
            else {
                continue;
            };
            if instance.identity != reservation.identity || instance.state != WorkflowState::Queued
            {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
            committed += 1;
        }
        if committed == identities.len() {
            for identity in &identities {
                repository.repair_instance_referrers(identity)?;
            }
            return repository.finalize_instances(&identities, now_ms);
        }
        if committed != 0 {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        let oldest = identities
            .iter()
            .map(|identity| identity.created_at_ms)
            .min()
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        if now_ms.saturating_sub(oldest)
            >= i64::try_from(self.limits.creation_grace_ms)
                .map_err(|_| error(ErrorCode::LimitInvalid))?
        {
            let removed = repository.abandon_creations(&identities)?;
            if removed != identities.len() {
                return Err(error(ErrorCode::WorkflowInvariantViolation));
            }
        }
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
