//! Fair host-wide coordinator for durable Vectorize mutation frontiers.

use open_compute_core::{ComponentName, ErrorCode, PlatformError, ResourceAvailability};
use open_compute_storage::{
    PlatformStorage, ResourceRepository, VectorizeEngine, VectorizeIndexRecord,
    VectorizeIndexRepository, VectorizePaths,
};
use open_compute_workers::ResourcePins;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

const INDEX_BATCH: u32 = 1_024;
const SAFETY_INTERVAL: Duration = Duration::from_millis(500);

/// One bounded coordinator pass summary without resource-ID-valued labels.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VectorizeCoordinatorReport {
    /// Ready indexes inspected.
    pub indexes: u32,
    /// Mutations applied.
    pub applied: u32,
    /// Index frontiers blocked by permanent failure.
    pub blocked: u32,
    /// Indexes currently held by an unexpired claim.
    pub claimed: u32,
}

/// Global, bounded owner of Vectorize mutation progress.
#[derive(Clone, Debug)]
pub struct VectorizeCoordinator {
    storage: Arc<PlatformStorage>,
    metrics: Option<Arc<crate::metrics::MetricsRegistry>>,
    health: Option<crate::health::HealthCoordinator>,
    pins: ResourcePins,
    cursor: Arc<Mutex<Option<(open_compute_core::AccountId, open_compute_core::ResourceId)>>>,
}

impl VectorizeCoordinator {
    /// Bind platform and per-index authority.
    #[must_use]
    pub fn new(storage: Arc<PlatformStorage>, pins: ResourcePins) -> Self {
        Self {
            storage,
            metrics: None,
            health: None,
            pins,
            cursor: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach the process fixed-series metrics registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<crate::metrics::MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Attach the process health authority for background failure propagation.
    #[must_use]
    pub(crate) fn with_health(mut self, health: crate::health::HealthCoordinator) -> Self {
        self.health = Some(health);
        self
    }

    /// Apply at most one frontier mutation per ready index for fair scheduling.
    pub fn drain_once(&self) -> Result<VectorizeCoordinatorReport, PlatformError> {
        self.drain_page(INDEX_BATCH)
    }

    fn drain_page(&self, limit: u32) -> Result<VectorizeCoordinatorReport, PlatformError> {
        let after = *self.cursor.lock().map_err(|_| coordinator_error())?;
        let repository = VectorizeIndexRepository::new(self.storage.db());
        let mut indexes = repository.ready_indexes_after(after, limit)?;
        if indexes.is_empty() && after.is_some() {
            indexes = repository.ready_indexes_after(None, limit)?;
        }
        let next_cursor = if indexes.len() == usize::try_from(limit).unwrap_or(usize::MAX) {
            indexes
                .last()
                .map(|index| (index.resource.account_id, index.resource.id))
        } else {
            None
        };
        *self.cursor.lock().map_err(|_| coordinator_error())? = next_cursor;
        let mut report = VectorizeCoordinatorReport {
            indexes: u32::try_from(indexes.len()).unwrap_or(u32::MAX),
            ..VectorizeCoordinatorReport::default()
        };
        for index in indexes {
            let Ok(_pin) = self.pins.try_pin(index.resource.id) else {
                continue;
            };
            let index = match VectorizeIndexRepository::new(self.storage.db())
                .get(index.resource.account_id, index.resource.id)
            {
                Ok(index) if index.resource.state == open_compute_core::ResourceState::Ready => {
                    index
                }
                Ok(_) | Err(_) => continue,
            };
            let engine = match open_engine(&self.storage, &index).and_then(|engine| {
                engine.quick_check()?;
                Ok(engine)
            }) {
                Ok(engine) => {
                    if index.resource.availability != ResourceAvailability::Healthy {
                        ResourceRepository::new(self.storage.db()).set_availability(
                            index.resource.account_id,
                            index.resource.id,
                            ResourceAvailability::Healthy,
                            None,
                            unix_ms(),
                        )?;
                    }
                    engine
                }
                Err(error)
                    if matches!(
                        error.code(),
                        ErrorCode::ResourceInvariantViolation
                            | ErrorCode::ResourceUnavailable
                            | ErrorCode::PathInvalid
                    ) =>
                {
                    let code = if error.code() == ErrorCode::ResourceInvariantViolation {
                        "VECTORIZE_CORRUPT"
                    } else {
                        "VECTORIZE_UNAVAILABLE"
                    };
                    ResourceRepository::new(self.storage.db()).set_availability(
                        index.resource.account_id,
                        index.resource.id,
                        ResourceAvailability::Unavailable,
                        Some(code),
                        unix_ms(),
                    )?;
                    report.blocked = report.blocked.saturating_add(1);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let now_ms = unix_ms();
            match engine.apply_next(now_ms) {
                Ok(Some(_)) => report.applied = report.applied.saturating_add(1),
                Ok(None) if engine.frontier_is_claimed(now_ms)? => {
                    report.claimed = report.claimed.saturating_add(1);
                }
                Ok(None) => {}
                Err(error) if error.code() == ErrorCode::ResourceUnavailable => {
                    report.blocked = report.blocked.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }
        if let Some(metrics) = &self.metrics {
            metrics.observe_vectorize_coordinator(
                report.indexes,
                report.applied,
                report.claimed,
                report.blocked,
            );
        }
        Ok(report)
    }

    /// Run an immediate pass and bounded periodic safety reconciliation until shutdown.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(SAFETY_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                _ = interval.tick() => {
                    let coordinator = self.clone();
                    let healthy = matches!(
                        tokio::task::spawn_blocking(move || coordinator.drain_once()).await,
                        Ok(Ok(_))
                    );
                    if let Some(health) = &self.health {
                        let _ = health.set_search_background(
                            ComponentName::VectorizeMutations,
                            healthy,
                        );
                    }
                }
            }
        }
    }
}

fn coordinator_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Vectorize coordinator state is unavailable",
    )
}

fn open_engine(
    storage: &PlatformStorage,
    index: &VectorizeIndexRecord,
) -> Result<VectorizeEngine, PlatformError> {
    let path = VectorizePaths::open(storage.data_dir().root())?.resolve_storage_key(
        &index.storage_key,
        index.resource.account_id,
        index.resource.id,
    )?;
    VectorizeEngine::open(
        &path,
        &index.resource.id.to_string(),
        index.dimensions,
        &index.metric,
        index.quota_vectors,
        index.quota_bytes,
        storage.sqlite_busy_timeout_ms(),
    )
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "vectorize_coordinator_tests.rs"]
mod tests;
