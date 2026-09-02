//! Immutable deployment asset metadata and object-reference authority.

use crate::{ControlDb, DeploymentState};
use open_compute_core::{AccountId, DeploymentId, ErrorCode, PlatformError, WorkerId};
use rusqlite::{OptionalExtension, Transaction, params};

mod uploads;
pub use uploads::{
    BeginDeploymentUploadFinalize, DeploymentUploadFinalize, DeploymentUploadFinalizeDisposition,
    DeploymentUploadObjectRecord, DeploymentUploadRecord, DeploymentUploadRepository,
    DeploymentUploadStatus, NewDeploymentUpload, NewDeploymentUploadObject,
};

/// One deployment's canonical static-asset authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentAssetsRecord {
    /// Owning immutable deployment.
    pub deployment_id: DeploymentId,
    /// Canonical manifest digest.
    pub manifest_sha256: [u8; 32],
    /// Canonical manifest byte length.
    pub manifest_size: u64,
    /// Canonical manifest schema.
    pub manifest_schema_version: u32,
    /// Canonical manifest JSON bytes.
    pub manifest_json: Vec<u8>,
    /// Canonical routing JSON bytes.
    pub routing_config_json: Vec<u8>,
    /// Optional tenant environment binding name.
    pub binding_name: Option<String>,
    /// Logical manifest entry count.
    pub logical_file_count: u32,
    /// Logical bytes before physical deduplication.
    pub logical_total_bytes: u64,
    /// Authority creation timestamp.
    pub created_at_ms: i64,
}

/// New static-asset metadata committed with a staging deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDeploymentAssets {
    /// Canonical manifest digest.
    pub manifest_sha256: [u8; 32],
    /// Canonical manifest bytes.
    pub manifest_json: Vec<u8>,
    /// Canonical route configuration.
    pub routing_config_json: Vec<u8>,
    /// Optional tenant environment binding.
    pub binding_name: Option<String>,
    /// Logical manifest entry count.
    pub logical_file_count: u32,
    /// Logical bytes before deduplication.
    pub logical_total_bytes: u64,
}

/// Type of an immutable content-addressed deployment object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentObjectKind {
    /// Canonical Worker bundle.
    Bundle,
    /// Canonical static-asset manifest.
    AssetManifest,
    /// Original static file bytes.
    AssetBlob,
}

impl DeploymentObjectKind {
    /// Stable current-schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bundle => "bundle",
            Self::AssetManifest => "asset_manifest",
            Self::AssetBlob => "asset_blob",
        }
    }
}

/// One object reference inserted atomically with its deployment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewDeploymentObjectRef {
    /// Semantic object kind.
    pub kind: DeploymentObjectKind,
    /// Raw SHA-256 digest.
    pub sha256: [u8; 32],
    /// Exact object length.
    pub size: u64,
}

/// Read-only repository for deployment asset authority.
#[derive(Clone, Copy, Debug)]
pub struct DeploymentAssetsRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> DeploymentAssetsRepository<'a> {
    /// Bind the repository to the current control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Read one deployment's asset metadata, if it declares assets.
    pub fn get(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Option<DeploymentAssetsRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_assets_conn(conn, deployment_id))
    }

    /// Authorize an asset backend read against a ready immutable deployment.
    pub fn authorize_ready(
        &self,
        deployment_id: DeploymentId,
        expected_descriptor_sha256: &[u8; 32],
    ) -> Result<(AccountId, WorkerId, DeploymentAssetsRecord), PlatformError> {
        self.db.with_read(|conn| {
            let state: Option<(String, Vec<u8>, String, String)> = conn
                .query_row(
                    "SELECT d.state, d.worker_code_sha256, d.worker_id, w.account_id
                     FROM worker_deployments d
                     JOIN workers w ON w.id = d.worker_id
                     WHERE d.id = ?1",
                    [deployment_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((state, descriptor, worker, account)) = state else {
                return Err(not_found());
            };
            if DeploymentState::parse(&state)? != DeploymentState::Ready
                || descriptor.as_slice() != expected_descriptor_sha256
            {
                return Err(invariant());
            }
            Ok((
                account.parse().map_err(|_| invariant())?,
                worker.parse().map_err(|_| invariant())?,
                read_assets_conn(conn, deployment_id)?.ok_or_else(not_found)?,
            ))
        })
    }

    /// Authorize one manifest member by immutable deployment, descriptor, digest, and length.
    pub fn authorize_blob(
        &self,
        deployment_id: DeploymentId,
        expected_descriptor_sha256: &[u8; 32],
        sha256: &[u8; 32],
        size: u64,
    ) -> Result<(), PlatformError> {
        self.db.with_read(|conn| {
            let size = i64::try_from(size).map_err(|_| invariant())?;
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1
                     FROM worker_deployments d
                     JOIN deployment_assets a ON a.deployment_id = d.id
                     JOIN deployment_object_refs r ON r.deployment_id = d.id
                     WHERE d.id = ?1 AND d.state = 'ready' AND d.worker_code_sha256 = ?2
                       AND r.object_kind = 'asset_blob' AND r.sha256 = ?3 AND r.size = ?4",
                    params![
                        deployment_id.to_string(),
                        expected_descriptor_sha256.as_slice(),
                        sha256.as_slice(),
                        size,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            found.map(|_| ()).ok_or_else(not_found)
        })
    }

    /// List immutable asset blob digests retained by one deployment.
    pub fn list_asset_blobs(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Vec<([u8; 32], u64)>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT sha256, size FROM deployment_object_refs
                     WHERE deployment_id = ?1 AND object_kind = 'asset_blob'
                     ORDER BY sha256",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([deployment_id.to_string()], |row| {
                    let digest: Vec<u8> = row.get(0)?;
                    let size: i64 = row.get(1)?;
                    Ok((digest, size))
                })
                .map_err(|_| db_error())?;
            let mut out = Vec::new();
            for row in rows {
                let (digest, size) = row.map_err(|_| db_error())?;
                out.push((
                    digest.try_into().map_err(|_| invariant())?,
                    u64::try_from(size).map_err(|_| db_error())?,
                ));
            }
            Ok(out)
        })
    }
}

