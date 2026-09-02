//! Synthetic binding authority for authenticated operator data APIs.

use open_compute_core::{
    AccountId, BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DeploymentId,
    ErrorCode, PlatformError, ResourceId,
};
use open_compute_storage::{
    AuthorizedBinding, DeploymentBindingRecord, PlatformStorage, ResourceRepository,
};

/// Build a full-permission operator binding for one persisted resource.
pub(crate) fn operator_binding(
    storage: &PlatformStorage,
    account_id: AccountId,
    resource_id: ResourceId,
    kind: BindingKind,
) -> Result<AuthorizedBinding, PlatformError> {
    let resource = ResourceRepository::new(storage.db()).get(account_id, resource_id)?;
    if resource.kind != kind {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "resource kind does not match operator request",
        ));
    }
    Ok(AuthorizedBinding {
        binding: DeploymentBindingRecord {
            id: BindingId::generate(),
            deployment_id: DeploymentId::generate(),
            name: "__operator__".to_owned(),
            kind,
            resource_id,
            resource_spec_generation: resource.spec_generation,
            capability_version: 1,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
            descriptor_sha256: [0; 32],
            created_at_ms: 0,
        },
        resource,
        account_id,
    })
}
