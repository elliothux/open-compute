//! Lifecycle admission binds every caller operation to its exact current scheduler generation.

use super::*;
use open_compute_core::WorkflowOperationId;
use open_compute_core::workflow::WorkflowRestartSelector;
use open_compute_storage::scheduler::{WorkflowInstanceAction, WorkflowInstanceRecord};
use open_compute_storage::{WorkflowOperation, WorkflowOperationKind, WorkflowOperationResult};

impl WorkflowController<'_> {
    /// Inspect an account-scoped, logically live instance without returning its payload or fences.
    pub fn inspect(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        now_ms: i64,
    ) -> Result<open_compute_storage::scheduler::WorkflowInstanceInspection, PlatformError> {
        self.current_instance(account, definition, id, now_ms)?;
        self.scheduler
            .inspect_workflow_instance(id, now_ms)?
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))
    }

    /// Restart the same immutable version under a fresh execution generation.
    /// The system host supplies a distinct operation UUID for each external invocation.
    pub fn restart(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        operation_id: WorkflowOperationId,
        restart_from: Option<WorkflowRestartSelector>,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if let Some(selector) = &restart_from {
            selector.validate()?;
        }
        let _admission = self.storage.reserve_mutation(64 * 1024)?;
        let repository = WorkflowRepository::new(self.storage.db());
        repository.definition(account, definition)?;
        let reservation = repository
            .reservation(id)?
            .filter(|row| {
                row.identity.target.account_id == account
                    && row.identity.target.definition_id == definition
            })
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        if let Some(operation) = repository.instance_operation(id)? {
            if operation.kind() != WorkflowOperationKind::Restart
                || operation.id() != operation_id
                || operation.restart_from() != restart_from.as_ref()
            {
                return Err(error(ErrorCode::WorkflowInstanceBusy));
            }
            if let Some(code) = self.finish_operation(&operation, now_ms)? {
                return Err(error(code));
            }
            self.scheduler.wake_signal().notify();
            return Ok(());
        }
        if self
            .scheduler
            .workflow_restart_matches(id, operation_id, restart_from.as_ref())?
        {
            return Ok(());
        }
        let instance = self.current_instance(account, definition, id, now_ms)?;
        if instance.identity != reservation.identity {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        let operation = repository.prepare_restart_operation(
            &instance.identity,
            operation_id,
            restart_from,
            self.limits,
            now_ms,
        )?;
        if let Some(code) = self.finish_operation(&operation, now_ms)? {
            return Err(error(code));
        }
        self.scheduler.wake_signal().notify();
        Ok(())
    }

    /// Delete one instance through the same purge saga used by retention GC.
    pub fn delete(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        operation_id: WorkflowOperationId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let mut instance = self.current_instance(account, definition, id, now_ms)?;
        let _admission = self.storage.reserve_mutation(64 * 1024)?;
        if !instance.state.is_terminal() {
            self.scheduler.modify_workflow(
                &instance.identity,
                WorkflowInstanceAction::Terminate,
                now_ms,
                self.limits,
            )?;
            instance = self
                .scheduler
                .workflow_instance(id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        }
        let repository = WorkflowRepository::new(self.storage.db());
        if repository
            .reservation(id)?
            .is_some_and(|reservation| reservation.state == WorkflowRefState::Live)
        {
            repository.retain_instance(&instance.identity, now_ms)?;
        }
        let operation = repository.prepare_instance_operation(
            &instance.identity,
            operation_id,
            WorkflowOperationKind::Purge,
            self.limits,
            now_ms,
        )?;
        if let Some(code) = self.finish_operation(&operation, now_ms)? {
            return Err(error(code));
        }
        Ok(())
    }

    /// Admit pause/resume/terminate without exposing private execution identity to the caller.
    pub fn modify(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        action: WorkflowInstanceAction,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let instance = self.current_instance(account, definition, id, now_ms)?;
        let _admission = self.storage.reserve_mutation(64 * 1024)?;
        self.scheduler
            .modify_workflow(&instance.identity, action, now_ms, self.limits)?;
        if action == WorkflowInstanceAction::Terminate {
            WorkflowRepository::new(self.storage.db())
                .retain_instance(&instance.identity, now_ms)?;
        }
        Ok(())
    }

    /// Fence normal execution and durably run registered rollback handlers before termination.
    pub fn rollback(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let instance = self.current_instance(account, definition, id, now_ms)?;
        let _admission = self.storage.reserve_mutation(64 * 1024)?;
        self.scheduler
            .request_workflow_rollback(&instance.identity, now_ms, self.limits)
    }

    pub(super) fn current_instance(
        &self,
        account: AccountId,
        definition: WorkflowId,
        id: WorkflowInstanceId,
        now_ms: i64,
    ) -> Result<WorkflowInstanceRecord, PlatformError> {
        let repository = WorkflowRepository::new(self.storage.db());
        repository.definition(account, definition)?;
        let reservation = repository
            .reservation(id)?
            .filter(|row| {
                row.identity.target.account_id == account
                    && row.identity.target.definition_id == definition
            })
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        if let Some(operation) = repository.instance_operation(id)? {
            return Err(error(if operation.kind() == WorkflowOperationKind::Purge {
                ErrorCode::WorkflowInstanceNotFound
            } else {
                ErrorCode::WorkflowInstanceBusy
            }));
        }
        if matches!(
            reservation.state,
            WorkflowRefState::Creating | WorkflowRefState::Restarting | WorkflowRefState::Releasing
        ) {
            return Err(error(ErrorCode::WorkflowInstanceBusy));
        }
        let instance = self
            .scheduler
            .workflow_instance(id)?
            .ok_or_else(|| error(ErrorCode::WorkflowInstanceNotFound))?;
        if instance.identity != reservation.identity {
            return Err(error(ErrorCode::WorkflowInvariantViolation));
        }
        if instance
            .durable
            .expires_at_ms
            .is_some_and(|expiry| expiry <= now_ms)
        {
            return Err(error(ErrorCode::WorkflowInstanceNotFound));
        }
        Ok(instance)
    }

    pub(super) fn creation_conflict(
        &self,
        definition: WorkflowId,
        external: Option<&str>,
        now_ms: i64,
    ) -> Result<ErrorCode, PlatformError> {
        let Some(external) = external else {
            return Ok(ErrorCode::WorkflowInstanceAlreadyExists);
        };
        let repository = WorkflowRepository::new(self.storage.db());
        let existing = repository.find_instance(definition, external)?;
        if repository
            .instance_operation(existing.identity.instance_id)?
            .is_some_and(|operation| operation.kind() == WorkflowOperationKind::Purge)
        {
            return Ok(ErrorCode::WorkflowInstanceCleanupPending);
        }
        let instance = self
            .scheduler
            .workflow_instance(existing.identity.instance_id)?
            .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
        if instance
            .durable
            .expires_at_ms
            .is_some_and(|expiry| expiry <= now_ms)
        {
            return Ok(ErrorCode::WorkflowInstanceCleanupPending);
        }
        Ok(ErrorCode::WorkflowInstanceAlreadyExists)
    }

    pub(super) fn reconcile_operations(
        &self,
        cursor: &mut WorkflowReconcileCursor,
        limit: u32,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let repository = WorkflowRepository::new(self.storage.db());
        let operations = repository.instance_operations(cursor.operation, limit)?;
        let next = operations.last().map(WorkflowOperation::id);
        for operation in operations {
            self.finish_operation(&operation, now_ms)?;
        }
        cursor.operation = next;
        let receipts = self
            .scheduler
            .workflow_gc_receipts(cursor.gc_receipt, limit)?;
        let next = receipts
            .last()
            .map(open_compute_storage::WorkflowGcReceipt::instance_id);
        for receipt in receipts {
            match repository.acknowledge_workflow_gc(&receipt) {
                Ok(proof) => self.scheduler.sweep_workflow_gc(&proof)?,
                Err(failure) if failure.code() == ErrorCode::WorkflowInstanceBusy => {}
                Err(failure) => return Err(failure),
            }
        }
        cursor.gc_receipt = next;
        Ok(())
    }

    pub(super) fn collect_expired(&self, limit: u32, now_ms: i64) -> Result<(), PlatformError> {
        let repository = WorkflowRepository::new(self.storage.db());
        for identity in self.scheduler.expired_workflows(now_ms, limit)? {
            if repository
                .instance_operation(identity.instance_id)?
                .is_some()
            {
                continue;
            }
            let reservation = repository
                .reservation(identity.instance_id)?
                .ok_or_else(|| error(ErrorCode::WorkflowInvariantViolation))?;
            if reservation.identity != identity {
                return self.corrupt(&identity, now_ms);
            }
            if reservation.state == WorkflowRefState::Live {
                repository.retain_instance(&identity, now_ms)?;
            }
            let operation = repository.prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Purge,
                self.limits,
                now_ms,
            )?;
            self.finish_operation(&operation, now_ms)?;
        }
        Ok(())
    }

    fn finish_operation(
        &self,
        operation: &WorkflowOperation,
        now_ms: i64,
    ) -> Result<Option<ErrorCode>, PlatformError> {
        let repository = WorkflowRepository::new(self.storage.db());
        match self
            .scheduler
            .apply_workflow_operation(operation, now_ms, self.limits)?
        {
            WorkflowOperationResult::Applied(proof) => {
                repository.complete_instance_operation(&proof, now_ms)?;
                Ok(None)
            }
            WorkflowOperationResult::Rejected(proof) => {
                repository.cancel_instance_operation(&proof, now_ms)?;
                Ok(Some(proof.code()))
            }
        }
    }
}
