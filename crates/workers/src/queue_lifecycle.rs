//! Queue lifecycle orchestration across control and scheduler SQLite authorities.

use open_compute_core::{AccountId, ErrorCode, PlatformError, QueueId, RequestId};
use open_compute_storage::{
    PlatformStorage, QueueAvailability, QueueConfig, QueueCreateReservation, QueueMetrics,
    QueueProjection, QueueRecord, QueueRepository, QueueState, SchedulerStore, WorkerRepository,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const PURGE_ROWS: u32 = 256;
const PURGE_BYTES: u64 = 4 * 1024 * 1024;

/// Create Queue control request.
#[derive(Clone, Debug)]
pub struct CreateQueueRequest {
    /// Account boundary.
    pub account_id: AccountId,
    /// Mutable display name.
    pub name: String,
    /// Queue behavior and safety config.
    pub config: QueueConfig,
    /// Required control idempotency key.
    pub idempotency_key: String,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Current wall-clock milliseconds.
    pub now_ms: i64,
}

/// Completed Queue create response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateQueueResult {
    /// Ready Queue authority.
    pub queue: QueueRecord,
}

/// New create result or exact persisted idempotent response bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateQueueOutcome {
    /// A new Queue reached ready/healthy.
    Applied(CreateQueueResult),
    /// Same operation already completed.
    Replay(Vec<u8>),
}

/// Completed Queue delete response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQueueResult {
    /// Immutable retired Queue identity.
    pub queue: QueueRecord,
    /// Messages physically purged by this delete.
    pub purged_messages: u64,
    /// Serialized body bytes physically purged by this delete.
    pub purged_bytes: u64,
}

/// Queue lifecycle owner over the two SQLite authorities.
#[derive(Debug)]
pub struct QueueController<'a> {
    storage: &'a PlatformStorage,
    scheduler: Arc<SchedulerStore>,
}

impl<'a> QueueController<'a> {
    /// Bind control authority and the independently owned scheduler store.
    #[must_use]
    pub const fn new(storage: &'a PlatformStorage, scheduler: Arc<SchedulerStore>) -> Self {
        Self { storage, scheduler }
    }

    /// Boundedly converge durable creating/configuring/deleting Queue intent after restart.
    pub fn reconcile_pending(&self, limit: u32, now_ms: i64) -> Result<u32, PlatformError> {
        let repository = QueueRepository::new(self.storage.db());
        let queues = repository.list_reconcile(limit)?;
        let mut reconciled = 0_u32;
        for queue in queues {
            match (queue.state, queue.availability_code.as_deref()) {
                (QueueState::Creating, Some("QUEUE_PROJECTION_PENDING")) => {
                    let projection = projection(&queue);
                    self.scheduler.ensure_queue_projection(&projection)?;
                    let ready = repository.mark_ready(queue.account_id, queue.id, now_ms)?;
                    let response = serde_json::to_vec(&CreateQueueResult {
                        queue: ready.clone(),
                    })
                    .map_err(|_| invariant())?;
                    repository.complete_reconciled_create(&ready, &response)?;
                }
                (QueueState::Ready, Some("QUEUE_CONFIG_PENDING")) => {
                    let projection = projection(&queue);
                    self.scheduler.reconcile_queue_config(&projection)?;
                    let healthy = repository.mark_config_healthy(
                        queue.account_id,
                        queue.id,
                        queue.config_generation,
                        RequestId::generate(),
                        now_ms,
                    )?;
                    self.scheduler.finish_queue_config(
                        queue.id,
                        healthy.lifecycle_generation,
                        healthy.config_generation,
                        now_ms,
                    )?;
                }
                (QueueState::Deleting, Some("QUEUE_DELETE_PENDING")) => {
                    self.scheduler.fence_queue_delete(
                        queue.id,
                        queue.lifecycle_generation,
                        now_ms,
                    )?;
                    loop {
                        let batch =
                            self.scheduler
                                .purge_queue(queue.id, PURGE_ROWS, PURGE_BYTES)?;
                        if !batch.expired_remaining {
                            break;
                        }
                    }
                    self.scheduler
                        .delete_queue_projection(queue.id, queue.lifecycle_generation)?;
                    repository.mark_tombstoned(
                        queue.account_id,
                        queue.id,
                        RequestId::generate(),
                        now_ms,
                    )?;
                }
                (QueueState::Tombstoned, _) => {}
                (_, _) if queue.availability == QueueAvailability::Unavailable => {
                    return Err(PlatformError::new(
                        ErrorCode::QueueStorageUnavailable,
                        "Queue reconciliation encountered unavailable authority",
                    ));
                }
                _ => return Err(invariant()),
            }
            reconciled = reconciled.checked_add(1).ok_or_else(invariant)?;
        }
        Ok(reconciled)
    }

