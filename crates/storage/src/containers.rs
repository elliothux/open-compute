//! Container image, application, and execution-lease authority in control.sqlite.

use crate::control_db::ControlDb;
use open_compute_core::containers::{ContainerLeaseState, ContainerResources};
use open_compute_core::{
    DurableObjectId, ErrorCode, InstanceId, PlatformError, ResourceId, VersionId,
};
use rusqlite::{Connection, OptionalExtension as _, params};

mod applications;
mod deletion;
mod directory_restores;
pub use directory_restores::ContainerDirectoryRestore;
mod leases;
mod managed;
mod management;
mod model;
mod owner;
mod registry;
mod rollouts;
mod snapshots;
pub use snapshots::CONTAINER_SNAPSHOT_RETENTION_MS;
mod snapshot_model;
pub use snapshot_model::*;

pub use management::validate_container_images;
pub(crate) use management::{
    insert_version_declarations, require_container_policy_associations,
    require_ready_container_namespaces,
};
pub use model::*;
pub use registry::{ContainerRegistryBlob, validate_registry_tag};

/// Synchronous authority; provider and artifact I/O must run outside its transactions.
#[derive(Clone, Copy, Debug)]
pub struct ContainerAuthority<'a> {
    db: &'a ControlDb,
}

impl<'a> ContainerAuthority<'a> {
    /// Bind one instance's control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }
}

fn require_instance(conn: &Connection, instance: InstanceId) -> Result<(), PlatformError> {
    let found: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM instance_identity WHERE instance_id=?1)",
            [instance],
            |row| row.get(0),
        )
        .map_err(|_| invariant())?;
    if !found {
        return Err(not_found());
    }
    Ok(())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Container authority invariant failed",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "Container authority was not found",
    )
}

fn valid_opaque(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_application_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
#[path = "containers_tests.rs"]
mod tests;

/// Decode one canonical immutable OCI registry reference without accepting mutable tags.
pub fn registry_image_digest(reference: &str) -> Result<[u8; 32], PlatformError> {
    let (prefix, digest) = reference.rsplit_once("@sha256:").ok_or_else(invariant)?;
    if prefix.is_empty()
        || prefix.len() > 512
        || prefix.bytes().any(|b| !b.is_ascii_graphic())
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invariant());
    }
    hex::decode(digest)
        .map_err(|_| invariant())?
        .try_into()
        .map_err(|_| invariant())
}
