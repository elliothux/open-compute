//! AI Search namespace lifecycle authority.

use crate::{ReconcileOutcome, ResourceDriver, ResourceHealth};
use open_compute_core::{
    BindingKind, ErrorCode, PlatformError, ResourceAvailability, ResourceId, ResourceState,
};
use open_compute_storage::ai_search::{
    AI_SEARCH_SCHEMA_VERSION, AiSearchCatalog, AiSearchInstanceStorageContract, AiSearchPaths,
    AiSearchStore, inspect_ai_search_instance,
};
use open_compute_storage::{PlatformStorage, ResourceRecord};

/// Parent resource driver for `ai_search_namespace`.
///
/// A namespace has no local database. Its catalog row and child referrers are
/// the complete durable authority; each child instance owns its own driver.
#[derive(Debug)]
pub struct AiSearchNamespaceResourceDriver<'a> {
    storage: &'a PlatformStorage,
    description: Option<String>,
}

impl<'a> AiSearchNamespaceResourceDriver<'a> {
    /// Bind platform storage authority.
    #[must_use]
    pub const fn new(storage: &'a PlatformStorage) -> Self {
        Self {
            storage,
            description: None,
        }
    }

    /// Attach the optional Cloudflare-facing namespace description.
    #[must_use]
    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    fn catalog(
        &self,
        resource: &ResourceRecord,
    ) -> Result<open_compute_storage::ai_search::AiSearchNamespaceRecord, PlatformError> {
        AiSearchCatalog::new(self.storage.db()).get_namespace(resource.account_id, resource.id)
    }
}

impl ResourceDriver for AiSearchNamespaceResourceDriver<'_> {
    fn kind(&self) -> BindingKind {
        BindingKind::AiSearchNamespace
    }

    fn create_fingerprint_material(&self) -> Vec<u8> {
        self.description
            .as_deref()
            .unwrap_or("")
            .as_bytes()
            .to_vec()
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        if resource.kind != BindingKind::AiSearchNamespace
            || resource.state != ResourceState::Creating
            || resource.driver_schema_version != 1
        {
            return Err(invariant());
        }
        AiSearchCatalog::new(self.storage.db())
            .ensure_namespace_with_description(resource, self.description.as_deref())?;
        Ok(())
    }

    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        match resource.state {
            ResourceState::Creating => match self.catalog(resource) {
                Ok(_) => Ok(ReconcileOutcome::Ready),
                Err(error) if error.code() == ErrorCode::ResourceNotFound => {
                    Ok(ReconcileOutcome::Absent)
                }
                Err(error) => Err(error),
            },
            ResourceState::Ready => {
                self.catalog(resource)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Deleting | ResourceState::Tombstoned => Ok(ReconcileOutcome::Deleted),
        }
    }

    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        if AiSearchCatalog::new(self.storage.db())
            .has_live_instances(resource.account_id, resource.id)?
        {
            return Err(PlatformError::new(
                ErrorCode::ResourceReferenced,
                "AI Search namespace still has live instances",
            ));
        }
        Ok(())
    }

    fn finalize_delete(&self, _resource: &ResourceRecord) -> Result<(), PlatformError> {
        Ok(())
    }

    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        self.catalog(resource)?;
        Ok(ResourceHealth::healthy())
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "AI Search namespace lifecycle invariant failed",
    )
}

/// Frozen product specification used for one AI Search instance create.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchInstanceSpec {
    /// Parent namespace identity.
    pub namespace_resource_id: ResourceId,
    /// Cloudflare-facing instance identity.
    pub instance_key: String,
    /// Canonical public configuration JSON.
    pub public_config_json: Vec<u8>,
    /// Canonical secret-free model contract JSON.
    pub model_contract_json: Vec<u8>,
    /// SHA-256 of `model_contract_json`.
    pub model_contract_sha256: [u8; 32],
    /// Frozen embedding dimensions, or zero for keyword-only.
    pub dimensions: u32,
    /// Whether vector retrieval is enabled.
    pub vector_enabled: bool,
    /// Whether keyword retrieval is enabled.
    pub keyword_enabled: bool,
}

/// Static filesystem and SQLite lifecycle driver for one AI Search instance.
#[derive(Debug)]
pub struct AiSearchInstanceResourceDriver<'a> {
    storage: &'a PlatformStorage,
    create_spec: Option<AiSearchInstanceSpec>,
    busy_timeout_ms: u64,
}

impl<'a> AiSearchInstanceResourceDriver<'a> {
    /// Bind platform authority and one request's frozen instance specification.
    #[must_use]
    pub const fn new(
        storage: &'a PlatformStorage,
        spec: AiSearchInstanceSpec,
        busy_timeout_ms: u64,
    ) -> Self {
        Self {
            storage,
            create_spec: Some(spec),
            busy_timeout_ms,
        }
    }

    /// Build the startup recovery driver without inventing missing create input.
    #[must_use]
    pub const fn recovery(storage: &'a PlatformStorage, busy_timeout_ms: u64) -> Self {
        Self {
            storage,
            create_spec: None,
            busy_timeout_ms,
        }
    }

    fn paths(&self) -> Result<AiSearchPaths, PlatformError> {
        AiSearchPaths::open(self.storage.data_dir().root())
    }

