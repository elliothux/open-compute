//! Vectorize index lifecycle driver.

use crate::{ReconcileOutcome, ResourceDriver, ResourceHealth};
use open_compute_core::{
    BindingKind, ErrorCode, PlatformError, ResourceAvailability, ResourceState,
};
use open_compute_storage::{
    PlatformStorage, ResourceRecord, VECTORIZE_SCHEMA_VERSION, VectorizeEngine,
    VectorizeIndexRepository, VectorizePaths,
};

/// Frozen product specification used for one create operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VectorizeIndexSpec {
    /// Exact dimensions.
    pub dimensions: u32,
    /// Metric token.
    pub metric: String,
    /// Applied vector quota.
    pub quota_vectors: u64,
    /// Local SQLite byte quota.
    pub quota_bytes: u64,
}

/// Static filesystem and SQLite driver for `vectorize_index` resources.
#[derive(Debug)]
pub struct VectorizeResourceDriver<'a> {
    storage: &'a PlatformStorage,
    create_spec: Option<VectorizeIndexSpec>,
    busy_timeout_ms: u64,
}

impl<'a> VectorizeResourceDriver<'a> {
    /// Bind platform authority and one request's frozen create specification.
    #[must_use]
    pub const fn new(
        storage: &'a PlatformStorage,
        spec: VectorizeIndexSpec,
        busy_timeout_ms: u64,
    ) -> Self {
        Self {
            storage,
            create_spec: Some(spec),
            busy_timeout_ms,
        }
    }

    /// Build the startup recovery driver; it never invents a missing create specification.
    #[must_use]
    pub const fn recovery(storage: &'a PlatformStorage, busy_timeout_ms: u64) -> Self {
        Self {
            storage,
            create_spec: None,
            busy_timeout_ms,
        }
    }

    fn paths(&self) -> Result<VectorizePaths, PlatformError> {
        VectorizePaths::open(self.storage.data_dir().root())
    }

    fn verify_live(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let record = VectorizeIndexRepository::new(self.storage.db())
            .get(resource.account_id, resource.id)?;
        let path = self.paths()?.resolve_storage_key(
            &record.storage_key,
            resource.account_id,
            resource.id,
        )?;
        VectorizeEngine::open(
            &path,
            &resource.id.to_string(),
            record.dimensions,
            &record.metric,
            record.quota_vectors,
            record.quota_bytes,
            self.busy_timeout_ms,
        )?
        .quick_check()
    }
}

