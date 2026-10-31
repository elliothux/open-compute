//! P0.5 logical R2 bucket lifecycle over the typed object authority.

use crate::ResourcePins;
use open_compute_artifacts::{R2BucketIdentity, R2BucketLocator, R2ObjectStore};
use open_compute_core::{
    BindingKind, ErrorCode, InstanceId, PlatformError, R2Config, RequestId, ResourceId,
    ResourceState,
};
use open_compute_storage::PlatformStorage;
use open_compute_storage::r2::{R2_SCHEMA_VERSION, R2BucketRecord, R2BucketRepository};
use open_compute_storage::resources::{
    ReserveResourceCreate, ResourceCreateReservation, ResourceRecord, ResourceRepository,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Durable input for one R2 bucket create operation.
#[derive(Clone, Debug)]
pub struct CreateR2BucketRequest {
    /// Owning instance.
    pub instance_id: InstanceId,
    /// Validated bucket name.
    pub name: String,
    /// Required operation idempotency identity.
    pub idempotency_key: String,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Current wall-clock milliseconds.
    pub now_ms: i64,
    /// Whether a same-name in-progress bucket is the requested PUT identity.
    pub reconcile_by_name: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateR2BucketReplay {
    resource_id: ResourceId,
}

/// R2-specific async lifecycle controller.
#[derive(Clone, Debug)]
pub struct R2Controller<'a> {
    storage: &'a PlatformStorage,
    pins: ResourcePins,
    driver: R2ResourceDriver<'a>,
}

impl<'a> R2Controller<'a> {
    /// Bind the durable authority, object authority, process pins, and frozen R2 policy.
    #[must_use]
    pub fn new(
        storage: &'a PlatformStorage,
        objects: R2ObjectStore,
        pins: ResourcePins,
        config: R2Config,
    ) -> Self {
        Self {
            storage,
            pins,
            driver: R2ResourceDriver::new(storage, objects, config),
        }
    }

    /// Reserve, reconcile, and persist one ready bucket and its wire-independent replay identity.
    pub async fn create(
        &self,
        request: &CreateR2BucketRequest,
    ) -> Result<R2BucketRecord, PlatformError> {
        let expires_at_ms = request
            .now_ms
            .checked_add(IDEMPOTENCY_TTL_MS)
            .ok_or_else(invariant)?;
        let fingerprint = self.create_fingerprint(request.instance_id, &request.name)?;
        let repository = ResourceRepository::new(self.storage.db());
        let reservation = repository.reserve_create(
            &ReserveResourceCreate {
                instance_id: request.instance_id,
                kind: BindingKind::R2Bucket,
                name: &request.name,
                idempotency_key: &request.idempotency_key,
                fingerprint_key_id: self.storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: request.request_id,
                now_ms: request.now_ms,
                expires_at_ms,
            },
            self.storage.hardening().max_resources_per_kind,
        );
        let resource = match reservation {
            Ok(ResourceCreateReservation::Reserved(resource))
            | Ok(ResourceCreateReservation::Continue(resource)) => resource,
            Ok(ResourceCreateReservation::Complete(response)) => {
                return self.replay(request.instance_id, &response);
            }
            Err(error)
                if request.reconcile_by_name && error.code() == ErrorCode::ResourceNameConflict =>
            {
                return self
                    .reconcile_named(request.instance_id, &request.name, request.now_ms)
                    .await;
            }
            Err(error) => return Err(error),
        };
        let bucket = self.reconcile(&resource, request.now_ms).await?;
        let response = serde_json::to_vec(&CreateR2BucketReplay {
            resource_id: bucket.resource.id,
        })
        .map_err(|_| invariant())?;
        match repository.complete_create(
            request.instance_id,
            &request.idempotency_key,
            &fingerprint,
            bucket.resource.id,
            &response,
        ) {
            Ok(()) => Ok(bucket),
            Err(error) if error.code() == ErrorCode::IdempotencyConflict => {
                match repository.reserve_create(
                    &ReserveResourceCreate {
                        instance_id: request.instance_id,
                        kind: BindingKind::R2Bucket,
                        name: &request.name,
                        idempotency_key: &request.idempotency_key,
                        fingerprint_key_id: self.storage.crypto().fingerprint_key_id(),
                        request_fingerprint: &fingerprint,
                        resource_id: bucket.resource.id,
                        driver_schema_version: R2_SCHEMA_VERSION,
                        request_id: request.request_id,
                        now_ms: request.now_ms,
                        expires_at_ms,
                    },
                    self.storage.hardening().max_resources_per_kind,
                )? {
                    ResourceCreateReservation::Complete(response) => {
                        self.replay(request.instance_id, &response)
                    }
                    _ => Err(idempotency_conflict()),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Converge one bucket identity and persist ready state for a creating row.
    pub async fn reconcile(
        &self,
        resource: &ResourceRecord,
        now_ms: i64,
    ) -> Result<R2BucketRecord, PlatformError> {
        let bucket = self.driver.reconcile(resource).await?;
        if resource.state == ResourceState::Creating {
            match ResourceRepository::new(self.storage.db()).mark_ready(resource.id, now_ms) {
                Ok(_) => {}
                Err(error) => {
                    let current = ResourceRepository::new(self.storage.db())
                        .get(resource.instance_id, resource.id)?;
                    if current.state != ResourceState::Ready {
                        return Err(error);
                    }
                }
            }
        }
        R2BucketRepository::new(self.storage.db()).get(resource.instance_id, bucket.resource.id)
    }

    /// Fence, clean up, and tombstone one live bucket as a single lifecycle operation.
    pub async fn delete(
        &self,
        bucket: &R2BucketRecord,
        request_id: RequestId,
        now_ms: i64,
        drain_deadline: Duration,
        cleanup: impl Future<Output = Result<(), PlatformError>>,
    ) -> Result<(), PlatformError> {
        self.begin_delete(bucket, now_ms, drain_deadline).await?;
        if let Err(error) = cleanup.await {
            self.pins.unfence(bucket.resource.id);
            return Err(error);
        }
        self.finish_delete(bucket, request_id, now_ms, false).await
    }

    async fn begin_delete(
        &self,
        bucket: &R2BucketRecord,
        now_ms: i64,
        drain_deadline: Duration,
    ) -> Result<(), PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        if !repository.referrers(bucket.resource.id)?.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ResourceReferenced,
                "R2 bucket still has retained referrers",
            ));
        }
        self.driver.require_empty(bucket).await?;
        self.pins
            .fence_and_wait(bucket.resource.id, drain_deadline)
            .await?;
        let result = repository
            .begin_delete(bucket.resource.instance_id, bucket.resource.id, now_ms)
            .and_then(|_| {
                R2BucketRepository::new(self.storage.db())
                    .mark_delete_started(bucket.resource.id, now_ms)
                    .map(|_| ())
            });
        if result.is_err() {
            self.pins.unfence(bucket.resource.id);
        }
        result
    }

    /// Resume a persisted deleting row before backend cleanup.
    pub fn resume_delete(&self, bucket: &R2BucketRecord, now_ms: i64) -> Result<(), PlatformError> {
        R2BucketRepository::new(self.storage.db())
            .mark_delete_started(bucket.resource.id, now_ms)
            .map(|_| ())
    }

    /// Remove the physical identity and persist the tombstone after backend cleanup.
    pub async fn finish_delete(
        &self,
        bucket: &R2BucketRecord,
        request_id: RequestId,
        now_ms: i64,
        drain_objects: bool,
    ) -> Result<(), PlatformError> {
        let result = async {
            if drain_objects {
                self.driver.drain_objects(bucket).await?;
            }
            self.driver.finalize_delete(bucket).await?;
            ResourceRepository::new(self.storage.db()).mark_tombstoned(
                bucket.resource.instance_id,
                bucket.resource.id,
                request_id,
                now_ms,
            )?;
            Ok(())
        }
        .await;
        if result.is_ok() {
            self.pins.retire_fence(bucket.resource.id);
        } else {
            self.pins.unfence(bucket.resource.id);
        }
        result
    }

    fn replay(
        &self,
        instance_id: InstanceId,
        response: &[u8],
    ) -> Result<R2BucketRecord, PlatformError> {
        let replay: CreateR2BucketReplay =
            serde_json::from_slice(response).map_err(|_| invariant())?;
        R2BucketRepository::new(self.storage.db()).get(instance_id, replay.resource_id)
    }

    async fn reconcile_named(
        &self,
        instance_id: InstanceId,
        name: &str,
        now_ms: i64,
    ) -> Result<R2BucketRecord, PlatformError> {
        let resource = ResourceRepository::new(self.storage.db())
            .list(instance_id, Some(BindingKind::R2Bucket))?
            .into_iter()
            .find(|resource| resource.name == name && resource.state != ResourceState::Tombstoned)
            .ok_or_else(idempotency_conflict)?;
        match resource.state {
            ResourceState::Creating => self.reconcile(&resource, now_ms).await,
            ResourceState::Ready => {
                R2BucketRepository::new(self.storage.db()).get(instance_id, resource.id)
            }
            ResourceState::Deleting => Err(idempotency_conflict()),
            ResourceState::Tombstoned => Err(invariant()),
        }
    }

    fn create_fingerprint(
        &self,
        instance_id: InstanceId,
        name: &str,
    ) -> Result<[u8; 32], PlatformError> {
        let input = serde_json::to_vec(&serde_json::json!({
            "account": instance_id,
            "name": name,
            "maxObjectBytes": self.driver.config.max_object_bytes,
        }))
        .map_err(|_| invariant())?;
        Ok(self.storage.crypto().fingerprint_request(&input))
    }
}

/// Async product driver for `r2_bucket` resources.
#[derive(Clone, Debug)]
#[cfg_attr(
    not(any(test, feature = "test-support")),
    allow(
        unreachable_pub,
        reason = "public only through the test-support re-export"
    )
)]
pub struct R2ResourceDriver<'a> {
    storage: &'a PlatformStorage,
    objects: R2ObjectStore,
    config: R2Config,
}

