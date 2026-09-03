//! Single-process Queue/Cron cross-database promotion handoff.

use open_compute_core::{CronSchedule, ErrorCode, PlatformError, QueueConsumerId, QueueId};
use open_compute_storage::{
    CronActivationRecord, CronActivationState, CronRepository, CronScheduleProjection,
    PlatformStorage, QueueConsumerDeclaration, QueueConsumerProjection, QueueConsumerRecord,
    QueueConsumerRepository, QueueConsumerState, SchedulerStore, WorkerRepository,
};
use open_compute_workers::{ProductPromotionCoordinator, ProductPromotionRequest};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Owner of the ordered control/scheduler Queue and Cron handoff.
#[derive(Clone, Debug)]
pub(crate) struct P23PromotionCoordinator {
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    drain_timeout: Duration,
}

impl P23PromotionCoordinator {
    /// Bind promotion authority to the two databases owned by this ocd process.
    #[must_use]
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        scheduler: Arc<SchedulerStore>,
        drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            scheduler,
            drain_timeout,
        }
    }

    fn coordinate(&self, request: &ProductPromotionRequest) -> Result<(), PlatformError> {
        let workers = WorkerRepository::new(self.storage.db());
        let worker = workers.get_worker(request.account_id, request.worker_id)?;
        let already_promoted = worker.active_version_id == Some(request.version_id);
        let execution_generation = if already_promoted {
            worker.route_generation
        } else {
            worker
                .route_generation
                .checked_add(1)
                .ok_or_else(projection_pending)?
        };

        let queue_repo = QueueConsumerRepository::new(self.storage.db());
        let declarations = queue_repo.version_declarations(request.version_id)?;
        let desired: HashMap<QueueId, QueueConsumerDeclaration> = declarations
            .into_iter()
            .map(|declaration| (declaration.queue_id, declaration))
            .collect();
        let existing = queue_repo.live_for_worker(request.worker_id)?;
        validate_queue_conflicts(queue_repo, request.worker_id, desired.keys().copied())?;

        let mut queue_finish = Vec::new();
        for declaration in desired.values() {
            let current = existing
                .iter()
                .find(|record| record.queue_id == declaration.queue_id)
                .cloned();
            let record = if let Some(mut current) = current {
                if current.state == QueueConsumerState::Deleting
                    || (current.state == QueueConsumerState::Activating
                        && current.version_id != request.version_id)
                {
                    self.finish_queue_removal(queue_repo, &current, request.now_ms)?;
                    queue_repo.create_attachment(
                        request.account_id,
                        request.worker_id,
                        declaration,
                        request.now_ms,
                    )?
                } else {
                    if current.version_id != request.version_id {
                        if matches!(
                            current.state,
                            QueueConsumerState::Active | QueueConsumerState::Paused
                        ) {
                            if !queue_repo.begin_update(
                                current.id,
                                current.consumer_generation,
                                request.worker_id,
                                declaration,
                                request.now_ms,
                            )? {
                                return Err(projection_pending());
                            }
                            current = queue_repo.get(current.id)?;
                        }
                        if current.state != QueueConsumerState::Updating {
                            return Err(projection_pending());
                        }
                        let old_generation = current
                            .consumer_generation
                            .checked_sub(1)
                            .ok_or_else(projection_pending)?;
                        self.drain_delete_queue_if_present(
                            current.queue_id,
                            current.id,
                            old_generation,
                            request.now_ms,
                        )?;
                        if !queue_repo.switch_target(
                            current.id,
                            current.consumer_generation,
                            declaration,
                            request.now_ms,
                        )? {
                            return Err(projection_pending());
                        }
                        current = queue_repo.get(current.id)?;
                    }
                    current
                }
            } else {
                queue_repo.create_attachment(
                    request.account_id,
                    request.worker_id,
                    declaration,
                    request.now_ms,
                )?
            };
            self.scheduler
                .ensure_queue_consumer_projection(&queue_projection(
                    &record,
                    declaration,
                    self.scheduler
                        .queue_consumer_execution_generation(record.id, record.consumer_generation)?
                        .unwrap_or(execution_generation),
                    request.now_ms,
                ))?;
            match record.state {
                QueueConsumerState::Activating => {
                    queue_finish.push(QueueFinish::Activate(record));
                }
                QueueConsumerState::Updating => queue_finish.push(QueueFinish::Update {
                    paused: was_paused_before_update(&record),
                    record,
                }),
                QueueConsumerState::Active => self.scheduler.activate_queue_consumer(
                    record.id,
                    record.consumer_generation,
                    request.now_ms,
                )?,
                QueueConsumerState::Paused => {
                    self.scheduler.activate_queue_consumer(
                        record.id,
                        record.consumer_generation,
                        request.now_ms,
                    )?;
                    self.scheduler.pause_queue_consumer(
                        record.id,
                        record.consumer_generation,
                        request.now_ms,
                    )?;
                }
                QueueConsumerState::Deleting | QueueConsumerState::Tombstoned => {
                    return Err(projection_pending());
                }
            }
        }
        for record in existing
            .iter()
            .filter(|record| !desired.contains_key(&record.queue_id))
        {
            self.finish_queue_removal(queue_repo, record, request.now_ms)?;
        }

        let cron_repo = CronRepository::new(self.storage.db());
        let cron_config = cron_repo.version_config(request.version_id)?;
        let old_crons = cron_repo.live_for_worker(request.worker_id)?;
        let maximum_generation = old_crons
            .iter()
            .map(|activation| activation.activation_generation)
            .max()
            .unwrap_or(0);
        let reusable_generation = old_crons
            .iter()
            .filter(|activation| {
                activation.version_id == request.version_id
                    && matches!(
                        activation.state,
                        CronActivationState::Staging | CronActivationState::Active
                    )
            })
            .map(|activation| activation.activation_generation)
            .max()
            .filter(|generation| *generation == maximum_generation);
        let (generation, staged) = if let Some(generation) = reusable_generation {
            (
                generation,
                old_crons
                    .iter()
                    .filter(|activation| {
                        activation.version_id == request.version_id
                            && activation.activation_generation == generation
                            && matches!(
                                activation.state,
                                CronActivationState::Staging | CronActivationState::Active
                            )
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else {
            let generation = maximum_generation
                .checked_add(1)
                .ok_or_else(projection_pending)?;
            (
                generation,
                cron_repo.stage_activations(
                    request.account_id,
                    request.worker_id,
                    request.version_id,
                    generation,
                    &cron_config.declarations,
                    request.now_ms,
                )?,
            )
        };
        for activation in &staged {
            let parsed = CronSchedule::parse(&activation.expression)?;
            self.scheduler
                .ensure_cron_schedule_projection(&CronScheduleProjection {
                    activation_id: activation.id,
                    account_id: activation.account_id,
                    worker_id: activation.worker_id,
                    version_id: activation.version_id,
                    execution_generation: self
                        .scheduler
                        .cron_execution_generation(activation.id, activation.activation_generation)?
                        .unwrap_or(execution_generation),
                    activation_generation: activation.activation_generation,
                    expression: activation.expression.clone(),
                    expression_sha256: activation.expression_sha256,
                    parser_version: activation.parser_version,
                    next_fire_at_ms: parsed.next_after_ms(request.now_ms)?,
                    updated_at_ms: request.now_ms,
                })?;
        }
        cron_repo.retire_before(request.worker_id, generation, request.now_ms)?;
        for activation in old_crons
            .iter()
            .filter(|activation| activation.activation_generation < generation)
        {
            self.drain_cron_if_present(activation, request.now_ms)?;
        }

        if !already_promoted {
            workers.create_deployment_checked(
                request.account_id,
                request.worker_id,
                request.version_id,
                worker.active_version_id,
                Some(worker.route_generation),
                request.source,
                &request.annotations,
                request.request_id,
                request.now_ms,
            )?;
        }

        for action in queue_finish {
            let (record, paused, updating) = match action {
                QueueFinish::Activate(record) => (record, false, false),
                QueueFinish::Update { record, paused } => (record, paused, true),
            };
            self.scheduler.activate_queue_consumer(
                record.id,
                record.consumer_generation,
                request.now_ms,
            )?;
            if paused {
                self.scheduler.pause_queue_consumer(
                    record.id,
                    record.consumer_generation,
                    request.now_ms,
                )?;
            }
            let finished = if updating {
                queue_repo.finish_update(
                    record.id,
                    record.consumer_generation,
                    paused,
                    request.now_ms,
                )?
            } else {
                queue_repo.finish_activation(
                    record.id,
                    record.consumer_generation,
                    request.now_ms,
                )?
            };
            if !finished {
                return Err(projection_pending());
            }
        }
        for activation in &staged {
            self.scheduler.activate_cron_schedule(
                activation.id,
                activation.activation_generation,
                request.now_ms,
            )?;
        }
        cron_repo.activate_generation(request.worker_id, generation, request.now_ms)?;
        for activation in old_crons
            .iter()
            .filter(|activation| activation.activation_generation < generation)
        {
            self.scheduler
                .delete_cron_schedule_projection(activation.id, activation.activation_generation)?;
            if activation.state != CronActivationState::Tombstoned
                && !cron_repo.finish_retire(
                    activation.id,
                    activation.activation_generation,
                    request.now_ms,
                )?
            {
                return Err(projection_pending());
            }
        }
        Ok(())
    }

    fn finish_queue_removal(
        &self,
        repository: QueueConsumerRepository<'_>,
        record: &QueueConsumerRecord,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if record.state != QueueConsumerState::Deleting
            && !repository.begin_delete(record.id, record.consumer_generation, now_ms)?
        {
            return Err(projection_pending());
        }
        self.drain_delete_queue_if_present(
            record.queue_id,
            record.id,
            record.consumer_generation,
            now_ms,
        )?;
        if let Some(previous) = record.consumer_generation.checked_sub(1) {
            self.drain_delete_queue_if_present(record.queue_id, record.id, previous, now_ms)?;
        }
        if !repository.finish_delete(record.id, record.consumer_generation, now_ms)? {
            return Err(projection_pending());
        }
        Ok(())
    }

    fn drain_delete_queue_if_present(
        &self,
        queue_id: QueueId,
        consumer_id: QueueConsumerId,
        generation: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if generation == 0
            || !self
                .scheduler
                .inspect_queue_consumer_runtime(queue_id, consumer_id, generation)?
                .projection_exists
        {
            return Ok(());
        }
        self.scheduler
            .drain_queue_consumer(consumer_id, generation, now_ms)?;
        self.wait_queue_drain(consumer_id, generation)?;
        self.scheduler
            .delete_queue_consumer_projection(consumer_id, generation)
    }

    fn drain_cron_if_present(
        &self,
        activation: &CronActivationRecord,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if !self
            .scheduler
            .inspect_cron_runtime(activation.id, activation.activation_generation, now_ms)?
            .projection_exists
        {
            return Ok(());
        }
        self.scheduler.drain_cron_schedule(
            activation.id,
            activation.activation_generation,
            now_ms,
        )?;
        self.wait_cron_drain(activation)
    }

    fn wait_queue_drain(
        &self,
        consumer_id: QueueConsumerId,
        generation: u64,
    ) -> Result<(), PlatformError> {
        let deadline = Instant::now() + self.drain_timeout;
        while self
            .scheduler
            .queue_consumer_in_flight(consumer_id, generation)?
            > 0
        {
            if Instant::now() >= deadline {
                return Err(projection_pending());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn wait_cron_drain(&self, activation: &CronActivationRecord) -> Result<(), PlatformError> {
        let deadline = Instant::now() + self.drain_timeout;
        while self
            .scheduler
            .cron_activation_in_flight(activation.id, activation.activation_generation)?
            > 0
        {
            if Instant::now() >= deadline {
                return Err(PlatformError::new(
                    ErrorCode::CronProjectionPending,
                    "Cron activation drain did not complete before the bound",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

impl ProductPromotionCoordinator for P23PromotionCoordinator {
    fn promote(
        &self,
        request: ProductPromotionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        let coordinator = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || coordinator.coordinate(&request))
                .await
                .map_err(|_| projection_pending())?
        })
    }
}

fn validate_queue_conflicts(
    repository: QueueConsumerRepository<'_>,
    worker_id: open_compute_core::WorkerId,
    queues: impl Iterator<Item = QueueId>,
) -> Result<(), PlatformError> {
    for queue_id in queues {
        if repository
            .live_for_queue(queue_id)?
            .is_some_and(|record| record.worker_id != worker_id)
        {
            return Err(PlatformError::new(
                ErrorCode::QueueConsumerConflict,
                "Queue already has a live consumer owned by another Worker",
            ));
        }
    }
    Ok(())
}

fn queue_projection(
    record: &QueueConsumerRecord,
    declaration: &QueueConsumerDeclaration,
    execution_generation: u64,
    now_ms: i64,
) -> QueueConsumerProjection {
    QueueConsumerProjection {
        consumer_id: record.id,
        queue_id: declaration.queue_id,
        consumer_generation: record.consumer_generation,
        version_id: declaration.version_id,
        worker_id: record.worker_id,
        execution_generation,
        entrypoint: declaration.entrypoint.clone(),
        config: declaration.config,
        dead_letter_queue: declaration
            .dlq_queue_id
            .zip(declaration.dlq_lifecycle_generation),
        descriptor_sha256: declaration.descriptor_sha256,
        updated_at_ms: now_ms,
    }
}

#[derive(Debug)]
enum QueueFinish {
    Activate(QueueConsumerRecord),
    Update {
        record: QueueConsumerRecord,
        paused: bool,
    },
}

fn was_paused_before_update(record: &QueueConsumerRecord) -> bool {
    record.availability_code.as_deref() == Some("QUEUE_CONSUMER_DRAINING_PAUSED")
}

fn projection_pending() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueConsumerProjectionPending,
        "Queue/Cron cross-database promotion handoff is pending",
    )
}