    /// Create, project, and expose one Queue under an idempotency reservation.
    pub fn create(
        &self,
        request: &CreateQueueRequest,
    ) -> Result<CreateQueueOutcome, PlatformError> {
        validate_key(&request.idempotency_key)?;
        request.config.validate()?;
        let fingerprint = self
            .storage
            .crypto()
            .fingerprint_request(&create_fingerprint(request));
        let queue_id = QueueId::generate();
        let repository = QueueRepository::new(self.storage.db());
        let queue = match repository.reserve_create(
            request.account_id,
            queue_id,
            &request.name,
            request.config,
            &request.idempotency_key,
            self.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            request.now_ms,
            request.now_ms.saturating_add(IDEMPOTENCY_TTL_MS),
            self.storage.hardening().max_resources_per_kind_per_account,
        )? {
            QueueCreateReservation::Complete(bytes) => {
                return Ok(CreateQueueOutcome::Replay(bytes));
            }
            QueueCreateReservation::Running => {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Queue create is still reconciling",
                ));
            }
            QueueCreateReservation::Failed(bytes) => return Err(replayed_failure(&bytes)),
            QueueCreateReservation::Reserved(queue) => queue,
        };
        let workers = WorkerRepository::new(self.storage.db());
        let result = self.create_reserved(request, &queue);
        match result {
            Ok(result) => {
                let response = serde_json::to_vec(&result).map_err(|_| invariant())?;
                workers.complete_idempotency_with_queue_ref(
                    request.account_id,
                    "queue.create",
                    &request.idempotency_key,
                    &fingerprint,
                    &response,
                    result.queue.id,
                )?;
                Ok(CreateQueueOutcome::Applied(result))
            }
            Err(error) => Err(error),
        }
    }

    fn create_reserved(
        &self,
        request: &CreateQueueRequest,
        queue: &QueueRecord,
    ) -> Result<CreateQueueResult, PlatformError> {
        let projection = projection(queue);
        self.scheduler.create_queue_projection(&projection)?;
        self.scheduler.verify_queue_projection(&projection)?;
        let queue = QueueRepository::new(self.storage.db()).mark_ready(
            request.account_id,
            queue.id,
            request.now_ms,
        )?;
        Ok(CreateQueueResult { queue })
    }

    /// Rename a Queue without changing scheduler state or either generation.
    pub fn rename(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        name: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        QueueRepository::new(self.storage.db())
            .rename(account_id, queue_id, name, request_id, now_ms)
    }

    /// Apply the five-step no-stale-config projection protocol.
    pub fn update_config(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        expected_config_generation: u64,
        config: QueueConfig,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        let repository = QueueRepository::new(self.storage.db());
        let current = repository.get(account_id, queue_id)?;
        if current.config_generation != expected_config_generation {
            return Err(PlatformError::new(
                ErrorCode::QueueConfigPending,
                "Queue config generation is stale",
            ));
        }
        self.scheduler.begin_queue_config(
            queue_id,
            current.lifecycle_generation,
            expected_config_generation,
            now_ms,
        )?;
        let pending = repository.write_config_pending(
            account_id,
            queue_id,
            expected_config_generation,
            config,
            now_ms,
        )?;
        self.scheduler.project_queue_config(&projection(&pending))?;
        let healthy = repository.mark_config_healthy(
            account_id,
            queue_id,
            pending.config_generation,
            request_id,
            now_ms,
        )?;
        self.scheduler.finish_queue_config(
            queue_id,
            healthy.lifecycle_generation,
            healthy.config_generation,
            now_ms,
        )?;
        Ok(healthy)
    }

    /// Delete an unreferenced Queue, requiring explicit force for a non-empty backlog.
    pub fn delete(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        expected_lifecycle_generation: u64,
        force: bool,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<DeleteQueueResult, PlatformError> {
        let repository = QueueRepository::new(self.storage.db());
        let queue = repository.get(account_id, queue_id)?;
        let metrics = self.scheduler.queue_metrics(
            queue_id,
            expected_lifecycle_generation,
            queue.config_generation,
        )?;
        if !force && metrics.backlog_count != 0 {
            return Err(PlatformError::new(
                ErrorCode::QueueNotEmpty,
                "Queue backlog is non-empty; explicit force is required",
            ));
        }
        let deleting =
            repository.begin_delete(account_id, queue_id, expected_lifecycle_generation, now_ms)?;
        let fenced =
            self.scheduler
                .fence_queue_delete(queue_id, deleting.lifecycle_generation, now_ms)?;
        if !force && fenced.backlog_count != 0 {
            return Err(invariant());
        }
        let (mut purged_messages, mut purged_bytes) = (0_u64, 0_u64);
        if force {
            loop {
                let batch = self
                    .scheduler
                    .purge_queue(queue_id, PURGE_ROWS, PURGE_BYTES)?;
                purged_messages = purged_messages
                    .checked_add(batch.messages)
                    .ok_or_else(invariant)?;
                purged_bytes = purged_bytes
                    .checked_add(batch.bytes)
                    .ok_or_else(invariant)?;
                if !batch.expired_remaining {
                    break;
                }
            }
        }
        self.scheduler
            .delete_queue_projection(queue_id, deleting.lifecycle_generation)?;
        let queue = repository.mark_tombstoned(account_id, queue_id, request_id, now_ms)?;
        Ok(DeleteQueueResult {
            queue,
            purged_messages,
            purged_bytes,
        })
    }

    /// Read current durable metrics after verifying both Queue generations.
    pub fn metrics(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
    ) -> Result<QueueMetrics, PlatformError> {
        let queue = QueueRepository::new(self.storage.db()).get(account_id, queue_id)?;
        self.scheduler.queue_metrics(
            queue_id,
            queue.lifecycle_generation,
            queue.config_generation,
        )
    }
}

