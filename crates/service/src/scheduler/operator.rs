//! Authenticated Queue/Cron operator mutations, reconciliation, and inspection.

use super::{
    CronActivationInspect, QueueConsumerInspect, SchedulerGlobalInspect, SchedulerInspectV2,
    SchedulerPoolInspect, SchedulerService, atomic_option_i64, queue_consumer_generation_stale,
    scheduler_task_failed,
};
use open_compute_core::{
    CronSchedule, PlatformError, QueueConsumerId, RequestId, SchedulerKind, SchedulerPoolState,
};
use open_compute_storage::{
    CronActivationState, CronRepository, CronScheduleProjection, QueueConsumerProjection,
    QueueConsumerRepository, QueueConsumerState, SchedulerSummary, WorkerRepository,
};
use std::sync::atomic::Ordering;

impl SchedulerService {
    /// Pause one exact Queue consumer generation across scheduler and control authority.
    pub fn pause_queue_consumer_operator(
        &self,
        id: QueueConsumerId,
        generation: u64,
        request_id: RequestId,
    ) -> Result<(), PlatformError> {
        let now_ms = self.observed_wall_time_ms();
        let repository = QueueConsumerRepository::new(self.storage.db());
        let record = repository.get(id)?;
        if record.consumer_generation != generation {
            return Err(queue_consumer_generation_stale());
        }
        if record.state == QueueConsumerState::Paused {
            self.store.pause_queue_consumer(id, generation, now_ms)?;
            return Ok(());
        }
        if record.state != QueueConsumerState::Active {
            return Err(queue_consumer_generation_stale());
        }
        self.store.pause_queue_consumer(id, generation, now_ms)?;
        if !repository.pause(id, generation, request_id, now_ms)? {
            return Err(queue_consumer_generation_stale());
        }
        Ok(())
    }

    /// Resume one exact paused Queue consumer generation.
    pub fn resume_queue_consumer_operator(
        &self,
        id: QueueConsumerId,
        generation: u64,
        request_id: RequestId,
    ) -> Result<(), PlatformError> {
        let now_ms = self.observed_wall_time_ms();
        let repository = QueueConsumerRepository::new(self.storage.db());
        let record = repository.get(id)?;
        if record.consumer_generation != generation {
            return Err(queue_consumer_generation_stale());
        }
        if record.state == QueueConsumerState::Active {
            self.store.activate_queue_consumer(id, generation, now_ms)?;
            return Ok(());
        }
        if record.state != QueueConsumerState::Paused {
            return Err(queue_consumer_generation_stale());
        }
        self.store.activate_queue_consumer(id, generation, now_ms)?;
        if !repository.resume(id, generation, request_id, now_ms)? {
            return Err(queue_consumer_generation_stale());
        }
        Ok(())
    }

