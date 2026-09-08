//! Namespace-local SQLite KV engine.

use super::KvNamespaceRecord;
use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use rusqlite::blob::Blob;
use rusqlite::{Connection, Error as SqlError, ErrorCode as SqlErrorCode, MAIN_DB, OpenFlags};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Static adapter and storage capability version.
pub const KV_CAPABILITY_VERSION: u32 = 1;
/// Maximum UTF-8 bytes in a key.
pub const KV_MAX_KEY_BYTES: usize = 512;
/// Maximum bytes in one value.
pub const KV_MAX_VALUE_BYTES: usize = 25 * 1024 * 1024;
/// Maximum bytes in canonical metadata JSON.
pub const KV_MAX_METADATA_BYTES: usize = 1024;
/// Maximum keys in one multi-get.
pub const KV_MAX_MULTI_GET_KEYS: usize = 100;
/// Maximum bytes in one aggregate multi-get response.
pub const KV_MAX_MULTI_GET_RESPONSE_BYTES: usize = 25 * 1024 * 1024;
/// Default list page size.
pub const KV_DEFAULT_LIST_LIMIT: u16 = 1000;
/// Maximum list page size.
pub const KV_MAX_LIST_LIMIT: u16 = 1000;
/// Minimum relative expiration in seconds.
pub const KV_MIN_EXPIRATION_TTL_SECONDS: u64 = 60;
/// Minimum accepted compatibility-only cache TTL in seconds.
pub const KV_MIN_CACHE_TTL_SECONDS: u64 = 30;
/// Namespace SQLite schema version.
pub const KV_SCHEMA_VERSION: u32 = 1;

const FORMAT: &[u8] = b"open-compute-kv";
const DATABASE_FILE_MODE: u32 = 0o600;

/// Canonical, already-validated mutation options.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KvPutOptions {
    /// Absolute backend Unix time in milliseconds.
    pub expires_at_ms: Option<i64>,
    /// Canonical JSON bytes; `Some(b"null")` differs from no metadata.
    pub metadata_json: Option<Vec<u8>>,
}

/// One value and metadata read from a single SQLite snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KvEntry {
    /// Exact stored bytes.
    pub value: Vec<u8>,
    /// Canonical JSON bytes when metadata was present.
    pub metadata_json: Option<Vec<u8>>,
    /// Absolute expiry in milliseconds.
    pub expires_at_ms: Option<i64>,
}

/// Metadata announced before a streamed value body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KvEntryInfo {
    /// Exact stored value length.
    pub value_length: usize,
    /// Canonical JSON bytes when metadata was present.
    pub metadata_json: Option<Vec<u8>>,
    /// Absolute expiry in milliseconds.
    pub expires_at_ms: Option<i64>,
}

/// One list result row, ordered by raw UTF-8 bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvListRow {
    /// UTF-8 key bytes.
    pub key: Vec<u8>,
    /// Canonical JSON metadata bytes.
    pub metadata_json: Option<Vec<u8>>,
    /// Absolute expiry in milliseconds.
    pub expires_at_ms: Option<i64>,
}

/// One keyset-paginated SQLite snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KvListPage {
    /// At most the requested number of live keys.
    pub rows: Vec<KvListRow>,
    /// Whether no further live key was observed in this snapshot.
    pub complete: bool,
}

/// Direct engine for one immutable namespace identity.
#[derive(Clone, Debug)]
pub struct KvEngine {
    path: PathBuf,
    account_id: AccountId,
    resource_id: ResourceId,
    quota_bytes: u64,
}

mod repository;

/// Validate a key at the authoritative Rust boundary.
pub fn validate_key(key: &str) -> Result<Vec<u8>, PlatformError> {
    let bytes = key.as_bytes();
    if key.is_empty() || key == "." || key == ".." {
        return Err(PlatformError::new(
            ErrorCode::KvKeyInvalid,
            "KV key is outside the supported grammar",
        ));
    }
    if bytes.len() > KV_MAX_KEY_BYTES {
        return Err(PlatformError::new(
            ErrorCode::KvKeyTooLarge,
            "KV key exceeds the 512-byte limit",
        ));
    }
    Ok(bytes.to_vec())
}

/// Canonically serialize JSON-compatible metadata with lexicographic object keys.
pub fn canonical_metadata(value: &Value) -> Result<Vec<u8>, PlatformError> {
    let canonical = canonical_value(value)?;
    let bytes = serde_json::to_vec(&canonical).map_err(|_| metadata_invalid())?;
    if bytes.len() > KV_MAX_METADATA_BYTES {
        return Err(PlatformError::new(
            ErrorCode::KvMetadataTooLarge,
            "KV metadata exceeds the 1024-byte limit",
        ));
    }
    Ok(bytes)
}

fn canonical_value(value: &Value) -> Result<Value, PlatformError> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(value.clone()),
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => Ok(value.clone()),
        Value::Number(_) => Err(metadata_invalid()),
        Value::Array(values) => values
            .iter()
            .map(canonical_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => {
            let mut ordered = serde_json::Map::new();
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            for key in keys {
                ordered.insert(key.clone(), canonical_value(&values[key])?);
            }
            Ok(Value::Object(ordered))
        }
    }
}