fn projection(queue: &QueueRecord) -> QueueProjection {
    QueueProjection {
        queue_id: queue.id,
        account_id: queue.account_id,
        lifecycle_generation: queue.lifecycle_generation,
        config_generation: queue.config_generation,
        config: queue.config,
        created_at_ms: queue.created_at_ms,
        updated_at_ms: queue.updated_at_ms,
    }
}

fn create_fingerprint(request: &CreateQueueRequest) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/queue-create/v1\0");
    digest.update(request.account_id.as_uuid().as_bytes());
    digest.update((request.name.len() as u64).to_be_bytes());
    digest.update(request.name.as_bytes());
    digest.update(request.config.delivery_delay_seconds.to_be_bytes());
    digest.update(request.config.retention_seconds.to_be_bytes());
    digest.update(request.config.max_message_bytes.to_be_bytes());
    digest.update(request.config.max_batch_messages.to_be_bytes());
    digest.update(request.config.max_batch_bytes.to_be_bytes());
    digest.update(request.config.max_backlog_bytes.to_be_bytes());
    digest.finalize().into()
}

fn validate_key(key: &str) -> Result<(), PlatformError> {
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "Queue idempotency key is invalid",
        ));
    }
    Ok(())
}

fn replayed_failure(bytes: &[u8]) -> PlatformError {
    let code = serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|value| value.get("code")?.as_str().map(ToOwned::to_owned));
    let code = match code.as_deref() {
        Some("QUEUE_NAME_CONFLICT") => ErrorCode::QueueNameConflict,
        Some("QUEUE_NOT_READY") => ErrorCode::QueueNotReady,
        Some("QUEUE_CONFIG_PENDING") => ErrorCode::QueueConfigPending,
        Some("QUEUE_REFERENCED") => ErrorCode::QueueReferenced,
        Some("QUEUE_NOT_EMPTY") => ErrorCode::QueueNotEmpty,
        _ => ErrorCode::Internal,
    };
    PlatformError::new(code, "idempotent Queue operation previously failed")
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue lifecycle invariant failed",
    )
}

#[cfg(test)]
#[path = "queue_lifecycle_tests.rs"]
mod tests;
