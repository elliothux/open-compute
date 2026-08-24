//! Immutable deployment-binding persistence and runtime authorization.

use crate::{ControlDb, DeploymentState, ResourceRecord};
use open_compute_core::{
    AccountId, BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DeploymentId,
    ErrorCode, PlatformError, ResourceAvailability, ResourceId, ResourceState,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::str::FromStr;

/// Immutable binding row frozen into one deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentBindingRecord {
    /// Binding identity.
    pub id: BindingId,
    /// Owning immutable deployment.
    pub deployment_id: DeploymentId,
    /// Tenant environment name.
    pub name: String,
    /// Static adapter kind.
    pub kind: BindingKind,
    /// Frozen resource identity.
    pub resource_id: ResourceId,
    /// Frozen resource specification generation.
    pub resource_spec_generation: u64,
    /// Static adapter capability version.
    pub capability_version: u32,
    /// Canonical method permissions.
    pub permissions: CanonicalPermissions,
    /// Canonical kind-specific configuration.
    pub config: CanonicalBindingConfig,
    /// Digest of canonical binding descriptor bytes.
    pub descriptor_sha256: [u8; 32],
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Binding input inserted in the same transaction as a staging deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDeploymentBinding {
    /// Platform-generated binding identity.
    pub id: BindingId,
    /// Tenant environment name.
    pub name: String,
    /// Static adapter kind.
    pub kind: BindingKind,
    /// Frozen resource identity.
    pub resource_id: ResourceId,
    /// Frozen resource specification generation.
    pub resource_spec_generation: u64,
    /// Static adapter capability version.
    pub capability_version: u32,
    /// Canonical permissions bytes.
    pub permissions_json: Vec<u8>,
    /// Canonical product configuration bytes.
    pub config_json: Vec<u8>,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
}

/// Runtime authorization result derived only from persisted binding authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedBinding {
    /// Immutable binding row.
    pub binding: DeploymentBindingRecord,
    /// Current resource authority row.
    pub resource: ResourceRecord,
    /// Owning account resolved through the deployment Worker.
    pub account_id: AccountId,
}

/// Binding-specific repository over the central control database.
#[derive(Clone, Copy, Debug)]
pub struct BindingRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> BindingRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Read canonical bindings for descriptor reconstruction.
    pub fn deployment_bindings(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Vec<DeploymentBindingRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_deployment_bindings_conn(conn, deployment_id))
    }

    /// Authorize one backend call from persisted immutable authority.
    pub fn authorize(
        &self,
        binding_id: BindingId,
        deployment_id: DeploymentId,
        descriptor_sha256: &[u8; 32],
    ) -> Result<AuthorizedBinding, PlatformError> {
        self.db.with_read(|conn| {
            let row: Option<(
                DeploymentBindingRecord,
                ResourceRecord,
                AccountId,
                String,
                bool,
            )> = conn
                .query_row(
                    "SELECT b.id, b.deployment_id, b.name, b.kind, b.resource_id,
                            b.resource_spec_generation, b.capability_version,
                            b.permissions_json, b.config_json, b.descriptor_sha256, b.created_at_ms,
                            r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                            r.availability_code, r.spec_generation, r.driver_schema_version,
                            r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                            w.account_id, d.state,
                            EXISTS(SELECT 1 FROM resource_referrers rr
                              WHERE rr.resource_id = b.resource_id
                                AND rr.referrer_kind = 'deployment_binding'
                                AND rr.referrer_id = b.id)
                     FROM deployment_bindings b
                     JOIN worker_deployments d ON d.id = b.deployment_id
                     JOIN workers w ON w.id = d.worker_id
                     JOIN resources r ON r.id = b.resource_id
                     WHERE b.id = ?1 AND b.deployment_id = ?2",
                    params![binding_id.to_string(), deployment_id.to_string()],
                    |row| {
                        let binding = map_binding_offset(row, 0)?;
                        let resource = map_resource_offset(row, 11)?;
                        let account: String = row.get(23)?;
                        let deployment_state: String = row.get(24)?;
                        let referrer: bool = row.get(25)?;
                        Ok((
                            binding,
                            resource,
                            AccountId::from_str(&account)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            deployment_state,
                            referrer,
                        ))
                    },
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((binding, resource, account_id, deployment_state, referrer)) = row else {
                return Err(PlatformError::new(
                    ErrorCode::BindingNotFound,
                    "binding authority was not found",
                ));
            };
            if binding.descriptor_sha256 != *descriptor_sha256
                || deployment_state != DeploymentState::Ready.as_str()
                || binding.kind != resource.kind
                || binding.resource_spec_generation != resource.spec_generation
                || account_id != resource.account_id
                || !referrer
            {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "binding authority invariant failed",
                ));
            }
            if resource.state != ResourceState::Ready {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotReady,
                    "resource lifecycle does not admit this operation",
                ));
            }
            if resource.availability != ResourceAvailability::Healthy {
                return Err(PlatformError::new(
                    ErrorCode::ResourceUnavailable,
                    "resource is unavailable",
                ));
            }
            Ok(AuthorizedBinding {
                binding,
                resource,
                account_id,
            })
        })
    }
}

