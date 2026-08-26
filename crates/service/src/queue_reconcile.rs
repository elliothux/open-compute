//! Restart reconciliation for idempotent Queue control mutations.

use super::{
    MutationOutcome, PatchQueueBody, QueueMutationIntent, complete_mutation, internal,
    is_final_mutation_failure, now_ms, persist_failure,
};
use open_compute_core::{ErrorCode, PlatformError, RequestId};
use open_compute_storage::{
    PlatformStorage, QueueAvailability, QueueConfig, QueueProjection, QueueRepository, QueueState,
    RunningQueueMutation, SchedulerStore,
};
use open_compute_workers::{DeleteQueueResult, QueueController};
use std::sync::Arc;

pub(super) fn reconcile_running_mutations(
    storage: &PlatformStorage,
    scheduler: &Arc<SchedulerStore>,
    limit: u32,
) -> Result<u32, PlatformError> {
    let mutations = QueueRepository::new(storage.db()).list_running_mutations(limit)?;
    let mut reconciled = 0_u32;
    for mutation in mutations {
        if let Err(error) = resume_running_mutation(storage, scheduler.clone(), &mutation) {
            if !is_final_mutation_failure(error.code()) {
                return Err(error);
            }
            persist_failure(
                storage,
                mutation.account_id,
                &mutation.scope,
                &mutation.idempotency_key,
                &mutation.request_fingerprint,
                error.code(),
            )?;
        }
        reconciled = reconciled.saturating_add(1);
    }
    Ok(reconciled)
}

pub(super) fn resume_running_mutation(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    mutation: &RunningQueueMutation,
) -> Result<MutationOutcome, PlatformError> {
    let intent: QueueMutationIntent =
        serde_json::from_slice(&mutation.intent_json).map_err(|_| internal())?;
    let expected_scope = match &intent {
        QueueMutationIntent::Patch { version: 1, .. } => {
            format!("queue.patch:{}", mutation.queue_id)
        }
        QueueMutationIntent::Delete { version: 1, .. } => {
            format!("queue.delete:{}", mutation.queue_id)
        }
        _ => return Err(internal()),
    };
    if mutation.scope != expected_scope {
        return Err(internal());
    }
    match intent {
        QueueMutationIntent::Patch {
            request_id, body, ..
        } => resume_patch(storage, scheduler, mutation, request_id, &body),
        QueueMutationIntent::Delete {
            request_id,
            expected_lifecycle_generation,
            force,
            purged_messages,
            purged_bytes,
            ..
        } => {
            let purge = match (purged_messages, purged_bytes) {
                (Some(messages), Some(bytes)) => Some((messages, bytes)),
                (None, None) => None,
                _ => return Err(internal()),
            };
            resume_delete(
                storage,
                scheduler,
                mutation,
                request_id,
                expected_lifecycle_generation,
                force,
                purge,
            )
        }
    }
}

fn resume_patch(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    mutation: &RunningQueueMutation,
    request_id: RequestId,
    body: &PatchQueueBody,
) -> Result<MutationOutcome, PlatformError> {
    let repository = QueueRepository::new(storage.db());
    let current = repository.get(mutation.account_id, mutation.queue_id)?;
    let controller = QueueController::new(storage, scheduler.clone());
    let queue = if let Some(name) = &body.name {
        if current.config_generation != body.expected_config_generation {
            return Err(stale_config());
        }
        if current.name == *name {
            current
        } else {
            controller.rename(
                mutation.account_id,
                mutation.queue_id,
                name,
                request_id,
                now_ms(),
            )?
        }
    } else {
        resume_config_patch(storage, scheduler, mutation, request_id, body, current)?
    };
    complete_mutation(
        storage,
        mutation.account_id,
        &mutation.scope,
        &mutation.idempotency_key,
        &mutation.request_fingerprint,
        mutation.queue_id,
        &serde_json::json!({ "queue": queue }),
    )
}