#[cfg_attr(
    not(any(test, feature = "test-support")),
    allow(
        unreachable_pub,
        reason = "public only through the test-support re-export"
    )
)]
impl<'a> R2ResourceDriver<'a> {
    /// Bind lifecycle authority, typed object store, and frozen object limits.
    #[must_use]
    pub fn new(storage: &'a PlatformStorage, objects: R2ObjectStore, config: R2Config) -> Self {
        Self {
            storage,
            objects,
            config,
        }
    }

    /// Insert the immutable locator, create the marker, and verify both authorities.
    pub async fn create(&self, resource: &ResourceRecord) -> Result<R2BucketRecord, PlatformError> {
        if resource.kind != BindingKind::R2Bucket
            || resource.state != ResourceState::Creating
            || resource.driver_schema_version != R2_SCHEMA_VERSION
        {
            return Err(invariant());
        }
        let prefix = self.objects.physical_prefix(resource.id);
        let bucket = R2BucketRepository::new(self.storage.db()).ensure_bucket(
            resource,
            &prefix,
            self.config.max_object_bytes,
            &self.objects.authority_sha256(),
        )?;
        let locator = self.locator(&bucket)?;
        self.objects
            .ensure_identity(&locator, &identity(self.storage, resource))
            .await?;
        self.verify_identity(resource, &locator).await?;
        Ok(bucket)
    }

