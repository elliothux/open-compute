//! Resource lifecycle controller over durable authority and a static driver.

use crate::ResourcePins;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, RequestId, ResourceAvailability, ResourceId,
    ResourceState,
};
use open_compute_storage::{
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRecord,
    ResourceRepository,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Driver reconciliation observation for a durable lifecycle row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconcileOutcome {
    /// Physical identity is absent and may be created.
    Absent,
    /// Physical identity and schema are verified and ready.
    Ready,
    /// Live identity is inaccessible and deletion may be finalized.
    Deleted,
    /// Durable product intent exists but requires its product controller to resume.
    Deferred,
}

/// Stable driver health observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceHealth {
    /// Persisted availability state.
    pub availability: ResourceAvailability,
    /// Stable reason for degraded or unavailable state.
    pub code: Option<&'static str>,
}

impl ResourceHealth {
    /// Healthy probe result.
    #[must_use]
    pub const fn healthy() -> Self {
        Self {
            availability: ResourceAvailability::Healthy,
            code: None,
        }
    }
}

/// Compile-time product lifecycle driver contract.
///
/// Implementations are ordinary Rust types selected by composition. Runtime
/// plugin loading and tenant-supplied driver code are not supported.
pub trait ResourceDriver: Send + Sync {
    /// Product kind implemented by this driver.
    fn kind(&self) -> BindingKind;
    /// Product identity mixed into create idempotency without exposing secret material.
    fn create_fingerprint_material(&self) -> Vec<u8> {
        Vec::new()
    }
    /// Create or publish the physical identity idempotently.
    fn create(&self, resource: &ResourceRecord) -> Result<(), PlatformError>;
    /// Probe durable state after startup or a lost response.
    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError>;
    /// Fence or quarantine the physical identity idempotently.
    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError>;
    /// Verify and finish physical deletion idempotently.
    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError>;
    /// Probe resource-local health without changing lifecycle.
    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError>;
}

/// Resource create primitive consumed by product-specific controllers.
#[derive(Clone, Debug)]
pub struct CreateResourceRequest {
    /// Owning account.
    pub account_id: AccountId,
    /// Product kind.
    pub kind: BindingKind,
    /// Display name.
    pub name: String,
    /// Required idempotency key.
    pub idempotency_key: String,
    /// Product driver schema version.
    pub driver_schema_version: u32,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Current wall-clock milliseconds.
    pub now_ms: i64,
}

/// Canonical resource-create result persisted for replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResourceResult {
    /// Immutable resource identity.
    pub resource_id: ResourceId,
    /// Ready lifecycle state.
    pub state: ResourceState,
}

/// New result or exact persisted response bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreateResourceOutcome {
    /// Driver reconciliation completed now.
    Applied(CreateResourceResult),
    /// Same completed operation returned its exact response bytes.
    Replay(Vec<u8>),
}

/// Direct lifecycle orchestrator parameterized by one static product driver.
#[derive(Debug)]
pub struct ResourceController<'a, D> {
    storage: &'a PlatformStorage,
    pins: ResourcePins,
    driver: D,
}

impl<'a, D: ResourceDriver> ResourceController<'a, D> {
    /// Bind durable authority, process-local pins, and one product driver.
    #[must_use]
    pub fn new(storage: &'a PlatformStorage, pins: ResourcePins, driver: D) -> Self {
        Self {
            storage,
            pins,
            driver,
        }
    }