impl ResourceDriver for VectorizeResourceDriver<'_> {
    fn kind(&self) -> BindingKind {
        BindingKind::VectorizeIndex
    }

    fn create_fingerprint_material(&self) -> Vec<u8> {
        self.create_spec.as_ref().map_or_else(Vec::new, |spec| {
            format!(
                "{}\0{}\0{}\0{}",
                spec.dimensions, spec.metric, spec.quota_vectors, spec.quota_bytes
            )
            .into_bytes()
        })
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let spec = self.create_spec.as_ref().ok_or_else(not_ready)?;
        if resource.kind != BindingKind::VectorizeIndex
            || resource.state != ResourceState::Creating
            || !(32..=1_536).contains(&spec.dimensions)
            || !matches!(spec.metric.as_str(), "cosine" | "euclidean" | "dot-product")
            || spec.quota_vectors == 0
            || spec.quota_vectors > 200_000
            || spec.quota_bytes < 1_048_576
        {
            return Err(invariant());
        }
        let paths = self.paths()?;
        let storage_key = VectorizePaths::storage_key(resource.account_id, resource.id);
        let catalog = VectorizeIndexRepository::new(self.storage.db());
        let record = match catalog.get(resource.account_id, resource.id) {
            Ok(record) => record,
            Err(error) if error.code() == ErrorCode::ResourceNotFound => catalog.ensure_index(
                resource,
                &storage_key,
                VECTORIZE_SCHEMA_VERSION,
                spec.dimensions,
                &spec.metric,
                spec.quota_vectors,
                spec.quota_bytes,
            )?,
            Err(error) => return Err(error),
        };
        let live = paths.resolve_storage_key(&storage_key, resource.account_id, resource.id)?;
        if live.exists() {
            return self.verify_live(resource);
        }
        let staging = paths.create_staging(resource.id)?;
        let result = (|| {
            VectorizeEngine::open(
                &staging.join("data.sqlite"),
                &resource.id.to_string(),
                record.dimensions,
                &record.metric,
                record.quota_vectors,
                record.quota_bytes,
                self.busy_timeout_ms,
            )?
            .quick_check()?;
            paths.publish_staging(&staging, resource.account_id, resource.id)?;
            self.verify_live(resource)
        })();
        if result.is_err() && staging.exists() {
            let _ = paths.remove_operation_dir(&staging);
        }
        result
    }

    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        let paths = self.paths()?;
        match resource.state {
            ResourceState::Creating => {
                let record = match VectorizeIndexRepository::new(self.storage.db())
                    .get(resource.account_id, resource.id)
                {
                    Ok(record) => record,
                    Err(error) if error.code() == ErrorCode::ResourceNotFound => {
                        return Ok(if self.create_spec.is_some() {
                            ReconcileOutcome::Absent
                        } else {
                            ReconcileOutcome::Deferred
                        });
                    }
                    Err(error) => return Err(error),
                };
                let live = paths.resolve_storage_key(
                    &record.storage_key,
                    resource.account_id,
                    resource.id,
                )?;
                if live.exists() {
                    self.verify_live(resource)?;
                    return Ok(ReconcileOutcome::Ready);
                }
                let candidates = paths.staging_candidates(resource.id)?;
                if candidates.len() > 1 {
                    return Err(invariant());
                }
                let Some(staging) = candidates.first() else {
                    return Ok(ReconcileOutcome::Absent);
                };
                VectorizeEngine::open(
                    &staging.join("data.sqlite"),
                    &resource.id.to_string(),
                    record.dimensions,
                    &record.metric,
                    record.quota_vectors,
                    record.quota_bytes,
                    self.busy_timeout_ms,
                )?
                .quick_check()?;
                paths.publish_staging(staging, resource.account_id, resource.id)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Ready => {
                self.verify_live(resource)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Deleting => Ok(
                if paths.index_dir(resource.account_id, resource.id).exists() {
                    ReconcileOutcome::Ready
                } else {
                    ReconcileOutcome::Deleted
                },
            ),
            ResourceState::Tombstoned => Ok(ReconcileOutcome::Deleted),
        }
    }

    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let paths = self.paths()?;
        let live = paths.index_dir(resource.account_id, resource.id);
        if !live.exists() {
            return Ok(());
        }
        let record = VectorizeIndexRepository::new(self.storage.db())
            .get(resource.account_id, resource.id)?;
        VectorizeEngine::open(
            &live.join("data.sqlite"),
            &resource.id.to_string(),
            record.dimensions,
            &record.metric,
            record.quota_vectors,
            record.quota_bytes,
            self.busy_timeout_ms,
        )?
        .checkpoint(true)?;
        paths.quarantine(resource.account_id, resource.id)?;
        Ok(())
    }

    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let paths = self.paths()?;
        if paths.index_dir(resource.account_id, resource.id).exists() {
            return Err(invariant());
        }
        for path in paths.quarantine_candidates(resource.id)? {
            paths.remove_operation_dir(&path)?;
        }
        Ok(())
    }

    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        match self.verify_live(resource) {
            Ok(()) => Ok(ResourceHealth::healthy()),
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::ResourceInvariantViolation | ErrorCode::PathInvalid
                ) =>
            {
                Ok(ResourceHealth {
                    availability: ResourceAvailability::Unavailable,
                    code: Some("VECTORIZE_CORRUPT"),
                })
            }
            Err(error) if error.code() == ErrorCode::ResourceUnavailable => Ok(ResourceHealth {
                availability: ResourceAvailability::Unavailable,
                code: Some("VECTORIZE_UNAVAILABLE"),
            }),
            Err(error) => Err(error),
        }
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Vectorize lifecycle invariant failed",
    )
}

fn not_ready() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotReady,
        "Vectorize create specification is unavailable",
    )
}

#[cfg(test)]
#[path = "vectorize_tests.rs"]
mod tests;