    /// Reconcile one creating or ready bucket from SQLite plus its object marker.
    pub async fn reconcile(
        &self,
        resource: &ResourceRecord,
    ) -> Result<R2BucketRecord, PlatformError> {
        if resource.kind != BindingKind::R2Bucket {
            return Err(invariant());
        }
        let repository = R2BucketRepository::new(self.storage.db());
        let bucket = match repository.get(resource.instance_id, resource.id) {
            Ok(bucket) => bucket,
            Err(error)
                if error.code() == ErrorCode::ResourceNotFound
                    && resource.state == ResourceState::Creating =>
            {
                return self.create(resource).await;
            }
            Err(error) => return Err(error),
        };
        let locator = self.locator(&bucket)?;
        match resource.state {
            ResourceState::Creating => {
                self.objects
                    .ensure_identity(&locator, &identity(self.storage, resource))
                    .await?;
                self.verify_identity(resource, &locator).await?;
            }
            ResourceState::Ready => self.verify_identity(resource, &locator).await?,
            ResourceState::Deleting => return Ok(bucket),
            ResourceState::Tombstoned => return Err(invariant()),
        }
        Ok(bucket)
    }

    /// Refuse deletion while the objects prefix is non-empty.
    pub async fn require_empty(&self, bucket: &R2BucketRecord) -> Result<(), PlatformError> {
        let locator = self.locator(bucket)?;
        if !self.objects.is_empty(&locator).await? {
            return Err(PlatformError::new(
                ErrorCode::R2BucketNotEmpty,
                "R2 bucket is not empty",
            ));
        }
        Ok(())
    }