    fn verify_live(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let record = AiSearchCatalog::new(self.storage.db())
            .get_instance(resource.account_id, resource.id)?;
        let path = self.paths()?.resolve_storage_key(
            &record.storage_key,
            resource.account_id,
            resource.id,
        )?;
        inspect_ai_search_instance(
            &path,
            &resource.id.to_string(),
            record.model_contract_sha256,
            self.busy_timeout_ms,
        )?;
        Ok(())
    }
}

impl ResourceDriver for AiSearchInstanceResourceDriver<'_> {
    fn kind(&self) -> BindingKind {
        BindingKind::AiSearchInstance
    }

    fn create_fingerprint_material(&self) -> Vec<u8> {
        self.create_spec.as_ref().map_or_else(Vec::new, |spec| {
            let mut material = Vec::new();
            material.extend_from_slice(spec.namespace_resource_id.to_string().as_bytes());
            material.push(0);
            material.extend_from_slice(spec.instance_key.as_bytes());
            material.push(0);
            material.extend_from_slice(&spec.model_contract_sha256);
            material.push(0);
            material.extend_from_slice(&spec.public_config_json);
            material
        })
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let spec = self.create_spec.as_ref().ok_or_else(not_ready)?;
        if resource.kind != BindingKind::AiSearchInstance
            || resource.state != ResourceState::Creating
            || resource.driver_schema_version != AI_SEARCH_SCHEMA_VERSION
        {
            return Err(invariant());
        }
        let paths = self.paths()?;
        let storage_key = AiSearchPaths::storage_key(resource.account_id, resource.id);
        let catalog = AiSearchCatalog::new(self.storage.db());
        let record = match catalog.get_instance(resource.account_id, resource.id) {
            Ok(record) => record,
            Err(error) if error.code() == ErrorCode::ResourceNotFound => catalog.ensure_instance(
                resource,
                spec.namespace_resource_id,
                &spec.instance_key,
                &storage_key,
                AI_SEARCH_SCHEMA_VERSION,
                spec.model_contract_sha256,
            )?,
            Err(error) => return Err(error),
        };
        let live =
            paths.resolve_storage_key(&record.storage_key, resource.account_id, resource.id)?;
        if live.exists() {
            return self.verify_live(resource);
        }
        let staging = paths.create_staging(resource.id)?;
        let result = (|| {
            AiSearchStore::open(
                &staging.join("data.sqlite"),
                &AiSearchInstanceStorageContract {
                    resource_id: &resource.id.to_string(),
                    model_contract_sha256: spec.model_contract_sha256,
                    model_contract_json: &spec.model_contract_json,
                    public_config_json: &spec.public_config_json,
                    dimensions: spec.dimensions,
                    vector_enabled: spec.vector_enabled,
                    keyword_enabled: spec.keyword_enabled,
                },
                resource.created_at_ms,
            )?;
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
                let record = match AiSearchCatalog::new(self.storage.db())
                    .get_instance(resource.account_id, resource.id)
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
                inspect_ai_search_instance(
                    &staging.join("data.sqlite"),
                    &resource.id.to_string(),
                    record.model_contract_sha256,
                    self.busy_timeout_ms,
                )?;
                paths.publish_staging(staging, resource.account_id, resource.id)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Ready => {
                self.verify_live(resource)?;
                Ok(ReconcileOutcome::Ready)
            }
            ResourceState::Deleting => Ok(
                if paths
                    .instance_dir(resource.account_id, resource.id)
                    .exists()
                {
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
        let live = paths.instance_dir(resource.account_id, resource.id);
        if !live.exists() {
            return Ok(());
        }
        let record = AiSearchCatalog::new(self.storage.db())
            .get_instance(resource.account_id, resource.id)?;
        let authority = inspect_ai_search_instance(
            &live.join("data.sqlite"),
            &resource.id.to_string(),
            record.model_contract_sha256,
            self.busy_timeout_ms,
        )?;
        let store = AiSearchStore::open(
            &live.join("data.sqlite"),
            &AiSearchInstanceStorageContract {
                resource_id: &authority.resource_id,
                model_contract_sha256: authority.model_contract_sha256,
                model_contract_json: &authority.inspection.indexing_model_contract_json,
                public_config_json: &authority.inspection.indexing_public_config_json,
                dimensions: authority.dimensions,
                vector_enabled: authority.vector_enabled,
                keyword_enabled: authority.keyword_enabled,
            },
            resource.created_at_ms,
        )?;
        if store.pending_object_gc_count()? != 0 || !store.object_references()?.is_empty() {
            return Err(not_ready());
        }
        store.checkpoint(true)?;
        paths.quarantine(resource.account_id, resource.id)?;
        Ok(())
    }

    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let paths = self.paths()?;
        if paths
            .instance_dir(resource.account_id, resource.id)
            .exists()
        {
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
                    code: Some("AI_SEARCH_CORRUPT"),
                })
            }
            Err(error) if error.code() == ErrorCode::ResourceUnavailable => Ok(ResourceHealth {
                availability: ResourceAvailability::Unavailable,
                code: Some("AI_SEARCH_UNAVAILABLE"),
            }),
            Err(error) => Err(error),
        }
    }
}

fn not_ready() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotReady,
        "AI Search create input is unavailable during recovery",
    )
}

#[cfg(test)]
#[path = "ai_search_tests.rs"]
mod tests;
