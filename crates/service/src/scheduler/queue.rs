//! P2.2 Queue retention adapter isolated from Alarm dispatch ownership.

use super::{SchedulerService, decode_pool_state, encode_pool_state};
use open_compute_core::{ErrorCode, SchedulerKind, SchedulerPoolState};
use std::sync::atomic::Ordering;

impl SchedulerService {
    pub(super) async fn run_queue_maintenance(&self, now_ms: i64, rows: u32) {
        self.queue_in_flight.store(1, Ordering::Release);
        let store = self.store.clone();
        let result = tokio::task::spawn_blocking(move || {
            store.sweep_queue_retention(now_ms, rows, 4 * 1024 * 1024)
        })
        .await;
        self.queue_in_flight.store(0, Ordering::Release);
        match result {
            Ok(Ok(batch)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_retention(true, batch.messages, batch.bytes);
                    if let Ok((messages, bytes)) = self.store.queue_backlog_totals() {
                        metrics.set_queue_backlog(messages, bytes);
                    }
                }
                self.set_queue_pool_state(SchedulerPoolState::Ready);
            }
            Ok(Err(error))
                if matches!(
                    error.code(),
                    ErrorCode::QueueInvariantViolation | ErrorCode::SchedulerCorrupt
                ) =>
            {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_retention(false, 0, 0);
                }
                self.set_queue_pool_state(SchedulerPoolState::CircuitOpen);
            }
            Ok(Err(error)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_retention(false, 0, 0);
                }
                self.set_queue_pool_state(SchedulerPoolState::Backoff);
                tracing::warn!(
                    code = error.code().as_str(),
                    "Queue retention entered bounded backoff"
                );
            }
            Err(_) => {
                if let Some(metrics) = &self.metrics {
                    metrics.observe_queue_retention(false, 0, 0);
                }
                self.set_queue_pool_state(SchedulerPoolState::Backoff);
            }
        }
    }

    pub(super) fn poll_queue_once(&self) -> Result<usize, open_compute_core::PlatformError> {
        let queue = self.config.pool(SchedulerKind::Queue);
        if !queue.enabled || self.is_paused() || self.queue_paused.load(Ordering::Acquire) {
            return Ok(0);
        }
        let result = self.store.sweep_queue_retention(
            self.observed_wall_time_ms(),
            queue.claim_batch,
            4 * 1024 * 1024,
        )?;
        if let Some(metrics) = &self.metrics {
            metrics.observe_queue_retention(true, result.messages, result.bytes);
            if let Ok((messages, bytes)) = self.store.queue_backlog_totals() {
                metrics.set_queue_backlog(messages, bytes);
            }
        }
        Ok(usize::try_from(result.messages).unwrap_or(usize::MAX))
    }

    pub(super) fn queue_pool_state(&self) -> SchedulerPoolState {
        decode_pool_state(self.queue_pool_state.load(Ordering::Acquire))
    }

    pub(super) fn set_queue_pool_state(&self, state: SchedulerPoolState) {
        self.queue_pool_state
            .store(encode_pool_state(state), Ordering::Release);
        if let Some(metrics) = &self.metrics {
            metrics.set_scheduler_pool_state(SchedulerKind::Queue, state);
        }
    }
}
