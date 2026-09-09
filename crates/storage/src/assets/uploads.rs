//! Resumable version-upload session authority.

use crate::{ControlDb, VersionContentKind, VersionObjectKind};
use open_compute_core::{
    AccountId, ErrorCode, PlatformError, StartupId, VersionId, VersionUploadId, WorkerId,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::str::FromStr;

/// Durable version-upload session state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionUploadStatus {
    /// Objects may still be verified.
    Open,
    /// A fixed version identifier is being committed through the ordinary pipeline.
    Finalizing,
    /// The version was committed and may be queried after a lost response.
    Committed,
    /// The caller cancelled the session before finalization.
    Aborted,
    /// The unfinished session exceeded its fixed lifetime.
    Expired,
}

impl VersionUploadStatus {
    /// Stable current-schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Finalizing => "finalizing",
            Self::Committed => "committed",
            Self::Aborted => "aborted",
            Self::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "open" => Ok(Self::Open),
            "finalizing" => Ok(Self::Finalizing),
            "committed" => Ok(Self::Committed),
            "aborted" => Ok(Self::Aborted),
            "expired" => Ok(Self::Expired),
            _ => Err(invariant()),
        }
    }
}

/// One declared content-addressed object in an upload session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionUploadObjectRecord {
    /// Object digest.
    pub sha256: [u8; 32],
    /// Semantic inventory kind.
    pub kind: VersionObjectKind,
    /// Declared and verified byte length.
    pub size: u64,
    /// Whether the platform verified the actual bytes.
    pub verified: bool,
    /// Verification timestamp.
    pub verified_at_ms: Option<i64>,
}

/// Durable upload-session projection safe for authenticated control responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionUploadRecord {
    /// Session identifier.
    pub id: VersionUploadId,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Worker.
    pub worker_id: WorkerId,
    /// Caller idempotency key.
    pub idempotency_key: String,
    /// Secret-keyed normalized input fingerprint.
    pub input_fingerprint: [u8; 32],
    /// Worker or assets-only content discriminator.
    pub content_kind: VersionContentKind,
    /// Optional Worker bundle digest.
    pub bundle_sha256: Option<[u8; 32]>,
    /// Optional Worker bundle length.
    pub bundle_size: Option<u64>,
    /// Canonical asset manifest digest.
    pub manifest_sha256: [u8; 32],
    /// Canonical asset manifest length.
    pub manifest_size: u64,
    /// Canonical manifest bytes.
    pub manifest_json: Vec<u8>,
    /// Canonical asset routing bytes.
    pub routing_config_json: Vec<u8>,
    /// Current state.
    pub status: VersionUploadStatus,
    /// Fixed version identity once finalization begins.
    pub version_id: Option<VersionId>,
    /// Secret-keyed fingerprint of the write-only finalize metadata.
    pub finalize_fingerprint: Option<[u8; 32]>,
    /// Exclusive platform startup generation that most recently owned finalization.
    pub finalize_owner_startup_id: Option<StartupId>,
    /// Exact successful finalize response retained for lost-response replay.
    pub finalize_response_json: Option<Vec<u8>>,
    /// Stable terminal pipeline failure retained for lost-response replay.
    pub finalize_error_code: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Expiration timestamp for unfinished work.
    pub expires_at_ms: i64,
    /// Last durable state change.
    pub updated_at_ms: i64,
    /// Canonically ordered inventory.
    pub objects: Vec<VersionUploadObjectRecord>,
}

/// Ownership result for one serialized finalize attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionUploadFinalizeDisposition {
    /// This attempt assigned the version identity for the first time.
    Reserved,
    /// This attempt reclaimed unfinished work after a prior attempt released its lock.
    Recover,
    /// The exact operation was already committed and has a persisted response.
    Committed,
}

/// Durable upload record plus the action its finalize owner must take.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionUploadFinalize {
    /// Current durable upload authority.
    pub upload: VersionUploadRecord,
    /// Whether to create, recover, or replay the fixed version.
    pub disposition: VersionUploadFinalizeDisposition,
}

/// Durable identity and request proof used to reserve or resume one upload finalization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginVersionUploadFinalize {
    /// Owning account.
    pub account_id: AccountId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Upload session being finalized.
    pub upload_id: VersionUploadId,
    /// One fixed version identity reused by every retry.
    pub version_id: VersionId,
    /// HMAC fingerprint of canonical finalization metadata.
    pub finalize_fingerprint: [u8; 32],
    /// Platform startup generation currently recovering the operation.
    pub owner_startup_id: StartupId,
    /// Control-plane wall time.
    pub now_ms: i64,
}