fn validate_put_options(options: &KvPutOptions, now_ms: i64) -> Result<(), PlatformError> {
    if now_ms < 0
        || options
            .expires_at_ms
            .is_some_and(|expires| expires < now_ms.saturating_add(60_000))
    {
        return Err(PlatformError::new(
            ErrorCode::KvInvalidOptions,
            "KV expiration is outside the supported range",
        ));
    }
    validate_stored_metadata(options.metadata_json.as_deref())
}

fn validate_stored_metadata(metadata: Option<&[u8]>) -> Result<(), PlatformError> {
    let Some(metadata) = metadata else {
        return Ok(());
    };
    if metadata.len() > KV_MAX_METADATA_BYTES {
        return Err(corrupt());
    }
    let parsed: Value = serde_json::from_slice(metadata).map_err(|_| corrupt())?;
    if canonical_metadata(&parsed).map_err(|_| corrupt())? != metadata {
        return Err(corrupt());
    }
    Ok(())
}

fn verify_identity(
    conn: &Connection,
    account: AccountId,
    resource: ResourceId,
) -> Result<(), PlatformError> {
    for (key, expected) in [
        ("format", FORMAT.to_vec()),
        ("schema_version", KV_SCHEMA_VERSION.to_string().into_bytes()),
        ("account_id", account.to_string().into_bytes()),
        ("resource_id", resource.to_string().into_bytes()),
    ] {
        let actual: Vec<u8> = conn
            .query_row("SELECT value FROM kv_meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .map_err(map_sql)?;
        if actual != expected {
            return Err(corrupt());
        }
    }
    Ok(())
}

fn verify_schema(conn: &Connection) -> Result<(), PlatformError> {
    let entries: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'kv_entries'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql)?;
    if entries.is_none_or(|sql| {
        !sql.contains("id INTEGER PRIMARY KEY")
            || !sql.contains("key BLOB NOT NULL UNIQUE")
            || !sql.to_ascii_uppercase().contains("STRICT")
    }) {
        return Err(corrupt());
    }
    Ok(())
}

fn quick_check_conn(conn: &Connection) -> Result<(), PlatformError> {
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(map_sql)?;
    if result != "ok" {
        return Err(corrupt());
    }
    Ok(())
}

fn apply_quota(conn: &Connection, quota_bytes: u64) -> Result<(), PlatformError> {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(map_sql)?;
    if page_size == 0 {
        return Err(corrupt());
    }
    let pages = quota_bytes / page_size;
    conn.pragma_update(None, "max_page_count", pages)
        .map_err(map_sql)
}

fn ensure_within_quota(conn: &Connection, quota_bytes: u64) -> Result<(), PlatformError> {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(map_sql)?;
    let page_count: u64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(map_sql)?;
    if page_size
        .checked_mul(page_count)
        .is_none_or(|bytes| bytes > quota_bytes)
    {
        return Err(PlatformError::new(
            ErrorCode::KvStorageFull,
            "KV database exceeds the frozen namespace quota",
        ));
    }
    Ok(())
}

fn copy_exact_bounded<R: Read>(
    reader: &mut R,
    blob: &mut Blob<'_>,
    length: usize,
) -> Result<(), PlatformError> {
    let mut copied = 0_usize;
    let mut buffer = [0_u8; 64 * 1024];
    while copied < length {
        let wanted = buffer.len().min(length - copied);
        let count = reader
            .read(&mut buffer[..wanted])
            .map_err(|_| storage_unavailable())?;
        if count == 0 {
            return Err(PlatformError::new(
                ErrorCode::KvInternalProtocolError,
                "KV staged value ended before its declared length",
            ));
        }
        blob.write_all(&buffer[..count])
            .map_err(|_| storage_unavailable())?;
        copied += count;
    }
    let mut extra = [0_u8; 1];
    if reader.read(&mut extra).map_err(|_| storage_unavailable())? != 0 {
        return Err(value_too_large());
    }
    Ok(())
}

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut next = prefix.to_vec();
    for index in (0..next.len()).rev() {
        if next[index] != u8::MAX {
            next[index] += 1;
            next.truncate(index + 1);
            return Some(next);
        }
    }
    None
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the callback contract transfers ownership of this value"
)]
fn map_sql(error: SqlError) -> PlatformError {
    match error {
        SqlError::SqliteFailure(inner, _) => match inner.code {
            SqlErrorCode::DatabaseCorrupt | SqlErrorCode::NotADatabase => corrupt(),
            SqlErrorCode::DatabaseBusy | SqlErrorCode::DatabaseLocked => {
                PlatformError::new(ErrorCode::KvBusy, "KV namespace is temporarily busy")
            }
            SqlErrorCode::DiskFull => PlatformError::new(
                ErrorCode::KvStorageFull,
                "KV namespace storage quota was reached",
            ),
            _ => storage_unavailable(),
        },
        _ => storage_unavailable(),
    }
}

fn metadata_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvMetadataInvalid,
        "KV metadata is not canonical JSON-compatible data",
    )
}

fn value_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvValueTooLarge,
        "KV value exceeds the 25 MiB limit",
    )
}

fn response_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvResponseTooLarge,
        "KV aggregate response exceeds the fixed byte limit",
    )
}

fn cursor_invalid() -> PlatformError {
    PlatformError::new(ErrorCode::KvCursorInvalid, "KV list cursor is invalid")
}

fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvCorrupt,
        "KV namespace database failed an integrity invariant",
    )
}

fn storage_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvUnavailable,
        "KV namespace storage is unavailable",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "KV namespace identity invariant failed",
    )
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