pub(crate) fn read_deployment_bindings_conn(
    conn: &rusqlite::Connection,
    deployment_id: DeploymentId,
) -> Result<Vec<DeploymentBindingRecord>, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT b.id, b.deployment_id, b.name, b.kind, b.resource_id,
                    b.resource_spec_generation, b.capability_version,
                    b.permissions_json, b.config_json, b.descriptor_sha256, b.created_at_ms,
                    r.kind, r.spec_generation, r.state, r.account_id, w.account_id,
                    EXISTS(SELECT 1 FROM resource_referrers rr
                      WHERE rr.resource_id = b.resource_id
                        AND rr.referrer_kind = 'deployment_binding'
                        AND rr.referrer_id = b.id)
             FROM deployment_bindings b
             JOIN resources r ON r.id = b.resource_id
             JOIN worker_deployments d ON d.id = b.deployment_id
             JOIN workers w ON w.id = d.worker_id
             WHERE b.deployment_id = ?1 ORDER BY b.name, b.id",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([deployment_id.to_string()], |row| {
            let binding = map_binding(row)?;
            let resource_kind: String = row.get(11)?;
            let resource_generation: i64 = row.get(12)?;
            let resource_state: String = row.get(13)?;
            let resource_account: String = row.get(14)?;
            let worker_account: String = row.get(15)?;
            let referrer: bool = row.get(16)?;
            if binding.kind.as_str() != resource_kind
                || i64::try_from(binding.resource_spec_generation)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?
                    != resource_generation
                || resource_state != ResourceState::Ready.as_str()
                || resource_account != worker_account
                || !referrer
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(binding)
        })
        .map_err(|_| db_error())?;
    collect_rows(rows)
}

pub(crate) fn insert_staging_bindings(
    tx: &Transaction<'_>,
    deployment_id: DeploymentId,
    bindings: &[NewDeploymentBinding],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for binding in bindings {
        tx.execute(
            "INSERT INTO deployment_bindings
             (id, deployment_id, name, kind, resource_id, resource_spec_generation,
              capability_version, permissions_json, config_json, descriptor_sha256,
              created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                binding.id.to_string(),
                deployment_id.to_string(),
                binding.name,
                binding.kind.as_str(),
                binding.resource_id.to_string(),
                i64::try_from(binding.resource_spec_generation).map_err(|_| binding_invariant())?,
                i64::from(binding.capability_version),
                binding.permissions_json,
                binding.config_json,
                binding.descriptor_sha256.as_slice(),
                now_ms,
            ],
        )
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::BindingTypeMismatch,
                "staging binding authority invariant failed",
            )
        })?;
    }
    Ok(())
}

fn map_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentBindingRecord> {
    map_binding_offset(row, 0)
}

fn map_binding_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<DeploymentBindingRecord> {
    let id: String = row.get(offset)?;
    let deployment: String = row.get(offset + 1)?;
    let kind: String = row.get(offset + 3)?;
    let resource: String = row.get(offset + 4)?;
    let generation: i64 = row.get(offset + 5)?;
    let capability: i64 = row.get(offset + 6)?;
    let permissions_json: Vec<u8> = row.get(offset + 7)?;
    let config_json: Vec<u8> = row.get(offset + 8)?;
    let descriptor: Vec<u8> = row.get(offset + 9)?;
    let permissions: CanonicalPermissions =
        serde_json::from_slice(&permissions_json).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let config: CanonicalBindingConfig =
        serde_json::from_slice(&config_json).map_err(|_| rusqlite::Error::InvalidQuery)?;
    if serde_json::to_vec(&permissions).map_err(|_| rusqlite::Error::InvalidQuery)?
        != permissions_json
        || serde_json::to_vec(&config).map_err(|_| rusqlite::Error::InvalidQuery)? != config_json
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(DeploymentBindingRecord {
        id: BindingId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        deployment_id: DeploymentId::from_str(&deployment)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(offset + 2)?,
        kind: BindingKind::from_str(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        resource_id: ResourceId::from_str(&resource).map_err(|_| rusqlite::Error::InvalidQuery)?,
        resource_spec_generation: u64::try_from(generation)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        capability_version: u32::try_from(capability).map_err(|_| rusqlite::Error::InvalidQuery)?,
        permissions,
        config,
        descriptor_sha256: descriptor
            .as_slice()
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(offset + 10)?,
    })
}

fn map_resource_offset(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<ResourceRecord> {
    let id: String = row.get(offset)?;
    let account: String = row.get(offset + 1)?;
    let kind: String = row.get(offset + 2)?;
    let state: String = row.get(offset + 4)?;
    let availability: String = row.get(offset + 5)?;
    let generation: i64 = row.get(offset + 7)?;
    let schema: i64 = row.get(offset + 8)?;
    Ok(ResourceRecord {
        id: ResourceId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: BindingKind::from_str(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(offset + 3)?,
        state: ResourceState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability: ResourceAvailability::from_str(&availability)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability_code: row.get(offset + 6)?,
        spec_generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        driver_schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(offset + 9)?,
        updated_at_ms: row.get(offset + 10)?,
        deleted_at_ms: row.get(offset + 11)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(|_| binding_invariant())?);
    }
    Ok(output)
}

fn binding_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "persisted deployment binding invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}

#[cfg(test)]
#[path = "bindings_tests.rs"]
mod tests;
