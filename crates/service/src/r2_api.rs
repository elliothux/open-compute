//! R2 composition state shared by the v4 API and runtime bindings.

use crate::r2_backend::R2BindingService;
use open_compute_artifacts::R2ObjectStore;
use open_compute_core::{
    BindingKind, ErrorCode, PlatformError, R2Config, RequestId, ResourceState,
};
use open_compute_storage::{PlatformStorage, R2BucketRepository, ResourceRepository};
use open_compute_workers::{R2ResourceDriver, ResourcePins};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Shared R2 composition state.
#[derive(Clone)]
pub struct R2ApiState {
    storage: Arc<PlatformStorage>,
    objects: R2ObjectStore,
    pins: ResourcePins,
    config: R2Config,
    delete_drain_timeout: Duration,
    binding: Option<Arc<R2BindingService>>,
}

impl std::fmt::Debug for R2ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2ApiState")
            .field("config", &self.config)
            .field("binding", &self.binding.is_some())
            .finish_non_exhaustive()
    }
}

impl R2ApiState {
    /// Bind durable authority, typed object access, pins, and frozen defaults.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        objects: R2ObjectStore,
        pins: ResourcePins,
        config: R2Config,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            objects,
            pins,
            config,
            delete_drain_timeout,
            binding: None,
        }
    }

    /// Attach the authoritative R2 binding executor used by management operations.
    #[must_use]
    pub fn with_binding(mut self, binding: Arc<R2BindingService>) -> Self {
        self.binding = Some(binding);
        self
    }

    pub(crate) fn binding(&self) -> Result<&Arc<R2BindingService>, PlatformError> {
        self.binding.as_ref().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "R2 management binding is unavailable",
            )
        })
    }

    /// Recover every creating/deleting R2 lifecycle before readiness.
    pub async fn reconcile_pending(&self) -> Result<u32, PlatformError> {
        let candidates = ResourceRepository::new(self.storage.db()).reconcile_candidates()?;
        let driver = self.driver();
        let mut reconciled = 0_u32;
        for resource in candidates {
            if resource.kind != BindingKind::R2Bucket {
                continue;
            }
            match resource.state {
                ResourceState::Creating => {
                    driver.reconcile(&resource).await?;
                    ResourceRepository::new(self.storage.db()).mark_ready(resource.id, now_ms())?;
                }
                ResourceState::Deleting => {
                    let bucket = R2BucketRepository::new(self.storage.db())
                        .get(resource.account_id, resource.id)?;
                    R2BucketRepository::new(self.storage.db())
                        .mark_delete_started(resource.id, now_ms())?;
                    crate::r2_backend::multipart::reconcile_bucket_multipart(
                        &self.storage,
                        &self.objects,
                        &bucket,
                        true,
                        true,
                        Duration::from_millis(self.config.operation_timeout_ms),
                    )
                    .await?;
                    crate::r2_backend::objects::reconcile_bucket_objects(
                        &self.storage,
                        &self.objects,
                        &bucket,
                        Duration::from_millis(self.config.operation_timeout_ms),
                    )
                    .await?;
                    driver.drain_objects(&bucket).await?;
                    driver.finalize_delete(&bucket).await?;
                    ResourceRepository::new(self.storage.db()).mark_tombstoned(
                        resource.account_id,
                        resource.id,
                        RequestId::generate(),
                        now_ms(),
                    )?;
                }
                ResourceState::Ready | ResourceState::Tombstoned => continue,
            }
            reconciled = reconciled.saturating_add(1);
        }
        for bucket in R2BucketRepository::new(self.storage.db()).list_all()? {
            if bucket.resource.state == ResourceState::Ready {
                crate::r2_backend::multipart::reconcile_bucket_multipart(
                    &self.storage,
                    &self.objects,
                    &bucket,
                    true,
                    false,
                    Duration::from_millis(self.config.operation_timeout_ms),
                )
                .await?;
                crate::r2_backend::objects::reconcile_bucket_objects(
                    &self.storage,
                    &self.objects,
                    &bucket,
                    Duration::from_millis(self.config.operation_timeout_ms),
                )
                .await?;
            }
        }
        Ok(reconciled)
    }

    fn driver(&self) -> R2ResourceDriver<'_> {
        R2ResourceDriver::new(&self.storage, self.objects.clone(), self.config.clone())
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) const fn objects(&self) -> &R2ObjectStore {
        &self.objects
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    pub(crate) const fn config(&self) -> &R2Config {
        &self.config
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
    }

    pub(crate) fn resource_driver(&self) -> R2ResourceDriver<'_> {
        self.driver()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