/// One object declared when creating an upload session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewVersionUploadObject {
    /// Object digest.
    pub sha256: [u8; 32],
    /// Semantic object kind.
    pub kind: VersionObjectKind,
    /// Exact byte length.
    pub size: u64,
}

/// Validated upload-session creation input.
#[derive(Clone, Debug)]
pub struct NewVersionUpload<'a> {
    /// New session identifier.
    pub id: VersionUploadId,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Worker.
    pub worker_id: WorkerId,
    /// Caller idempotency key.
    pub idempotency_key: &'a str,
    /// Secret-keyed normalized input fingerprint.
    pub input_fingerprint: [u8; 32],
    /// Worker or assets-only content discriminator.
    pub content_kind: VersionContentKind,
    /// Optional Worker bundle identity.
    pub bundle: Option<([u8; 32], u64)>,
    /// Canonical asset manifest identity.
    pub manifest_sha256: [u8; 32],
    /// Canonical manifest bytes.
    pub manifest_json: &'a [u8],
    /// Canonical asset routing bytes.
    pub routing_config_json: &'a [u8],
    /// Complete deduplicated inventory, including manifest and optional bundle.
    pub objects: &'a [NewVersionUploadObject],
    /// Creation timestamp.
    pub now_ms: i64,
    /// Fixed expiration timestamp.
    pub expires_at_ms: i64,
}

/// Transactional owner for resumable upload state.
#[derive(Clone, Copy, Debug)]
pub struct VersionUploadRepository<'a> {
    db: &'a ControlDb,
}

mod repository;

fn validate_new(
    input: &NewVersionUpload<'_>,
    max_open_per_worker: u32,
    max_open_per_account: u32,
) -> Result<(), PlatformError> {
    let bundle_shape = match input.content_kind {
        VersionContentKind::Worker => input.bundle.is_some(),
        VersionContentKind::AssetsOnly => input.bundle.is_none(),
    };
    if max_open_per_worker == 0
        || max_open_per_account < max_open_per_worker
        || !bundle_shape
        || input.idempotency_key.is_empty()
        || input.idempotency_key.len() > 128
        || input.manifest_json.is_empty()
        || input.routing_config_json.is_empty()
        || input.expires_at_ms <= input.now_ms
        || input.objects.is_empty()
    {
        return Err(conflict());
    }
    let manifest_size = u64::try_from(input.manifest_json.len()).map_err(|_| conflict())?;
    if !input.objects.iter().any(|object| {
        object.kind == VersionObjectKind::AssetManifest
            && object.sha256 == input.manifest_sha256
            && object.size == manifest_size
    }) || input.bundle.is_some_and(|bundle| {
        !input.objects.iter().any(|object| {
            object.kind == VersionObjectKind::Bundle
                && object.sha256 == bundle.0
                && object.size == bundle.1
        })
    }) {
        return Err(conflict());
    }
    let mut identities = input
        .objects
        .iter()
        .map(|object| object.sha256)
        .collect::<Vec<_>>();
    identities.sort_unstable();
    if identities.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(conflict());
    }
    Ok(())
}

fn read_by_key(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
    key: &str,
) -> Result<Option<VersionUploadRecord>, PlatformError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM version_uploads
             WHERE account_id = ?1 AND worker_id = ?2 AND idempotency_key = ?3",
            params![account_id.to_string(), worker_id.to_string(), key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| db_error())?;
    id.map(|value| {
        let upload_id = VersionUploadId::from_str(&value).map_err(|_| invariant())?;
        read_tx(tx, upload_id)
    })
    .transpose()
}

fn read_tx(
    tx: &Transaction<'_>,
    upload_id: VersionUploadId,
) -> Result<VersionUploadRecord, PlatformError> {
    let mut record = tx
        .query_row(
            "SELECT id, account_id, worker_id, idempotency_key, input_fingerprint,
                    content_kind, bundle_sha256, bundle_size, manifest_sha256,
                    manifest_size, manifest_json, routing_config_json, status,
                    version_id, finalize_fingerprint, finalize_owner_startup_id,
                    finalize_response_json, finalize_error_code,
                    created_at_ms, expires_at_ms, updated_at_ms
             FROM version_uploads WHERE id = ?1",
            [upload_id.to_string()],
            map_upload,
        )
        .optional()
        .map_err(|_| db_error())?
        .ok_or_else(not_found)?;
    let mut stmt = tx
        .prepare(
            "SELECT sha256, object_kind, size, verified, verified_at_ms
             FROM version_upload_objects WHERE session_id = ?1
             ORDER BY object_kind, sha256",
        )
        .map_err(|_| db_error())?;
    let rows = stmt
        .query_map([upload_id.to_string()], map_object)
        .map_err(|_| db_error())?;
    record.objects = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| db_error())?;
    Ok(record)
}