    /// Idempotently drain every object by repeatedly deleting the current first page.
    pub async fn drain_objects(&self, bucket: &R2BucketRecord) -> Result<u64, PlatformError> {
        let locator = self.locator(bucket)?;
        let mut batches = 0_u64;
        while self.objects.delete_first_page(&locator).await? {
            batches = batches.saturating_add(1);
        }
        Ok(batches)
    }

    /// Confirm the object prefix empty, delete the marker, and confirm both absent.
    pub async fn finalize_delete(&self, bucket: &R2BucketRecord) -> Result<(), PlatformError> {
        let locator = self.locator(bucket)?;
        if !self.objects.is_empty(&locator).await? {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "R2 deletion still has reachable objects",
            ));
        }
        self.objects.delete_identity(&locator).await?;
        if !self.objects.is_empty(&locator).await?
            || self.objects.read_identity(&locator).await?.is_some()
        {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "R2 physical identity is still reachable",
            ));
        }
        Ok(())
    }

    /// Validate the persisted locator through the configured typed store.
    pub fn locator(&self, bucket: &R2BucketRecord) -> Result<R2BucketLocator, PlatformError> {
        if bucket.schema_version != R2_SCHEMA_VERSION
            || bucket.resource.kind != BindingKind::R2Bucket
            || bucket.max_object_bytes != self.config.max_object_bytes
            || bucket.object_authority_sha256 != self.objects.authority_sha256()
        {
            return Err(invariant());
        }
        self.objects
            .locator(bucket.resource.id, &bucket.physical_prefix)
    }

    async fn verify_identity(
        &self,
        resource: &ResourceRecord,
        locator: &R2BucketLocator,
    ) -> Result<(), PlatformError> {
        let found = self.objects.read_identity(locator).await?;
        if found.as_ref() != Some(&identity(self.storage, resource)) {
            return Err(PlatformError::new(
                ErrorCode::R2PrefixCollision,
                "R2 physical prefix identity does not match this resource",
            ));
        }
        Ok(())
    }
}

fn identity(storage: &PlatformStorage, resource: &ResourceRecord) -> R2BucketIdentity {
    R2BucketIdentity {
        schema_version: R2_SCHEMA_VERSION,
        instance_id: storage.identity().instance_id,
        resource_id: resource.id,
        created_at_ms: resource.created_at_ms,
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 lifecycle reconciliation invariant failed",
    )
}

fn idempotency_conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::IdempotencyConflict,
        "R2 create idempotency identity conflicts with durable state",
    )
}

#[cfg(test)]
#[path = "r2_tests.rs"]
mod tests;
