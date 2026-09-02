//! Immutable deployment cache policy and platform-provided binding metadata.

use crate::ControlDb;
use open_compute_core::{DeploymentId, ErrorCode, PlatformError};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

/// Persisted cache policy for the default export or one named entrypoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentCachePolicyRecord {
    /// Absent identifies the deployment default policy.
    pub entrypoint: Option<String>,
    /// Whether automatic response caching is active.
    pub enabled: bool,
    /// Whether automatic entries are shared across deployment versions.
    pub cross_version_cache: bool,
}

/// Platform-provided immutable environment binding kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinBindingKind {
    /// Workers AI binding limited to the verified Markdown Conversion surface.
    Ai,
    /// Local Images transformation session factory.
    Images,
    /// Frozen deployment version metadata object.
    VersionMetadata,
}

impl BuiltinBindingKind {
    /// Current-schema database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ai => "ai",
            Self::Images => "images",
            Self::VersionMetadata => "version_metadata",
        }
    }

    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "ai" => Ok(Self::Ai),
            "images" => Ok(Self::Images),
            "version_metadata" => Ok(Self::VersionMetadata),
            _ => Err(invariant()),
        }
    }
}

/// Immutable platform-provided binding row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentBuiltinBindingRecord {
    /// Tenant environment name.
    pub name: String,
    /// Platform binding kind.
    pub kind: BuiltinBindingKind,
    /// Optional deployment tag, only for Version Metadata.
    pub tag: Option<String>,
    /// SHA-256 of the canonical binding descriptor.
    pub descriptor_sha256: [u8; 32],
}

/// Insert cache and platform-binding metadata inside an existing deployment transaction.
pub(crate) fn insert_runtime_features(
    tx: &rusqlite::Transaction<'_>,
    deployment_id: DeploymentId,
    cache: &[DeploymentCachePolicyRecord],
    bindings: &[DeploymentBuiltinBindingRecord],
) -> Result<(), PlatformError> {
    for policy in cache {
        tx.execute(
            "INSERT INTO deployment_cache_policies
             (deployment_id, entrypoint_name, enabled, cross_version_cache)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                deployment_id.to_string(),
                policy.entrypoint.as_deref().unwrap_or(""),
                policy.enabled,
                policy.cross_version_cache,
            ],
        )
        .map_err(|_| invariant())?;
    }
    for binding in bindings {
        tx.execute(
            "INSERT INTO deployment_builtin_bindings
             (deployment_id, binding_name, kind, tag, descriptor_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                deployment_id.to_string(),
                binding.name,
                binding.kind.as_str(),
                binding.tag,
                binding.descriptor_sha256.as_slice(),
            ],
        )
        .map_err(|_| invariant())?;
    }
    Ok(())
}

pub(crate) fn read_cache_policies_conn(
    connection: &Connection,
    deployment_id: DeploymentId,
) -> Result<Vec<DeploymentCachePolicyRecord>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT entrypoint_name, enabled, cross_version_cache
             FROM deployment_cache_policies
             WHERE deployment_id = ?1 ORDER BY entrypoint_name",
        )
        .map_err(|_| invariant())?;
    let rows = statement
        .query_map([deployment_id.to_string()], |row| {
            let entrypoint: String = row.get(0)?;
            Ok(DeploymentCachePolicyRecord {
                entrypoint: (!entrypoint.is_empty()).then_some(entrypoint),
                enabled: row.get(1)?,
                cross_version_cache: row.get(2)?,
            })
        })
        .map_err(|_| invariant())?;
    rows.map(|row| row.map_err(|_| invariant())).collect()
}

pub(crate) fn read_builtin_bindings_conn(
    connection: &Connection,
    deployment_id: DeploymentId,
) -> Result<Vec<DeploymentBuiltinBindingRecord>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT binding_name, kind, tag, descriptor_sha256
             FROM deployment_builtin_bindings
             WHERE deployment_id = ?1 ORDER BY binding_name",
        )
        .map_err(|_| invariant())?;
    let rows = statement
        .query_map([deployment_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(|_| invariant())?;
    let mut result = Vec::new();
    for row in rows {
        let (name, kind, tag, digest) = row.map_err(|_| invariant())?;
        result.push(DeploymentBuiltinBindingRecord {
            name,
            kind: BuiltinBindingKind::parse(&kind)?,
            tag,
            descriptor_sha256: digest.try_into().map_err(|_| invariant())?,
        });
    }
    Ok(result)
}

/// Read immutable runtime features for one deployment.
pub fn deployment_runtime_features(
    db: &ControlDb,
    deployment_id: DeploymentId,
) -> Result<
    (
        Vec<DeploymentCachePolicyRecord>,
        Vec<DeploymentBuiltinBindingRecord>,
    ),
    PlatformError,
> {
    db.with_read(|connection| {
        Ok((
            read_cache_policies_conn(connection, deployment_id)?,
            read_builtin_bindings_conn(connection, deployment_id)?,
        ))
    })
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "deployment runtime feature invariant failed",
    )
}
