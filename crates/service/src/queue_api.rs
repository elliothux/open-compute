//! Transport-neutral Queue catalog and reconciliation composition.

use crate::metrics::{MetricsRegistry, QueueReconcileOperation};
use crate::scheduler::SchedulerService;
use open_compute_core::{
    AccountId, ErrorCode, PlatformError, QueueConsumerId, QueueId, RequestId, WorkerId,
};
use open_compute_storage::{
    NewQueueConsumerDeclaration, PlatformStorage, QUEUE_DEFAULT_MAX_BACKLOG_BYTES,
    QueueAvailability, QueueConsumerConfig, QueueConsumerRecord, QueueConsumerRepository,
    QueueConsumerState, QueueRepository, QueueState, WorkerRepository,
};
use open_compute_workers::QueueController;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Queue lifecycle authority shared by public v4 and runtime composition.
#[derive(Clone, Debug)]
pub struct QueueApiState {
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerService>,
    metrics: Option<Arc<MetricsRegistry>>,
    default_max_backlog_bytes: u64,
    max_consumer_concurrency: u32,
}

impl QueueApiState {
    /// Bind control, scheduler, and consumer lifecycle authority.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        scheduler: Arc<SchedulerService>,
        max_consumer_concurrency: u32,
    ) -> Self {
        Self {
            storage,
            scheduler,
            metrics: None,
            default_max_backlog_bytes: QUEUE_DEFAULT_MAX_BACKLOG_BYTES,
            max_consumer_concurrency,
        }
    }

    /// Attach fixed low-cardinality reconciliation metrics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the installation Queue backlog default.
    #[must_use]
    pub fn with_default_max_backlog_bytes(mut self, bytes: u64) -> Self {
        self.default_max_backlog_bytes = bytes;
        self
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) fn scheduler(&self) -> &Arc<open_compute_storage::SchedulerStore> {
        self.scheduler.store()
    }

    pub(crate) const fn default_max_backlog_bytes(&self) -> u64 {
        self.default_max_backlog_bytes
    }

    pub(crate) const fn max_consumer_concurrency(&self) -> u32 {
        self.max_consumer_concurrency
    }

    pub(crate) fn set_delivery_paused(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        paused: bool,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<open_compute_storage::QueueRecord, PlatformError> {
        let queue = QueueRepository::new(self.storage.db())
            .set_delivery_paused(account_id, queue_id, paused, now_ms)?;
        if let Some(record) =
            QueueConsumerRepository::new(self.storage.db()).live_for_queue(queue_id)?
        {
            match (paused, record.state) {
                (true, QueueConsumerState::Active) => {
                    self.scheduler.pause_queue_consumer_operator(
                        record.id,
                        record.consumer_generation,
                        request_id,
                    )?;
                }
                (false, QueueConsumerState::Paused) => {
                    self.scheduler.resume_queue_consumer_operator(
                        record.id,
                        record.consumer_generation,
                        request_id,
                    )?;
                }
                _ => {}
            }
        }
        Ok(queue)
    }

    /// Converge a bounded startup batch before producer traffic is exposed.
    pub async fn reconcile_pending(&self) -> Result<u32, PlatformError> {
        let storage = self.storage.clone();
        let scheduler = self.scheduler.store().clone();
        let now = now_ms()?;
        let (pending, result) = tokio::task::spawn_blocking(move || {
            let pending = QueueRepository::new(storage.db()).list_reconcile(256)?;
            let result = QueueController::new(&storage, scheduler).reconcile_pending(256, now);
            Ok::<_, PlatformError>((pending, result))
        })
        .await
        .map_err(|_| internal())??;
        if let Some(metrics) = &self.metrics {
            let success = result.is_ok();
            for queue in pending {
                let operation = match queue.state {
                    QueueState::Creating => QueueReconcileOperation::Create,
                    QueueState::Ready => QueueReconcileOperation::Config,
                    QueueState::Deleting => QueueReconcileOperation::Delete,
                    QueueState::Tombstoned => continue,
                };
                let lag_ms = now.saturating_sub(queue.updated_at_ms).max(0);
                metrics.observe_queue_reconcile(
                    operation,
                    success,
                    Duration::from_millis(u64::try_from(lag_ms).unwrap_or(u64::MAX)),
                );
            }
        }
        let count = result?;
        self.reconcile_delivery_pauses()?;
        Ok(count)
    }

    pub(crate) fn upsert_consumer(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        worker_id: WorkerId,
        config: QueueConsumerConfig,
        dead_letter_queue: Option<QueueId>,
        now_ms: i64,
    ) -> Result<QueueConsumerRecord, PlatformError> {
        let config = config.validate(self.max_consumer_concurrency)?;
        let queues = QueueRepository::new(self.storage.db());
        let source = queues.get(account_id, queue_id)?;
        require_ready(&source)?;
        let dead_letter_queue = dead_letter_queue
            .map(|id| {
                if id == source.id {
                    return Err(PlatformError::new(
                        ErrorCode::QueueDlqInvalid,
                        "Queue cannot dead-letter to itself",
                    ));
                }
                let queue = queues.get(account_id, id)?;
                require_ready(&queue)?;
                Ok((queue.id, queue.lifecycle_generation))
            })
            .transpose()?;
        let workers = WorkerRepository::new(self.storage.db());
        let worker = workers.get_worker(account_id, worker_id)?;
        let version_id = worker.active_version_id.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::QueueConsumerNotReady,
                "Queue consumer Worker has no active version",
            )
        })?;
        let descriptor = serde_json::json!({
            "capabilityVersion": 1,
            "queueId": source.id,
            "queueLifecycleGeneration": source.lifecycle_generation,
            "entrypoint": null,
            "maxBatchSize": config.max_batch_size,
            "maxBatchTimeoutSeconds": config.max_batch_timeout_seconds,
            "maxRetries": config.max_retries,
            "retryDelaySeconds": config.retry_delay_seconds,
            "maxConcurrency": config.max_concurrency,
            "deadLetterQueueId": dead_letter_queue.map(|value| value.0),
            "deadLetterQueueLifecycleGeneration": dead_letter_queue.map(|value| value.1),
        });
        let declaration = QueueConsumerRepository::new(self.storage.db()).create_api_declaration(
            version_id,
            &NewQueueConsumerDeclaration {
                id: QueueConsumerId::generate(),
                queue_id,
                queue_lifecycle_generation: source.lifecycle_generation,
                entrypoint: None,
                config,
                dead_letter_queue,
                capability_version: 1,
                descriptor_sha256: Sha256::digest(
                    serde_json::to_vec(&descriptor).map_err(|_| internal())?,
                )
                .into(),
            },
            now_ms,
        )?;
        let repository = QueueConsumerRepository::new(self.storage.db());
        let record = match repository.live_for_queue(queue_id)? {
            None => repository.create_attachment(account_id, worker_id, &declaration, now_ms)?,
            Some(record) => {
                if !matches!(
                    record.state,
                    QueueConsumerState::Active | QueueConsumerState::Paused
                ) || !repository.begin_update(
                    record.id,
                    record.consumer_generation,
                    worker_id,
                    &declaration,
                    now_ms,
                )? {
                    return Err(projection_pending());
                }
                repository.get(record.id)?
            }
        };
        self.scheduler.repair_products(1_000)?;
        let record = repository.get(record.id)?;
        if matches!(
            record.state,
            QueueConsumerState::Activating | QueueConsumerState::Updating
        ) {
            return Err(projection_pending());
        }
        self.apply_delivery_pause(&source, &record)?;
        repository.get(record.id)
    }

    pub(crate) fn delete_consumer(
        &self,
        account_id: AccountId,
        consumer_id: QueueConsumerId,
        _request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let repository = QueueConsumerRepository::new(self.storage.db());
        let record = repository.get(consumer_id)?;
        if record.account_id != account_id {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotFound,
                "consumer not found",
            ));
        }
        if record.state != QueueConsumerState::Deleting
            && !repository.begin_delete(record.id, record.consumer_generation, now_ms)?
        {
            return Err(projection_pending());
        }
        self.scheduler.repair_products(1_000)?;
        if repository.get(record.id)?.state != QueueConsumerState::Tombstoned {
            return Err(projection_pending());
        }
        Ok(())
    }

    fn reconcile_delivery_pauses(&self) -> Result<(), PlatformError> {
        let queues = QueueRepository::new(self.storage.db());
        let consumers = QueueConsumerRepository::new(self.storage.db());
        for queue in queues.list_account(self.storage.identity().default_account_id)? {
            if let Some(record) = consumers.live_for_queue(queue.id)? {
                self.apply_delivery_pause(&queue, &record)?;
            }
        }
        Ok(())
    }

    fn apply_delivery_pause(
        &self,
        queue: &open_compute_storage::QueueRecord,
        record: &QueueConsumerRecord,
    ) -> Result<(), PlatformError> {
        let request_id = RequestId::generate();
        match (queue.delivery_paused, record.state) {
            (true, QueueConsumerState::Active) => self.scheduler.pause_queue_consumer_operator(
                record.id,
                record.consumer_generation,
                request_id,
            ),
            (false, QueueConsumerState::Paused) => self.scheduler.resume_queue_consumer_operator(
                record.id,
                record.consumer_generation,
                request_id,
            ),
            _ => Ok(()),
        }
    }
}

fn require_ready(queue: &open_compute_storage::QueueRecord) -> Result<(), PlatformError> {
    if queue.state == QueueState::Ready && queue.availability == QueueAvailability::Healthy {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::QueueConsumerNotReady,
            "Queue is not ready for a consumer",
        ))
    }
}

fn projection_pending() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueConsumerProjectionPending,
        "Queue consumer projection is pending",
    )
}

pub(crate) fn now_ms() -> Result<i64, PlatformError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| internal())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| internal())
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "Queue lifecycle operation failed")
}
