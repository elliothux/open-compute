//! Account-scoped durable R2 multipart upload authority.

use crate::{ControlDb, r2::valid_ssec_key_md5};
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use rusqlite::{OptionalExtension, params};
use std::fmt;
use std::str::FromStr as _;

/// Lifecycle of one tenant-visible multipart upload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum R2MultipartState {
    /// Catalog reserved; provider create may still be in flight.
    Initiating,
    /// Provider create may have succeeded but its response was not observed.
    CreateUnknown,
    /// Parts may still be uploaded.
    Open,
    /// Complete is in flight or awaiting reconciliation.
    Completing,
    /// Object has been committed.
    Completed,
    /// Abort is in flight or awaiting reconciliation.
    Aborting,
    /// Upload was aborted.
    Aborted,
}

impl R2MultipartState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Initiating => "initiating",
            Self::CreateUnknown => "create_unknown",
            Self::Open => "open",
            Self::Completing => "completing",
            Self::Completed => "completed",
            Self::Aborting => "aborting",
            Self::Aborted => "aborted",
        }
    }

    fn parse(value: &str) -> Result<Self, PlatformError> {
        match value {
            "initiating" => Ok(Self::Initiating),
            "create_unknown" => Ok(Self::CreateUnknown),
            "open" => Ok(Self::Open),
            "completing" => Ok(Self::Completing),
            "completed" => Ok(Self::Completed),
            "aborting" => Ok(Self::Aborting),
            "aborted" => Ok(Self::Aborted),
            _ => Err(invariant()),
        }
    }
}

/// Durable multipart upload mapping. SSE-C plaintext is never stored.
#[derive(Clone, Eq, PartialEq)]
pub struct R2MultipartUploadRecord {
    /// Tenant-visible upload id.
    pub upload_id: String,
    /// Owning logical bucket.
    pub resource_id: ResourceId,
    /// Owning account.
    pub account_id: AccountId,
    /// Exact object key.
    pub object_key: String,
    /// Provider multipart id, absent only while initiating.
    pub provider_upload_id: Option<String>,
    /// Worker API storage class token.
    pub storage_class: String,
    /// Canonical HTTP metadata JSON.
    pub http_metadata: String,
    /// Canonical custom metadata JSON.
    pub custom_metadata: String,
    /// Public `ssecKeyMd5` when the upload is SSE-C.
    pub ssec_key_md5: Option<String>,
    /// AEAD envelope JSON for the SSE-C key. Never plaintext.
    pub ssec_envelope: Option<String>,
    /// Object version allocated at create.
    pub object_version: String,
    /// Canonical exact ordered completion request, once completion starts.
    pub completion_manifest: Option<String>,
    /// Canonical completed object metadata, once completion commits.
    pub completed_metadata: Option<String>,
    /// Current lifecycle state.
    pub state: R2MultipartState,
}

impl fmt::Debug for R2MultipartUploadRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("R2MultipartUploadRecord")
            .field("upload_id", &self.upload_id)
            .field("resource_id", &self.resource_id)
            .field("account_id", &self.account_id)
            .field("object_key", &self.object_key)
            .field("provider_upload_id", &self.provider_upload_id)
            .field("storage_class", &self.storage_class)
            .field("ssec_key_md5", &self.ssec_key_md5)
            .field(
                "ssec_envelope",
                &self.ssec_envelope.as_ref().map(|_| "present"),
            )
            .field("object_version", &self.object_version)
            .field(
                "completion_manifest",
                &self.completion_manifest.as_ref().map(|_| "present"),
            )
            .field(
                "completed_metadata",
                &self.completed_metadata.as_ref().map(|_| "present"),
            )
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// One stored multipart part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct R2MultipartPartRecord {
    /// Part number in `1..=10000`.
    pub part_number: i32,
    /// Provider part `ETag`.
    pub etag: String,
    /// Part size in bytes.
    pub size: u64,
}

/// Typed repository for multipart upload rows.
#[derive(Clone, Copy, Debug)]
pub struct R2MultipartRepository<'a> {
    db: &'a ControlDb,
}

mod repository;

fn map_upload(row: &rusqlite::Row<'_>) -> rusqlite::Result<R2MultipartUploadRecord> {
    let record = R2MultipartUploadRecord {
        upload_id: row.get(0)?,
        resource_id: ResourceId::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&row.get::<_, String>(2)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_key: row.get(3)?,
        provider_upload_id: row.get(4)?,
        storage_class: row.get(5)?,
        http_metadata: row.get(6)?,
        custom_metadata: row.get(7)?,
        ssec_key_md5: row.get(8)?,
        ssec_envelope: row.get(9)?,
        object_version: row.get(10)?,
        completion_manifest: row.get(11)?,
        completed_metadata: row.get(12)?,
        state: R2MultipartState::parse(&row.get::<_, String>(13)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    };
    if !valid_upload_record(&record) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(record)
}

fn valid_upload_record(record: &R2MultipartUploadRecord) -> bool {
    if record.ssec_key_md5.is_some() != record.ssec_envelope.is_some()
        || !valid_ssec_key_md5(record.ssec_key_md5.as_deref())
        || (record.state != R2MultipartState::Initiating
            && record.state != R2MultipartState::CreateUnknown
            && record.provider_upload_id.is_none())
    {
        return false;
    }
    let manifest_expected = matches!(
        record.state,
        R2MultipartState::Completing | R2MultipartState::Completed
    );
    if manifest_expected != record.completion_manifest.is_some()
        || (record.state == R2MultipartState::Completed) != record.completed_metadata.is_some()
    {
        return false;
    }
    if let Some(raw) = record.completion_manifest.as_deref() {
        let Ok(parts) = serde_json::from_str::<Vec<StoredCompletionPart>>(raw) else {
            return false;
        };
        if parts.is_empty() || serde_json::to_string(&parts).ok().as_deref() != Some(raw) {
            return false;
        }
        let mut previous = 0_i64;
        for part in parts {
            if !(1..=10_000).contains(&part.part_number)
                || part.part_number <= previous
                || part.etag.is_empty()
            {
                return false;
            }
            previous = part.part_number;
        }
    }
    if let Some(raw) = record.completed_metadata.as_deref() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return false;
        };
        if value.get("key").and_then(serde_json::Value::as_str) != Some(record.object_key.as_str())
            || value.get("version").and_then(serde_json::Value::as_str)
                != Some(record.object_version.as_str())
        {
            return false;
        }
    }
    true
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCompletionPart {
    part_number: i64,
    etag: String,
}

fn read_upload(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
    upload_id: &str,
) -> Result<Option<R2MultipartUploadRecord>, PlatformError> {
    match conn
        .query_row(
            "SELECT upload_id, resource_id, account_id, object_key, provider_upload_id,
                storage_class, http_metadata, custom_metadata, ssec_key_md5, ssec_envelope,
                object_version, completion_manifest, completed_metadata, state
         FROM r2_multipart_uploads
         WHERE upload_id = ?1 AND account_id = ?2 AND resource_id = ?3",
            params![upload_id, account_id.to_string(), resource_id.to_string()],
            map_upload,
        )
        .optional()
    {
        Ok(record) => Ok(record),
        Err(rusqlite::Error::InvalidQuery) => Err(invariant()),
        Err(_) => Err(db_error()),
    }
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 multipart authority invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "R2 multipart catalog is unavailable")
}

fn multipart_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::R2MultipartInvalid,
        "R2 multipart upload is invalid",
    )
}
