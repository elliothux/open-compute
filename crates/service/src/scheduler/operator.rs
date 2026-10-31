//! Authenticated Queue/Cron operator mutations, reconciliation, and inspection.

use super::{
    CronActivationInspect, QueueConsumerInspect, SchedulerGlobalInspect, SchedulerInspect,
    SchedulerPoolInspect, SchedulerService, atomic_option_i64, queue_consumer_generation_stale,
};
use open_compute_core::{
    PlatformError, QueueConsumerId, RequestId, SchedulerKind, SchedulerPoolState,
};
use open_compute_storage::cron::CronRepository;
use open_compute_storage::queue_consumers::{QueueConsumerRepository, QueueConsumerState};
use open_compute_storage::scheduler::SchedulerSummary;
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
            return Ok(());
        }
        if record.state != QueueConsumerState::Active {
            return Err(queue_consumer_generation_stale());
        }
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
            return Ok(());
        }
        if record.state != QueueConsumerState::Paused {
            return Err(queue_consumer_generation_stale());
        }
        if !repository.resume(id, generation, request_id, now_ms)? {
            return Err(queue_consumer_generation_stale());
        }
        Ok(())
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
    pub fn inspect(&self) -> Result<SchedulerInspect, PlatformError> {
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
                instance_id: record.instance_id,
                queue_id: record.queue_id,
                worker_id: record.worker_id,
                version_id: record.version_id,
                pending_version_id: None,
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
            let runtime = self.store.inspect_cron_runtime(activation.id, now_ms)?;
            cron_activations.push(CronActivationInspect {
                id: activation.id,
                instance_id: activation.instance_id,
                worker_id: activation.worker_id,
                version_id: activation.version_id,
                expression: activation.expression,
                parser_version: activation.parser_version,
                generation: self
                    .store
                    .cron_execution_generation(activation.id)?
                    .unwrap_or_default(),
                state: open_compute_storage::cron::CronActivationState::Active,
                projection_exists: runtime.projection_exists,
                schedule_state: runtime.schedule_state,
                next_fire_at: runtime.next_fire_at_ms,
                ready_runs: runtime.ready_runs,
                claimed_runs: runtime.claimed_runs,
                last_outcome: runtime.last_outcome,
                last_error_code: runtime.last_error_code,
                last_unknown_reason: runtime.last_unknown_reason,
                oldest_dispatch_deadline_at: runtime.oldest_dispatch_deadline_at_ms,
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
        Ok(SchedulerInspect {
            version: 1,
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
