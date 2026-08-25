//! Durable Object namespace lifecycle driver.

use crate::{ReconcileOutcome, ResourceDriver, ResourceHealth};
use open_compute_core::{
    BindingKind, ErrorCode, PlatformError, ResourceAvailability, ResourceState, WorkerId,
};
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository, PlatformStorage, ResourceRecord,
};

/// Static control-plane driver for immutable Durable Object namespaces.
#[derive(Debug)]
pub struct DurableObjectResourceDriver<'a> {
    storage: &'a PlatformStorage,
    owner_worker_id: WorkerId,
    class_name: &'a str,
}

impl<'a> DurableObjectResourceDriver<'a> {
    /// Bind the central authority and immutable namespace identity supplied at create time.
    #[must_use]
    pub const fn new(
        storage: &'a PlatformStorage,
        owner_worker_id: WorkerId,
        class_name: &'a str,
    ) -> Self {
        Self {
            storage,
            owner_worker_id,
            class_name,
        }
    }

    fn verify(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        let namespace = DurableObjectRepository::new(self.storage)
            .get_namespace(resource.account_id, resource.id)?;
        if namespace.owner_worker_id != self.owner_worker_id
            || namespace.class_name != self.class_name
            || namespace.schema_version != DO_NAMESPACE_SCHEMA_VERSION
        {
            return Err(invariant());
        }
        Ok(())
    }
}

impl ResourceDriver for DurableObjectResourceDriver<'_> {
    fn kind(&self) -> BindingKind {
        BindingKind::DoNamespace
    }

    fn create_fingerprint_material(&self) -> Vec<u8> {
        let mut material = Vec::with_capacity(16 + self.class_name.len());
        material.extend_from_slice(self.owner_worker_id.as_uuid().as_bytes());
        material.extend_from_slice(self.class_name.as_bytes());
        material
    }

    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        DurableObjectRepository::new(self.storage)
            .ensure_namespace(resource, self.owner_worker_id, self.class_name)
            .map(|_| ())
    }

    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError> {
        match resource.state {
            ResourceState::Creating => match self.verify(resource) {
                Ok(()) => Ok(ReconcileOutcome::Ready),
                Err(error) if error.code() == ErrorCode::DoNamespaceNotFound => {
                    Ok(ReconcileOutcome::Absent)
                }
                Err(error) => Err(error),
            },
            ResourceState::Ready => self.verify(resource).map(|()| ReconcileOutcome::Ready),
            ResourceState::Deleting => self
                .begin_delete(resource)
                .map(|()| ReconcileOutcome::Deleted),
            ResourceState::Tombstoned => Ok(ReconcileOutcome::Deleted),
        }
    }

    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        if DurableObjectRepository::new(self.storage).has_live_objects(resource.id)? {
            return Err(PlatformError::new(
                ErrorCode::DoNamespaceNotEmpty,
                "Durable Object namespace still contains live objects",
            ));
        }
        Ok(())
    }

    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError> {
        self.begin_delete(resource)
    }

    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError> {
        self.verify(resource)?;
        Ok(ResourceHealth {
            availability: ResourceAvailability::Healthy,
            code: None,
        })
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Durable Object namespace lifecycle invariant failed",
    )
}

#[cfg(test)]
#[path = "durable_objects_tests.rs"]
mod tests;