fn map_upload(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionUploadRecord> {
    let id: String = row.get(0)?;
    let account: String = row.get(1)?;
    let worker: String = row.get(2)?;
    let fingerprint: Vec<u8> = row.get(4)?;
    let kind: String = row.get(5)?;
    let bundle_digest: Option<Vec<u8>> = row.get(6)?;
    let bundle_size: Option<i64> = row.get(7)?;
    let manifest_digest: Vec<u8> = row.get(8)?;
    let manifest_size: i64 = row.get(9)?;
    let status: String = row.get(12)?;
    let version: Option<String> = row.get(13)?;
    let finalize_fingerprint: Option<Vec<u8>> = row.get(14)?;
    let finalize_owner: Option<String> = row.get(15)?;
    Ok(VersionUploadRecord {
        id: id.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: account.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: worker.parse().map_err(|_| rusqlite::Error::InvalidQuery)?,
        idempotency_key: row.get(3)?,
        input_fingerprint: fingerprint
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        content_kind: VersionContentKind::parse(&kind)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        bundle_sha256: bundle_digest
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        bundle_size: bundle_size
            .map(u64::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        manifest_sha256: manifest_digest
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        manifest_size: u64::try_from(manifest_size).map_err(|_| rusqlite::Error::InvalidQuery)?,
        manifest_json: row.get(10)?,
        routing_config_json: row.get(11)?,
        status: VersionUploadStatus::parse(&status).map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_id: version
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        finalize_fingerprint: finalize_fingerprint
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        finalize_owner_startup_id: finalize_owner
            .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        finalize_response_json: row.get(16)?,
        finalize_error_code: row.get(17)?,
        created_at_ms: row.get(18)?,
        expires_at_ms: row.get(19)?,
        updated_at_ms: row.get(20)?,
        objects: Vec::new(),
    })
}

fn map_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionUploadObjectRecord> {
    let digest: Vec<u8> = row.get(0)?;
    let kind: String = row.get(1)?;
    let size: i64 = row.get(2)?;
    let verified: i64 = row.get(3)?;
    Ok(VersionUploadObjectRecord {
        sha256: digest
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: parse_object_kind(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        size: u64::try_from(size).map_err(|_| rusqlite::Error::InvalidQuery)?,
        verified: verified == 1,
        verified_at_ms: row.get(4)?,
    })
}

fn parse_object_kind(value: &str) -> Result<VersionObjectKind, PlatformError> {
    match value {
        "bundle" => Ok(VersionObjectKind::Bundle),
        "asset_manifest" => Ok(VersionObjectKind::AssetManifest),
        "asset_blob" => Ok(VersionObjectKind::AssetBlob),
        _ => Err(invariant()),
    }
}

fn expire_open(tx: &Transaction<'_>, now_ms: i64) -> Result<(), PlatformError> {
    tx.execute(
        "UPDATE version_uploads SET status = 'expired', updated_at_ms = ?1
         WHERE status = 'open' AND expires_at_ms <= ?1",
        [now_ms],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

fn require_live_worker(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<(), PlatformError> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM workers
             WHERE id = ?1 AND account_id = ?2 AND deleted_at_ms IS NULL",
            params![worker_id.to_string(), account_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| db_error())?;
    found.map(|_| ()).ok_or_else(not_found)
}

fn require_scope(
    record: &VersionUploadRecord,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<(), PlatformError> {
    if record.account_id == account_id && record.worker_id == worker_id {
        Ok(())
    } else {
        Err(not_found())
    }
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionNotFound,
        "version upload session was not found",
    )
}

fn incomplete() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadIncomplete,
        "version upload is missing verified objects",
    )
}

fn conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadConflict,
        "version upload conflicts with durable session state",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "version upload authority is inconsistent",
    )
}

#[cfg(test)]
#[path = "uploads_tests.rs"]
mod validation_tests;

fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::Internal,
        "version upload database operation failed",
    )
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
