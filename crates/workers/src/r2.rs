//! P0.5 logical R2 bucket lifecycle over typed S3 authority.

use open_compute_artifacts::{R2BucketIdentity, R2BucketLocator, R2ObjectStore};
use open_compute_core::{BindingKind, ErrorCode, PlatformError, R2Config, ResourceState};
use open_compute_storage::{
    PlatformStorage, R2_SCHEMA_VERSION, R2BucketRecord, R2BucketRepository, ResourceRecord,
};

/// Async product driver for `r2_bucket` resources.
#[derive(Clone, Debug)]
pub struct R2ResourceDriver<'a> {
    storage: &'a PlatformStorage,
    objects: R2ObjectStore,
    config: R2Config,
}

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

    /// Reconcile one creating or ready bucket from SQLite plus its S3 marker.
    pub async fn reconcile(
        &self,
        resource: &ResourceRecord,
    ) -> Result<R2BucketRecord, PlatformError> {
        if resource.kind != BindingKind::R2Bucket {
            return Err(invariant());
        }
        let repository = R2BucketRepository::new(self.storage.db());
        let bucket = match repository.get(resource.account_id, resource.id) {
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
            || bucket.provider_config_sha256 != self.objects.authority_sha256()
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
        platform_id: storage.identity().platform_id,
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

#[cfg(test)]
#[path = "r2_tests.rs"]
mod tests;