    /// Boundedly reconcile Queue/Cron control rows with exact scheduler projections.
    pub fn repair_products(&self, limit: u32) -> Result<u64, PlatformError> {
        if limit == 0 {
            return Err(scheduler_task_failed());
        }
        let now_ms = self.observed_wall_time_ms();
        let workers = WorkerRepository::new(self.storage.db());
        let queues = QueueConsumerRepository::new(self.storage.db());
        let mut repaired = 0_u64;
        for record in queues.list_live(limit)? {
            let mut record = record;
            let mut declaration = queues.declaration(record.declaration_id)?;
            let worker = workers.get_worker(record.account_id, record.worker_id)?;
            if record.state == QueueConsumerState::Deleting {
                let mut drained = true;
                for generation in [
                    Some(record.consumer_generation),
                    record.consumer_generation.checked_sub(1),
                ]
                .into_iter()
                .flatten()
                {
                    let runtime = self.store.inspect_queue_consumer_runtime(
                        record.queue_id,
                        record.id,
                        generation,
                    )?;
                    if runtime.projection_exists {
                        self.store
                            .drain_queue_consumer(record.id, generation, now_ms)?;
                        if self.store.queue_consumer_in_flight(record.id, generation)? == 0 {
                            self.store
                                .delete_queue_consumer_projection(record.id, generation)?;
                        } else {
                            drained = false;
                        }
                    }
                }
                if !drained {
                    continue;
                }
                if queues.finish_delete(record.id, record.consumer_generation, now_ms)? {
                    repaired = repaired.saturating_add(1);
                }
                continue;
            }
            if record.state == QueueConsumerState::Updating
                && let (Some(pending_declaration), Some(_)) =
                    (record.pending_declaration_id, record.pending_deployment_id)
            {
                let old_generation = record
                    .consumer_generation
                    .checked_sub(1)
                    .ok_or_else(scheduler_task_failed)?;
                let runtime = self.store.inspect_queue_consumer_runtime(
                    record.queue_id,
                    record.id,
                    old_generation,
                )?;
                if runtime.projection_exists {
                    self.store
                        .drain_queue_consumer(record.id, old_generation, now_ms)?;
                    if self
                        .store
                        .queue_consumer_in_flight(record.id, old_generation)?
                        == 0
                    {
                        self.store
                            .delete_queue_consumer_projection(record.id, old_generation)?;
                    } else {
                        continue;
                    }
                }
                let pending = queues.declaration(pending_declaration)?;
                if !queues.switch_target(record.id, record.consumer_generation, &pending, now_ms)? {
                    return Err(queue_consumer_generation_stale());
                }
                record = queues.get(record.id)?;
                declaration = pending;
            }
            let execution_generation = if worker.active_deployment_id == Some(record.deployment_id)
            {
                worker.route_generation
            } else {
                worker
                    .route_generation
                    .checked_add(1)
                    .ok_or_else(scheduler_task_failed)?
            };
            self.store
                .ensure_queue_consumer_projection(&QueueConsumerProjection {
                    consumer_id: record.id,
                    queue_id: record.queue_id,
                    consumer_generation: record.consumer_generation,
                    deployment_id: record.deployment_id,
                    worker_id: record.worker_id,
                    execution_generation,
                    entrypoint: declaration.entrypoint,
                    config: declaration.config,
                    dead_letter_queue: declaration
                        .dlq_queue_id
                        .zip(declaration.dlq_lifecycle_generation),
                    descriptor_sha256: declaration.descriptor_sha256,
                    updated_at_ms: now_ms,
                })?;
            match record.state {
                QueueConsumerState::Active => self.store.activate_queue_consumer(
                    record.id,
                    record.consumer_generation,
                    now_ms,
                )?,
                QueueConsumerState::Paused => {
                    self.store.activate_queue_consumer(
                        record.id,
                        record.consumer_generation,
                        now_ms,
                    )?;
                    self.store.pause_queue_consumer(
                        record.id,
                        record.consumer_generation,
                        now_ms,
                    )?;
                }
                QueueConsumerState::Activating
                    if worker.active_deployment_id == Some(record.deployment_id) =>
                {
                    self.store.activate_queue_consumer(
                        record.id,
                        record.consumer_generation,
                        now_ms,
                    )?;
                    if !queues.finish_activation(record.id, record.consumer_generation, now_ms)? {
                        return Err(queue_consumer_generation_stale());
                    }
                }
                QueueConsumerState::Updating
                    if worker.active_deployment_id == Some(record.deployment_id) =>
                {
                    let paused = record.availability_code.as_deref()
                        == Some("QUEUE_CONSUMER_DRAINING_PAUSED");
                    self.store.activate_queue_consumer(
                        record.id,
                        record.consumer_generation,
                        now_ms,
                    )?;
                    if paused {
                        self.store.pause_queue_consumer(
                            record.id,
                            record.consumer_generation,
                            now_ms,
                        )?;
                    }
                    if !queues.finish_update(
                        record.id,
                        record.consumer_generation,
                        paused,
                        now_ms,
                    )? {
                        return Err(queue_consumer_generation_stale());
                    }
                }
                QueueConsumerState::Activating | QueueConsumerState::Updating => {}
                QueueConsumerState::Deleting | QueueConsumerState::Tombstoned => {}
            }
            repaired = repaired.saturating_add(1);
        }

        let crons = CronRepository::new(self.storage.db());
        for activation in crons.list_live(limit)? {
            let worker = workers.get_worker(activation.account_id, activation.worker_id)?;
            let execution_generation =
                if worker.active_deployment_id == Some(activation.deployment_id) {
                    worker.route_generation
                } else {
                    worker
                        .route_generation
                        .checked_add(1)
                        .ok_or_else(scheduler_task_failed)?
                };
            let runtime = self.store.inspect_cron_runtime(
                activation.id,
                activation.activation_generation,
                now_ms,
            )?;
            if activation.state == CronActivationState::Retiring {
                if runtime.projection_exists {
                    self.store.drain_cron_schedule(
                        activation.id,
                        activation.activation_generation,
                        now_ms,
                    )?;
                    if self.store.cron_activation_in_flight(
                        activation.id,
                        activation.activation_generation,
                    )? > 0
                    {
                        continue;
                    }
                    self.store.delete_cron_schedule_projection(
                        activation.id,
                        activation.activation_generation,
                    )?;
                }
                if crons.finish_retire(activation.id, activation.activation_generation, now_ms)? {
                    repaired = repaired.saturating_add(1);
                }
                continue;
            }
            let parsed = CronSchedule::parse(&activation.expression)?;
            self.store
                .ensure_cron_schedule_projection(&CronScheduleProjection {
                    activation_id: activation.id,
                    account_id: activation.account_id,
                    worker_id: activation.worker_id,
                    deployment_id: activation.deployment_id,
                    execution_generation,
                    activation_generation: activation.activation_generation,
                    expression: activation.expression,
                    expression_sha256: activation.expression_sha256,
                    parser_version: activation.parser_version,
                    next_fire_at_ms: parsed.next_after_ms(now_ms)?,
                    updated_at_ms: now_ms,
                })?;
            if activation.state == CronActivationState::Active
                || (activation.state == CronActivationState::Staging
                    && worker.active_deployment_id == Some(activation.deployment_id))
            {
                self.store.activate_cron_schedule(
                    activation.id,
                    activation.activation_generation,
                    now_ms,
                )?;
                if activation.state == CronActivationState::Staging {
                    crons.activate_generation(
                        activation.worker_id,
                        activation.activation_generation,
                        now_ms,
                    )?;
                }
            }
            repaired = repaired.saturating_add(1);
        }
        Ok(repaired)
    }