pub(crate) fn insert_deployment_assets(
    tx: &Transaction<'_>,
    deployment_id: DeploymentId,
    assets: &NewDeploymentAssets,
    object_refs: &[NewDeploymentObjectRef],
    now_ms: i64,
) -> Result<(), PlatformError> {
    let manifest_size = i64::try_from(assets.manifest_json.len()).map_err(|_| invariant())?;
    tx.execute(
        "INSERT INTO deployment_assets
         (deployment_id, manifest_sha256, manifest_size, manifest_schema_version,
          manifest_json, routing_config_json, binding_name, logical_file_count,
          logical_total_bytes, created_at_ms)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            deployment_id.to_string(),
            assets.manifest_sha256.as_slice(),
            manifest_size,
            assets.manifest_json,
            assets.routing_config_json,
            assets.binding_name,
            i64::from(assets.logical_file_count),
            i64::try_from(assets.logical_total_bytes).map_err(|_| invariant())?,
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    for object in object_refs {
        tx.execute(
            "INSERT INTO deployment_object_refs
             (deployment_id, object_kind, sha256, size, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                deployment_id.to_string(),
                object.kind.as_str(),
                object.sha256.as_slice(),
                i64::try_from(object.size).map_err(|_| invariant())?,
                now_ms,
            ],
        )
        .map_err(|_| db_error())?;
    }
    Ok(())
}

pub(crate) fn insert_bundle_object_ref(
    tx: &Transaction<'_>,
    deployment_id: DeploymentId,
    sha256: &[u8; 32],
    size: u64,
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO deployment_object_refs
         (deployment_id, object_kind, sha256, size, created_at_ms)
         VALUES (?1, 'bundle', ?2, ?3, ?4)",
        params![
            deployment_id.to_string(),
            sha256.as_slice(),
            i64::try_from(size).map_err(|_| invariant())?,
            now_ms,
        ],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

pub(crate) fn delete_deployment_assets(
    tx: &Transaction<'_>,
    deployment_id: DeploymentId,
) -> Result<(), PlatformError> {
    tx.execute(
        "DELETE FROM deployment_object_refs WHERE deployment_id = ?1",
        [deployment_id.to_string()],
    )
    .map_err(|_| db_error())?;
    tx.execute(
        "DELETE FROM deployment_assets WHERE deployment_id = ?1",
        [deployment_id.to_string()],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

pub(crate) fn read_assets_conn(
    conn: &rusqlite::Connection,
    deployment_id: DeploymentId,
) -> Result<Option<DeploymentAssetsRecord>, PlatformError> {
    conn.query_row(
        "SELECT deployment_id, manifest_sha256, manifest_size, manifest_schema_version,
                manifest_json, routing_config_json, binding_name, logical_file_count,
                logical_total_bytes, created_at_ms
         FROM deployment_assets WHERE deployment_id = ?1",
        [deployment_id.to_string()],
        |row| {
            let id: String = row.get(0)?;
            let digest: Vec<u8> = row.get(1)?;
            let manifest_size: i64 = row.get(2)?;
            let schema: i64 = row.get(3)?;
            let file_count: i64 = row.get(7)?;
            let total_bytes: i64 = row.get(8)?;
            Ok(DeploymentAssetsRecord {
                deployment_id: id.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
                manifest_sha256: digest
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                manifest_size: manifest_size
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                manifest_schema_version: schema
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                manifest_json: row.get(4)?,
                routing_config_json: row.get(5)?,
                binding_name: row.get(6)?,
                logical_file_count: file_count
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                logical_total_bytes: total_bytes
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                created_at_ms: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(|_| db_error())
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentNotFound,
        "deployment assets were not found",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "deployment asset authority is inconsistent",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::Internal,
        "deployment asset database operation failed",
    )
}