    /// Reserve identity, reconcile physical create, and persist ready state.
    pub fn create(
        &self,
        request: &CreateResourceRequest,
    ) -> Result<CreateResourceOutcome, PlatformError> {
        if request.kind != self.driver.kind() || request.driver_schema_version == 0 {
            return Err(PlatformError::new(
                ErrorCode::BindingTypeMismatch,
                "resource driver does not implement the requested kind",
            ));
        }
        let fingerprint_input =
            create_fingerprint(request, &self.driver.create_fingerprint_material())?;
        let fingerprint = self
            .storage
            .crypto()
            .fingerprint_request(&fingerprint_input);
        let repository = ResourceRepository::new(self.storage.db());
        let reservation = repository.reserve_create(&ReserveResourceCreate {
            account_id: request.account_id,
            kind: request.kind,
            name: &request.name,
            idempotency_key: &request.idempotency_key,
            fingerprint_key_id: self.storage.crypto().fingerprint_key_id(),
            request_fingerprint: &fingerprint,
            resource_id: ResourceId::generate(),
            driver_schema_version: request.driver_schema_version,
            request_id: request.request_id,
            now_ms: request.now_ms,
            expires_at_ms: request.now_ms.saturating_add(IDEMPOTENCY_TTL_MS),
        })?;
        let resource = match reservation {
            ResourceCreateReservation::Complete(response) => {
                return Ok(CreateResourceOutcome::Replay(response));
            }
            ResourceCreateReservation::Failed(response) => {
                return Err(replayed_create_failure(&response));
            }
            ResourceCreateReservation::Reserved(resource)
            | ResourceCreateReservation::Continue(resource) => resource,
        };
        let resource = match self.reconcile_create(resource.clone(), request.now_ms) {
            Ok(resource) => resource,
            Err(error) => {
                if matches!(
                    self.driver.reconcile(&resource),
                    Ok(ReconcileOutcome::Absent)
                ) {
                    repository.fail_create(
                        request.account_id,
                        &request.idempotency_key,
                        &fingerprint,
                        resource.id,
                        error.code(),
                        request.request_id,
                        request.now_ms,
                    )?;
                }
                return Err(error);
            }
        };
        let result = CreateResourceResult {
            resource_id: resource.id,
            state: resource.state,
        };
        let response = serde_json::to_vec(&result).map_err(|_| invariant())?;
        repository.complete_create(
            request.account_id,
            &request.idempotency_key,
            &fingerprint,
            resource.id,
            &response,
        )?;
        Ok(CreateResourceOutcome::Applied(result))
    }