    /// Whether a fixed workload is process-locally paused.
    pub fn is_kind_paused(&self, kind: SchedulerKind) -> Result<bool, PlatformError> {
        self.ensure_kind_enabled(kind)?;
        Ok(match kind {
            SchedulerKind::Alarm => self.alarm_paused.load(Ordering::Acquire),
            SchedulerKind::Queue => self.queue_paused.load(Ordering::Acquire),
            SchedulerKind::Cron => self.cron_paused.load(Ordering::Acquire),
            SchedulerKind::Workflow => self.workflow_paused.load(Ordering::Acquire),
        })
    }

    /// Low-cardinality state summary for health and operator inspection.
    pub fn summary(&self) -> Result<SchedulerSummary, PlatformError> {
        self.store.summary(self.observed_wall_time_ms())
    }

    /// Versioned global and registered-pool operator state.
    pub fn inspect(&self) -> Result<SchedulerInspectV2, PlatformError> {
        let now_ms = self.observed_wall_time_ms();
        let summary = self.store.workload_summary(now_ms)?;
        let alarm = self.config.pool(SchedulerKind::Alarm);
        let queue = self.config.pool(SchedulerKind::Queue);
        let cron = self.config.pool(SchedulerKind::Cron);
        let workflow = self.config.pool(SchedulerKind::Workflow);
        let queue_summary = self.store.queue_consumer_workload_summary(now_ms)?;
        let cron_summary = self.store.cron_workload_summary(now_ms)?;
        let workflow_summary = self.store.workflow_workload_summary(now_ms)?;
        let mut queue_consumers = Vec::new();
        for record in QueueConsumerRepository::new(self.storage.db()).list_live(1_000)? {
            let runtime = self.store.inspect_queue_consumer_runtime(
                record.queue_id,
                record.id,
                record.consumer_generation,
            )?;
            queue_consumers.push(QueueConsumerInspect {
                id: record.id,
                account_id: record.account_id,
                queue_id: record.queue_id,
                worker_id: record.worker_id,
                deployment_id: record.deployment_id,
                pending_deployment_id: record.pending_deployment_id,
                generation: record.consumer_generation,
                state: record.state,
                projection_exists: runtime.projection_exists,
                backlog_messages: runtime.backlog_messages,
                backlog_bytes: runtime.backlog_bytes,
                ready_messages: runtime.ready_messages,
                claimed_batches: runtime.claimed_batches,
                claimed_messages: runtime.claimed_messages,
                dlq_pending: runtime.dlq_pending,
            });
        }
        let mut cron_activations = Vec::new();
        for activation in CronRepository::new(self.storage.db()).list_live(1_000)? {
            let runtime = self.store.inspect_cron_runtime(
                activation.id,
                activation.activation_generation,
                now_ms,
            )?;
            cron_activations.push(CronActivationInspect {
                id: activation.id,
                account_id: activation.account_id,
                worker_id: activation.worker_id,
                deployment_id: activation.deployment_id,
                expression: activation.expression,
                parser_version: activation.parser_version,
                generation: activation.activation_generation,
                state: activation.state,
                projection_exists: runtime.projection_exists,
                schedule_state: runtime.schedule_state,
                next_fire_at: runtime.next_fire_at_ms,
                ready_runs: runtime.ready_runs,
                claimed_runs: runtime.claimed_runs,
                last_outcome: runtime.last_outcome,
                lag_ms: runtime.lag_ms,
            });
        }
        let state = if !alarm.enabled {
            SchedulerPoolState::Disabled
        } else if self.is_paused() || self.alarm_paused.load(Ordering::Acquire) {
            SchedulerPoolState::Paused
        } else {
            self.alarm_pool_state()
        };
        Ok(SchedulerInspectV2 {
            version: 2,
            paused: self.is_paused(),
            global: SchedulerGlobalInspect {
                in_flight: self.global_in_flight.load(Ordering::Acquire),
                max_in_flight: self.config.max_in_flight,
                next_wake_at: atomic_option_i64(&self.next_wake_at_ms),
            },
            pools: vec![
                SchedulerPoolInspect {
                    kind: SchedulerKind::Alarm,
                    enabled: alarm.enabled,
                    state,
                    ready: summary.ready,
                    claimed: summary.claimed,
                    expired: summary.expired,
                    oldest_due_at: summary.oldest_due_at_ms,
                    next_due_at: summary.next_due_at_ms,
                    in_flight: self.alarm_in_flight.load(Ordering::Acquire),
                    max_in_flight: alarm.max_in_flight,
                },
                SchedulerPoolInspect {
                    kind: SchedulerKind::Queue,
                    enabled: queue.enabled,
                    state: if !queue.enabled {
                        SchedulerPoolState::Disabled
                    } else if self.is_paused() || self.queue_paused.load(Ordering::Acquire) {
                        SchedulerPoolState::Paused
                    } else {
                        self.queue_pool_state()
                    },
                    ready: queue_summary.ready,
                    claimed: queue_summary.claimed,
                    expired: queue_summary.expired,
                    oldest_due_at: queue_summary.oldest_due_at_ms,
                    next_due_at: queue_summary.next_due_at_ms,
                    in_flight: self.queue_in_flight.load(Ordering::Acquire),
                    max_in_flight: queue.max_in_flight,
                },
                SchedulerPoolInspect {
                    kind: SchedulerKind::Cron,
                    enabled: cron.enabled,
                    state: if !cron.enabled {
                        SchedulerPoolState::Disabled
                    } else if self.is_paused() || self.cron_paused.load(Ordering::Acquire) {
                        SchedulerPoolState::Paused
                    } else {
                        self.cron_pool_state()
                    },
                    ready: cron_summary.ready,
                    claimed: cron_summary.claimed,
                    expired: cron_summary.expired,
                    oldest_due_at: cron_summary.oldest_due_at_ms,
                    next_due_at: cron_summary.next_due_at_ms,
                    in_flight: self.cron_in_flight.load(Ordering::Acquire),
                    max_in_flight: cron.max_in_flight,
                },
                SchedulerPoolInspect {
                    kind: SchedulerKind::Workflow,
                    enabled: workflow.enabled,
                    state: if !workflow.enabled {
                        SchedulerPoolState::Disabled
                    } else if self.is_paused() || self.workflow_paused.load(Ordering::Acquire) {
                        SchedulerPoolState::Paused
                    } else {
                        self.workflow_pool_state()
                    },
                    ready: workflow_summary.ready,
                    claimed: workflow_summary.claimed,
                    expired: workflow_summary.expired,
                    oldest_due_at: workflow_summary.oldest_due_at_ms,
                    next_due_at: workflow_summary.next_due_at_ms,
                    in_flight: self.workflow_in_flight.load(Ordering::Acquire),
                    max_in_flight: workflow.max_in_flight,
                },
            ],
            queue_consumers,
            cron_activations,
        })
    }
}