fn resume_config_patch(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    mutation: &RunningQueueMutation,
    request_id: RequestId,
    body: &PatchQueueBody,
    current: open_compute_storage::QueueRecord,
) -> Result<open_compute_storage::QueueRecord, PlatformError> {
    let next = body
        .expected_config_generation
        .checked_add(1)
        .ok_or_else(internal)?;
    if current.config_generation == body.expected_config_generation {
        let mut config = current.config;
        apply_config_patch(&mut config, body);
        return QueueController::new(storage, scheduler).update_config(
            mutation.account_id,
            mutation.queue_id,
            body.expected_config_generation,
            config,
            request_id,
            now_ms(),
        );
    }
    if current.config_generation != next || !config_patch_matches(current.config, body) {
        return Err(stale_config());
    }
    let projection = QueueProjection {
        queue_id: current.id,
        account_id: current.account_id,
        lifecycle_generation: current.lifecycle_generation,
        config_generation: current.config_generation,
        config: current.config,
        created_at_ms: current.created_at_ms,
        updated_at_ms: current.updated_at_ms,
    };
    scheduler.reconcile_queue_config(&projection)?;
    let healthy = match (current.availability, current.availability_code.as_deref()) {
        (QueueAvailability::Healthy, None) => current,
        (QueueAvailability::Degraded, Some("QUEUE_CONFIG_PENDING")) => {
            QueueRepository::new(storage.db()).mark_config_healthy(
                mutation.account_id,
                mutation.queue_id,
                next,
                request_id,
                now_ms(),
            )?
        }
        _ => return Err(internal()),
    };
    scheduler.finish_queue_config(
        mutation.queue_id,
        healthy.lifecycle_generation,
        healthy.config_generation,
        now_ms(),
    )?;
    Ok(healthy)
}

fn resume_delete(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    mutation: &RunningQueueMutation,
    request_id: RequestId,
    expected_lifecycle_generation: u64,
    force: bool,
    purge: Option<(u64, u64)>,
) -> Result<MutationOutcome, PlatformError> {
    let repository = QueueRepository::new(storage.db());
    let controller = QueueController::new(storage, scheduler);
    let (purged_messages, purged_bytes) = if let Some(purge) = purge {
        purge
    } else {
        let metrics = controller.metrics(mutation.account_id, mutation.queue_id)?;
        if !force && metrics.backlog_count != 0 {
            return Err(PlatformError::new(
                ErrorCode::QueueNotEmpty,
                "Queue backlog is non-empty; explicit force is required",
            ));
        }
        let intent = QueueMutationIntent::Delete {
            version: 1,
            request_id,
            expected_lifecycle_generation,
            force,
            purged_messages: Some(metrics.backlog_count),
            purged_bytes: Some(metrics.backlog_bytes),
        };
        let json = serde_json::to_vec(&intent).map_err(|_| internal())?;
        repository.replace_mutation_intent(mutation, &json)?;
        (metrics.backlog_count, metrics.backlog_bytes)
    };
    let mut queue = repository.get(mutation.account_id, mutation.queue_id)?;
    match queue.state {
        QueueState::Ready => {
            queue = controller
                .delete(
                    mutation.account_id,
                    mutation.queue_id,
                    expected_lifecycle_generation,
                    force,
                    request_id,
                    now_ms(),
                )?
                .queue;
        }
        QueueState::Deleting => {
            controller.reconcile_pending(256, now_ms())?;
            queue = repository.get(mutation.account_id, mutation.queue_id)?;
        }
        QueueState::Tombstoned => {}
        QueueState::Creating => return Err(internal()),
    }
    if queue.state != QueueState::Tombstoned {
        return Err(internal());
    }
    complete_mutation(
        storage,
        mutation.account_id,
        &mutation.scope,
        &mutation.idempotency_key,
        &mutation.request_fingerprint,
        mutation.queue_id,
        &DeleteQueueResult {
            queue,
            purged_messages,
            purged_bytes,
        },
    )
}

fn apply_config_patch(config: &mut QueueConfig, body: &PatchQueueBody) {
    if let Some(value) = body.delivery_delay_seconds {
        config.delivery_delay_seconds = value;
    }
    if let Some(value) = body.retention_seconds {
        config.retention_seconds = value;
    }
    if let Some(value) = body.max_backlog_bytes {
        config.max_backlog_bytes = value;
    }
}

fn config_patch_matches(config: QueueConfig, body: &PatchQueueBody) -> bool {
    body.delivery_delay_seconds
        .is_none_or(|value| config.delivery_delay_seconds == value)
        && body
            .retention_seconds
            .is_none_or(|value| config.retention_seconds == value)
        && body
            .max_backlog_bytes
            .is_none_or(|value| config.max_backlog_bytes == value)
}

fn stale_config() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueConfigPending,
        "Queue config generation is stale",
    )
}