    /// Read one resource in this product driver scope.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<ResourceRecord, PlatformError> {
        let resource = ResourceRepository::new(self.storage.db()).get(account_id, resource_id)?;
        if resource.kind != self.driver.kind() {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotFound,
                "resource was not found in the driver scope",
            ));
        }
        Ok(resource)
    }

    /// List every live or tombstoned resource in this product driver scope.
    pub fn list(&self, account_id: AccountId) -> Result<Vec<ResourceRecord>, PlatformError> {
        ResourceRepository::new(self.storage.db()).list(account_id, Some(self.driver.kind()))
    }

    /// Rename one resource without changing its physical identity or generation.
    pub fn rename(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        name: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<ResourceRecord, PlatformError> {
        self.get(account_id, resource_id)?;
        ResourceRepository::new(self.storage.db()).rename(
            account_id,
            resource_id,
            name,
            request_id,
            now_ms,
        )
    }

    /// Fence calls, recheck referrers, and converge physical and durable delete.
    pub async fn delete(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        request_id: RequestId,
        now_ms: i64,
        drain_deadline: Duration,
    ) -> Result<(), PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        let resource = repository.get(account_id, resource_id)?;
        if resource.kind != self.driver.kind() {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotFound,
                "resource was not found in the driver scope",
            ));
        }
        if !repository.referrers(resource_id)?.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ResourceReferenced,
                "resource still has retained referrers",
            ));
        }
        self.pins
            .fence_and_wait(resource_id, drain_deadline)
            .await?;
        let operation = (|| {
            repository.begin_delete(account_id, resource_id, now_ms)?;
            let deleting = repository.get(account_id, resource_id)?;
            self.driver.begin_delete(&deleting)?;
            self.driver.finalize_delete(&deleting)?;
            repository.mark_tombstoned(account_id, resource_id, request_id, now_ms)
        })();
        if operation.is_ok() {
            self.pins.retire_fence(resource_id);
        } else {
            self.pins.unfence(resource_id);
        }
        operation
    }

    /// Reconcile every durable creating/deleting row owned by this driver.
    pub fn reconcile_pending(
        &self,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<u32, PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        let mut reconciled = 0_u32;
        for resource in repository.reconcile_candidates()? {
            if resource.kind != self.driver.kind() {
                continue;
            }
            match resource.state {
                ResourceState::Creating => match self.reconcile_create(resource, now_ms) {
                    Ok(_) => {}
                    Err(error) if error.code() == ErrorCode::ResourceNotReady => continue,
                    Err(error) => return Err(error),
                },
                ResourceState::Deleting => {
                    self.driver.begin_delete(&resource)?;
                    self.driver.finalize_delete(&resource)?;
                    repository.mark_tombstoned(
                        resource.account_id,
                        resource.id,
                        request_id,
                        now_ms,
                    )?;
                }
                ResourceState::Ready | ResourceState::Tombstoned => continue,
            }
            reconciled = reconciled.saturating_add(1);
        }
        Ok(reconciled)
    }

    /// Probe and persist resource-local health.
    pub fn refresh_health(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<ResourceRecord, PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        let resource = repository.get(account_id, resource_id)?;
        if resource.kind != self.driver.kind() || resource.state != ResourceState::Ready {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "resource cannot be health-probed in this lifecycle",
            ));
        }
        let health = self.driver.health(&resource)?;
        repository.set_availability(
            account_id,
            resource_id,
            health.availability,
            health.code,
            now_ms,
        )
    }

    fn reconcile_create(
        &self,
        resource: ResourceRecord,
        now_ms: i64,
    ) -> Result<ResourceRecord, PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        if resource.state == ResourceState::Ready {
            return Ok(resource);
        }
        if resource.state != ResourceState::Creating {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "resource create cannot continue from this lifecycle",
            ));
        }
        match self.driver.reconcile(&resource)? {
            ReconcileOutcome::Absent => self.driver.create(&resource)?,
            ReconcileOutcome::Ready => {}
            ReconcileOutcome::Deleted => return Err(invariant()),
            ReconcileOutcome::Deferred => {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotReady,
                    "resource create requires its product controller to resume",
                ));
            }
        }
        if self.driver.reconcile(&resource)? != ReconcileOutcome::Ready {
            return Err(invariant());
        }
        repository.mark_ready(resource.id, now_ms)?;
        repository.get(resource.account_id, resource.id)
    }
}

fn create_fingerprint(
    request: &CreateResourceRequest,
    product_material: &[u8],
) -> Result<[u8; 32], PlatformError> {
    let mut digest = Sha256::new();
    digest.update(b"open-compute/resource-create/v1\0");
    digest.update(request.account_id.as_uuid().as_bytes());
    frame(&mut digest, request.kind.as_str().as_bytes())?;
    frame(&mut digest, request.name.as_bytes())?;
    digest.update(request.driver_schema_version.to_be_bytes());
    if !product_material.is_empty() {
        frame(&mut digest, product_material)?;
    }
    Ok(digest.finalize().into())
}

fn replayed_create_failure(bytes: &[u8]) -> PlatformError {
    let code = serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|value| value.get("code")?.as_str().map(str::to_owned));
    match code.as_deref() {
        Some("RESOURCE_NAME_CONFLICT") => PlatformError::new(
            ErrorCode::ResourceNameConflict,
            "resource create previously failed with a name conflict",
        ),
        Some("DO_CLASS_NOT_FOUND") => PlatformError::new(
            ErrorCode::DoClassNotFound,
            "resource create previously failed class validation",
        ),
        _ => invariant(),
    }
}

fn frame(digest: &mut Sha256, value: &[u8]) -> Result<(), PlatformError> {
    let length = u64::try_from(value.len()).map_err(|_| invariant())?;
    digest.update(length.to_be_bytes());
    digest.update(value);
    Ok(())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "resource driver reconciliation invariant failed",
    )
}

#[cfg(test)]
#[path = "resource_lifecycle_tests.rs"]
mod tests;
