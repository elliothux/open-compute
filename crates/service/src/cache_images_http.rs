//! Authenticated operator controls for response-cache lifecycle and Images capacity.

use crate::images_backend::ImageBindingService;
use crate::metrics::MetricsRegistry;
use crate::run::gc_worker_artifacts;
use crate::snapshot_pins::SnapshotPins;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{PlatformError, WorkersConfig};
use open_compute_storage::CacheStats;
use open_compute_storage::{CacheManager, PlatformStorage};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Composed operator-only P3.3 authority.
#[derive(Clone)]
pub(crate) struct CacheImagesApiState {
    storage: Arc<PlatformStorage>,
    cache: Arc<CacheManager>,
    images: Arc<ImageBindingService>,
    artifacts: ArtifactStore,
    workers: WorkersConfig,
    snapshot_pins: Arc<SnapshotPins>,
    metrics: Arc<MetricsRegistry>,
}

impl std::fmt::Debug for CacheImagesApiState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheImagesApiState")
            .finish_non_exhaustive()
    }
}

impl CacheImagesApiState {
    /// Compose already-verified local authorities behind the existing admin capability.
    #[must_use]
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        cache: Arc<CacheManager>,
        images: Arc<ImageBindingService>,
        artifacts: ArtifactStore,
        workers: WorkersConfig,
        snapshot_pins: Arc<SnapshotPins>,
        metrics: Arc<MetricsRegistry>,
    ) -> Self {
        Self {
            storage,
            cache,
            images,
            artifacts,
            workers,
            snapshot_pins,
            metrics,
        }
    }

    /// Inspect the process-wide response-cache authority.
    pub(crate) fn cache_stats(&self) -> Result<CacheStats, PlatformError> {
        let stats = self.cache.stats(now_ms())?;
        self.metrics.set_response_cache_stats(stats);
        Ok(stats)
    }

    /// Run the existing artifact/cache garbage-collection workflow.
    pub(crate) async fn garbage_collect(&self) -> Result<u64, PlatformError> {
        gc_worker_artifacts(
            &self.storage,
            &self.artifacts,
            &self.workers,
            &self.snapshot_pins,
            Some(self.cache.clone()),
        )
        .await
    }

    /// Inspect the bounded native Images admission authority.
    pub(crate) fn image_capacity(
        &self,
    ) -> Result<crate::images_backend::ImageCapacity, PlatformError> {
        self.images.capacity()
    }
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "cache_images_http_tests.rs"]
mod tests;
