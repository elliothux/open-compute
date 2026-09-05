//! Periodic probe-driven R2 resource and provider health maintenance.

use crate::health::HealthCoordinator;
use open_compute_artifacts::R2ObjectStore;
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, PlatformError, R2Config, ReadinessReason,
    ResourceAvailability, ResourceId, ResourceState,
};
use open_compute_storage::{PlatformStorage, R2BucketRepository, ResourceRepository};
use open_compute_workers::R2ResourceDriver;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const MAX_BUCKETS_PER_PASS: usize = 64;
const MIN_PROVIDER_DEBOUNCE: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct R2Maintenance {
    provider_failures: HashMap<ResourceId, Instant>,
    next_offset: usize,
}

impl R2Maintenance {
    pub(crate) async fn run(
        &mut self,
        storage: &Arc<PlatformStorage>,
        objects: &R2ObjectStore,
        config: &R2Config,
        health: &HealthCoordinator,
    ) {
        let repository = R2BucketRepository::new(storage.db());
        let buckets = match repository.list_all() {
            Ok(buckets) => buckets,
            Err(error) => {
                tracing::warn!(
                    code = error.code().as_str(),
                    "R2 maintenance catalog pass failed"
                );
                return;
            }
        };
        let live = buckets
            .iter()
            .map(|bucket| bucket.resource.id)
            .collect::<HashSet<_>>();
        self.provider_failures
            .retain(|resource, _| live.contains(resource));
        let ready = buckets
            .into_iter()
            .filter(|bucket| bucket.resource.state == ResourceState::Ready)
            .collect::<Vec<_>>();
        if ready.is_empty() {
            self.next_offset = 0;
            return;
        }
        let start = self.next_offset % ready.len();
        let count = ready.len().min(MAX_BUCKETS_PER_PASS);
        self.next_offset = (start + count) % ready.len();
        let mut saw_ready = false;
        let mut provider_failure_sustained = false;
        let now_ms = unix_ms();
        let timeout = Duration::from_millis(config.operation_timeout_ms);
        let debounce = timeout.saturating_mul(2).max(MIN_PROVIDER_DEBOUNCE);
        let resources = ResourceRepository::new(storage.db());
        let driver = R2ResourceDriver::new(storage, objects.clone(), config.clone());

        for bucket in ready.iter().cycle().skip(start).take(count) {
            saw_ready = true;
            let result = tokio::time::timeout(timeout, async {
                driver.reconcile(&bucket.resource).await?;
                crate::r2_backend::multipart::reconcile_bucket_multipart(
                    storage, objects, bucket, false, false, timeout,
                )
                .await?;
                Ok::<_, PlatformError>(())
            })
            .await
            .unwrap_or_else(|_| Err(provider_unavailable()));
            let _ = repository.mark_probed(bucket.resource.id, now_ms);
            match result {
                Ok(_) => {
                    self.provider_failures.remove(&bucket.resource.id);
                    if bucket.resource.availability != ResourceAvailability::Healthy {
                        let _ = resources.set_availability(
                            bucket.resource.account_id,
                            bucket.resource.id,
                            ResourceAvailability::Healthy,
                            None,
                            now_ms,
                        );
                    }
                }
                Err(error) if error.code() == ErrorCode::R2ProviderUnavailable => {
                    let first = self
                        .provider_failures
                        .entry(bucket.resource.id)
                        .or_insert_with(Instant::now);
                    if first.elapsed() < debounce {
                        continue;
                    }
                    provider_failure_sustained = true;
                    let availability =
                        if bucket.resource.availability == ResourceAvailability::Healthy {
                            ResourceAvailability::Degraded
                        } else {
                            ResourceAvailability::Unavailable
                        };
                    let _ = resources.set_availability(
                        bucket.resource.account_id,
                        bucket.resource.id,
                        availability,
                        Some(ErrorCode::R2ProviderUnavailable.as_str()),
                        now_ms,
                    );
                }
                Err(error) => {
                    self.provider_failures.remove(&bucket.resource.id);
                    let _ = resources.set_availability(
                        bucket.resource.account_id,
                        bucket.resource.id,
                        ResourceAvailability::Unavailable,
                        Some(error.code().as_str()),
                        now_ms,
                    );
                }
            }
        }

        if saw_ready {
            update_provider_health(health, provider_failure_sustained);
        }
    }
}

#[cfg(test)]
#[path = "r2_maintenance_tests.rs"]
mod tests;

fn update_provider_health(health: &HealthCoordinator, failed: bool) {
    let snapshot = health.snapshot();
    let Some(component) = snapshot
        .components
        .iter()
        .find(|component| component.name == ComponentName::ObjectStorage)
    else {
        return;
    };
    if component.state == ComponentState::Draining {
        return;
    }
    let desired = if failed {
        ComponentState::Degraded
    } else {
        ComponentState::Healthy
    };
    let reason = if failed {
        ReadinessReason::ObjectStorageDegraded
    } else {
        ReadinessReason::Ready
    };
    if component.state != desired
        && let Err(error) =
            health.set_component(ComponentName::ObjectStorage, desired, Some(reason))
    {
        tracing::warn!(
            code = error.code().as_str(),
            "R2 provider health transition failed"
        );
    }
}

fn provider_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2ProviderUnavailable,
        "R2 provider health probe timed out",
    )
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}
