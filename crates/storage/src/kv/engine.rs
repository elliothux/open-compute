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

impl KvEngine {
    /// Initialize a new closed database in a private staging directory.
    pub fn create(
        path: &Path,
        account_id: AccountId,
        resource_id: ResourceId,
        created_at_ms: i64,
        quota_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if quota_bytes < 256 * 1024 * 1024 || created_at_ms < 0 {
            return Err(invariant());
        }
        let parent = path.parent().ok_or_else(invariant)?;
        fs::validate_owned_dir(parent)?;
        fs::ensure_file_secure(path)?;
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql)?;
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             CREATE TABLE kv_meta (
               key TEXT PRIMARY KEY,
               value BLOB NOT NULL
             ) STRICT, WITHOUT ROWID;
             CREATE TABLE kv_entries (
               id INTEGER PRIMARY KEY,
               key BLOB NOT NULL UNIQUE CHECK(length(key) BETWEEN 1 AND 512),
               value BLOB NOT NULL CHECK(length(value) <= 26214400),
               metadata_json BLOB CHECK(metadata_json IS NULL OR length(metadata_json) <= 1024),
               expires_at_ms INTEGER CHECK(expires_at_ms IS NULL OR expires_at_ms > 0),
               updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
             ) STRICT;
             CREATE INDEX kv_entries_expiration ON kv_entries(expires_at_ms, id)
             WHERE expires_at_ms IS NOT NULL;",
        )
        .map_err(map_sql)?;
        ensure_within_quota(&conn, quota_bytes)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        for (key, value) in [
            ("format", FORMAT.to_vec()),
            ("schema_version", KV_SCHEMA_VERSION.to_string().into_bytes()),
            ("resource_id", resource_id.to_string().into_bytes()),
            ("account_id", account_id.to_string().into_bytes()),
            ("created_at_ms", created_at_ms.to_string().into_bytes()),
        ] {
            tx.execute(
                "INSERT INTO kv_meta(key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(map_sql)?;
        }
        tx.commit().map_err(map_sql)?;
        apply_quota(&conn, quota_bytes)?;
        quick_check_conn(&conn)?;
        conn.execute_batch("PRAGMA optimize;").map_err(map_sql)?;
        drop(conn);
        fs::chmod(path, DATABASE_FILE_MODE)?;
        fs::validate_owned_file(path, true)?;
        let file = fs::open_nofollow(path, false, true)?;
        fs::validate_authority_fd(&file)?;
        file.sync_all().map_err(|_| storage_unavailable())?;
        drop(file);
        fs::fsync_dir(parent)?;
        Ok(Self {
            path: path.to_path_buf(),
            account_id,
            resource_id,
            quota_bytes,
        })
    }

    /// Build an engine from a verified live product catalog row.
    pub fn from_record(path: PathBuf, record: &KvNamespaceRecord) -> Result<Self, PlatformError> {
        if record.schema_version != KV_SCHEMA_VERSION
            || record.resource.driver_schema_version != KV_SCHEMA_VERSION
        {
            return Err(invariant());
        }
        let engine = Self {
            path,
            account_id: record.resource.account_id,
            resource_id: record.resource.id,
            quota_bytes: record.quota_bytes,
        };
        engine.verify()?;
        Ok(engine)
    }

    /// Copy a verified closed backup into a new namespace identity.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        source: &Path,
        destination: &Path,
        source_account: AccountId,
        source_resource: ResourceId,
        new_account: AccountId,
        new_resource: ResourceId,
        backup_id: &str,
        created_at_ms: i64,
        quota_bytes: u64,
    ) -> Result<Self, PlatformError> {
        fs::validate_owned_file(source, true)?;
        let source_conn = Connection::open_with_flags(
            source,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql)?;
        verify_identity(&source_conn, source_account, source_resource)?;
        verify_schema(&source_conn)?;
        quick_check_conn(&source_conn)?;
        drop(source_conn);
        let parent = destination.parent().ok_or_else(invariant)?;
        fs::validate_owned_dir(parent)?;
        if destination.exists() || std::fs::symlink_metadata(destination).is_ok() {
            return Err(invariant());
        }
        let mut input = fs::open_nofollow(source, false, false)?;
        let mut output = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(DATABASE_FILE_MODE)
            .open(destination)
            .map_err(|_| storage_unavailable())?;
        std::io::copy(&mut input, &mut output).map_err(|_| storage_unavailable())?;
        output.sync_all().map_err(|_| storage_unavailable())?;
        drop(output);
        fs::chmod(destination, DATABASE_FILE_MODE)?;
        let mut conn = Connection::open_with_flags(
            destination,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql)?;
        ensure_within_quota(&conn, quota_bytes)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        if uuid::Uuid::parse_str(backup_id)
            .ok()
            .is_none_or(|id| id.hyphenated().to_string() != backup_id)
        {
            return Err(invariant());
        }
        for (key, value) in [
            ("account_id", new_account.to_string().into_bytes()),
            ("resource_id", new_resource.to_string().into_bytes()),
            ("created_at_ms", created_at_ms.to_string().into_bytes()),
        ] {
            if tx
                .execute(
                    "UPDATE kv_meta SET value = ?1 WHERE key = ?2",
                    params![value, key],
                )
                .map_err(map_sql)?
                != 1
            {
                return Err(corrupt());
            }
        }
        tx.execute(
            "INSERT INTO kv_meta(key, value) VALUES ('restore_backup_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [backup_id.as_bytes()],
        )
        .map_err(map_sql)?;
        tx.commit().map_err(map_sql)?;
        apply_quota(&conn, quota_bytes)?;
        verify_identity(&conn, new_account, new_resource)?;
        quick_check_conn(&conn)?;
        drop(conn);
        let file = fs::open_nofollow(destination, false, true)?;
        file.sync_all().map_err(|_| storage_unavailable())?;
        fs::fsync_dir(parent)?;
        Ok(Self {
            path: destination.to_path_buf(),
            account_id: new_account,
            resource_id: new_resource,
            quota_bytes,
        })
    }

    /// Return the immutable backup identity used for restore-as-new, if any.
    pub fn restore_backup_id(&self) -> Result<Option<String>, PlatformError> {
        let conn = self.open_connection(false)?;
        let value = conn
            .query_row(
                "SELECT value FROM kv_meta WHERE key = 'restore_backup_id'",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(map_sql)?;
        value
            .map(|value| {
                let value = String::from_utf8(value).map_err(|_| corrupt())?;
                if uuid::Uuid::parse_str(&value)
                    .ok()
                    .is_none_or(|id| id.hyphenated().to_string() != value)
                {
                    return Err(corrupt());
                }
                Ok(value)
            })
            .transpose()
    }

    /// Verify secure path, embedded identity, schema, and quota.
    pub fn verify(&self) -> Result<(), PlatformError> {
        let conn = self.open_connection(false)?;
        verify_identity(&conn, self.account_id, self.resource_id)?;
        verify_schema(&conn)?;
        Ok(())
    }

    /// Run `PRAGMA quick_check` and exact identity validation.
    pub fn quick_check(&self) -> Result<(), PlatformError> {
        let conn = self.open_connection(false)?;
        verify_identity(&conn, self.account_id, self.resource_id)?;
        quick_check_conn(&conn)
    }

    /// Read one live entry in one SQLite snapshot.
    pub fn get(&self, key: &str, now_ms: i64) -> Result<Option<KvEntry>, PlatformError> {
        let key = validate_key(key)?;
        let mut conn = self.open_connection(false)?;
        let tx = conn.transaction().map_err(map_sql)?;
        let row = tx
            .query_row(
                "SELECT id, length(value), metadata_json, expires_at_ms
                 FROM kv_entries WHERE key = ?1
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?2)",
                params![key, now_ms],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sql)?;
        let Some((row_id, length, metadata_json, expires_at_ms)) = row else {
            tx.commit().map_err(map_sql)?;
            return Ok(None);
        };
        let length = usize::try_from(length).map_err(|_| corrupt())?;
        if length > KV_MAX_VALUE_BYTES {
            return Err(corrupt());
        }
        validate_stored_metadata(metadata_json.as_deref())?;
        let mut value = vec![0_u8; length];
        {
            let mut blob = tx
                .blob_open(MAIN_DB, "kv_entries", "value", row_id, true)
                .map_err(map_sql)?;
            if blob.len() != length {
                return Err(corrupt());
            }
            blob.read_exact(&mut value).map_err(|_| corrupt())?;
        }
        tx.commit().map_err(map_sql)?;
        Ok(Some(KvEntry {
            value,
            metadata_json,
            expires_at_ms,
        }))
    }

    /// Stream one live entry from its read snapshot without buffering the value.
    ///
    /// `announce` runs exactly once before any value chunk. A missing entry is
    /// announced as `None`. Returning an error from either callback cancels the
    /// read and releases the SQLite blob, transaction, and connection.
    pub fn stream_get(
        &self,
        key: &str,
        now_ms: i64,
        announce: impl FnOnce(Option<KvEntryInfo>) -> Result<(), PlatformError>,
        mut emit: impl FnMut(&[u8]) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        let key = validate_key(key)?;
        let mut conn = self.open_connection(false)?;
        let tx = conn.transaction().map_err(map_sql)?;
        let row = tx
            .query_row(
                "SELECT id, length(value), metadata_json, expires_at_ms
                 FROM kv_entries WHERE key = ?1
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?2)",
                params![key, now_ms],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sql)?;
        let Some((row_id, length, metadata_json, expires_at_ms)) = row else {
            announce(None)?;
            tx.commit().map_err(map_sql)?;
            return Ok(());
        };
        let length = usize::try_from(length).map_err(|_| corrupt())?;
        if length > KV_MAX_VALUE_BYTES {
            return Err(corrupt());
        }
        validate_stored_metadata(metadata_json.as_deref())?;
        announce(Some(KvEntryInfo {
            value_length: length,
            metadata_json,
            expires_at_ms,
        }))?;
        {
            let mut blob = tx
                .blob_open(MAIN_DB, "kv_entries", "value", row_id, true)
                .map_err(map_sql)?;
            if blob.len() != length {
                return Err(corrupt());
            }
            let mut remaining = length;
            let mut buffer = [0_u8; 64 * 1024];
            while remaining > 0 {
                let count = buffer.len().min(remaining);
                blob.read_exact(&mut buffer[..count])
                    .map_err(|_| corrupt())?;
                emit(&buffer[..count])?;
                remaining -= count;
            }
        }
        tx.commit().map_err(map_sql)
    }

    /// Read up to 100 keys in one snapshot with an aggregate response bound.
    pub fn get_many(
        &self,
        keys: &[String],
        now_ms: i64,
    ) -> Result<Vec<Option<KvEntry>>, PlatformError> {
        if keys.len() > KV_MAX_MULTI_GET_KEYS {
            return Err(PlatformError::new(
                ErrorCode::KvTooManyKeys,
                "KV multi-get exceeds the fixed key count",
            ));
        }
        let validated = keys
            .iter()
            .map(|key| validate_key(key))
            .collect::<Result<Vec<_>, _>>()?;
        let mut conn = self.open_connection(false)?;
        let tx = conn.transaction().map_err(map_sql)?;
        let mut statement = tx
            .prepare(
                "SELECT id, length(value), metadata_json, expires_at_ms
                 FROM kv_entries WHERE key = ?1
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?2)",
            )
            .map_err(map_sql)?;
        let mut entries = Vec::with_capacity(keys.len());
        let mut aggregate = 0_usize;
        for key in validated {
            let row = statement
                .query_row(params![key, now_ms], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                })
                .optional()
                .map_err(map_sql)?;
            let Some((row_id, length, metadata_json, expires_at_ms)) = row else {
                entries.push(None);
                continue;
            };
            let length = usize::try_from(length).map_err(|_| corrupt())?;
            aggregate = aggregate
                .checked_add(length)
                .and_then(|value| value.checked_add(metadata_json.as_ref().map_or(0, Vec::len)))
                .and_then(|value| value.checked_add(32))
                .ok_or_else(response_too_large)?;
            if aggregate > KV_MAX_MULTI_GET_RESPONSE_BYTES {
                return Err(response_too_large());
            }
            validate_stored_metadata(metadata_json.as_deref())?;
            let mut value = vec![0_u8; length];
            {
                let mut blob = tx
                    .blob_open(MAIN_DB, "kv_entries", "value", row_id, true)
                    .map_err(map_sql)?;
                blob.read_exact(&mut value).map_err(|_| corrupt())?;
            }
            entries.push(Some(KvEntry {
                value,
                metadata_json,
                expires_at_ms,
            }));
        }
        drop(statement);
        tx.commit().map_err(map_sql)?;
        Ok(entries)
    }

    /// Atomically replace one value using `zeroblob` plus incremental BLOB I/O.
    pub fn put(
        &self,
        key: &str,
        value: &[u8],
        options: &KvPutOptions,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.put_reader(
            key,
            &mut std::io::Cursor::new(value),
            value.len(),
            options,
            now_ms,
        )
    }

    /// Stream a known-size staged value into a single atomic transaction.
    pub fn put_reader<R: Read>(
        &self,
        key: &str,
        reader: &mut R,
        length: usize,
        options: &KvPutOptions,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let key = validate_key(key)?;
        if length > KV_MAX_VALUE_BYTES {
            return Err(value_too_large());
        }
        validate_put_options(options, now_ms)?;
        let length_i64 = i64::try_from(length).map_err(|_| value_too_large())?;
        let mut conn = self.open_connection(true)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        let existing = tx
            .query_row("SELECT id FROM kv_entries WHERE key = ?1", [&key], |row| {
                row.get::<_, i64>(0)
            })
            .optional()
            .map_err(map_sql)?;
        let row_id = if let Some(row_id) = existing {
            tx.execute(
                "UPDATE kv_entries SET value = zeroblob(?1), metadata_json = ?2,
                        expires_at_ms = ?3, updated_at_ms = ?4 WHERE id = ?5",
                params![
                    length_i64,
                    options.metadata_json,
                    options.expires_at_ms,
                    now_ms,
                    row_id,
                ],
            )
            .map_err(map_sql)?;
            row_id
        } else {
            tx.query_row(
                "INSERT INTO kv_entries
                 (key, value, metadata_json, expires_at_ms, updated_at_ms)
                 VALUES (?1, zeroblob(?2), ?3, ?4, ?5) RETURNING id",
                params![
                    key,
                    length_i64,
                    options.metadata_json,
                    options.expires_at_ms,
                    now_ms,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_sql)?
        };
        {
            let mut blob = tx
                .blob_open(MAIN_DB, "kv_entries", "value", row_id, false)
                .map_err(map_sql)?;
            copy_exact_bounded(reader, &mut blob, length)?;
        }
        tx.commit().map_err(map_sql)
    }

    /// Idempotently delete one key in its own immediate transaction.
    pub fn delete(&self, key: &str) -> Result<(), PlatformError> {
        let key = validate_key(key)?;
        let mut conn = self.open_connection(true)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        tx.execute("DELETE FROM kv_entries WHERE key = ?1", [key])
            .map_err(map_sql)?;
        tx.commit().map_err(map_sql)
    }

    /// List one keyset page ordered strictly by UTF-8 bytes.
    pub fn list(
        &self,
        prefix: &str,
        after_key: Option<&[u8]>,
        limit: u16,
        now_ms: i64,
    ) -> Result<KvListPage, PlatformError> {
        if limit == 0 || limit > KV_MAX_LIST_LIMIT {
            return Err(PlatformError::new(
                ErrorCode::KvInvalidOptions,
                "KV list limit is outside the supported range",
            ));
        }
        if prefix.len() > KV_MAX_KEY_BYTES {
            return Err(PlatformError::new(
                ErrorCode::KvKeyTooLarge,
                "KV list prefix exceeds the 512-byte limit",
            ));
        }
        let lower = prefix.as_bytes().to_vec();
        let upper = prefix_successor(&lower);
        if let Some(after) = after_key
            && (after.len() > KV_MAX_KEY_BYTES || std::str::from_utf8(after).is_err())
        {
            return Err(cursor_invalid());
        }
        let conn = self.open_connection(false)?;
        let requested = i64::from(limit) + 1;
        let mut statement = conn
            .prepare(
                "SELECT key, metadata_json, expires_at_ms FROM kv_entries
                 WHERE key >= ?1 AND (?2 IS NULL OR key < ?2)
                   AND (?3 IS NULL OR key > ?3)
                   AND (expires_at_ms IS NULL OR expires_at_ms > ?4)
                 ORDER BY key LIMIT ?5",
            )
            .map_err(map_sql)?;
        let rows = statement
            .query_map(params![lower, upper, after_key, now_ms, requested], |row| {
                Ok(KvListRow {
                    key: row.get(0)?,
                    metadata_json: row.get(1)?,
                    expires_at_ms: row.get(2)?,
                })
            })
            .map_err(map_sql)?;
        let mut page = Vec::new();
        for row in rows {
            let row = row.map_err(map_sql)?;
            if row.key.is_empty()
                || row.key.len() > KV_MAX_KEY_BYTES
                || std::str::from_utf8(&row.key).is_err()
            {
                return Err(corrupt());
            }
            validate_stored_metadata(row.metadata_json.as_deref())?;
            page.push(row);
        }
        let complete = page.len() <= usize::from(limit);
        page.truncate(usize::from(limit));
        Ok(KvListPage {
            rows: page,
            complete,
        })
    }

    /// Reclaim one bounded batch of logically expired rows.
    pub fn gc_expired(&self, now_ms: i64, batch: u16) -> Result<u32, PlatformError> {
        if batch == 0 || batch > 256 {
            return Err(PlatformError::new(
                ErrorCode::KvInvalidOptions,
                "KV GC batch is outside the supported range",
            ));
        }
        let mut conn = self.open_connection(true)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        let changed = tx
            .execute(
                "DELETE FROM kv_entries WHERE id IN (
                   SELECT id FROM kv_entries WHERE expires_at_ms IS NOT NULL
                     AND expires_at_ms <= ?1 ORDER BY expires_at_ms, id LIMIT ?2
                 )",
                params![now_ms, i64::from(batch)],
            )
            .map_err(map_sql)?;
        tx.commit().map_err(map_sql)?;
        u32::try_from(changed).map_err(|_| invariant())
    }

    /// Run an explicit WAL checkpoint before eviction, deletion, or backup.
    pub fn checkpoint(&self, truncate: bool) -> Result<(), PlatformError> {
        let conn = self.open_connection(true)?;
        conn.execute_batch(if truncate {
            "PRAGMA wal_checkpoint(TRUNCATE);"
        } else {
            "PRAGMA wal_checkpoint(PASSIVE);"
        })
        .map_err(map_sql)
    }

    /// Return the current WAL sidecar size without following an unexpected link.
    pub fn wal_bytes(&self) -> Result<u64, PlatformError> {
        let mut name = self.path.as_os_str().to_os_string();
        name.push("-wal");
        match std::fs::symlink_metadata(PathBuf::from(name)) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(metadata.len()),
            Ok(_) => Err(invariant()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(_) => Err(PlatformError::new(
                ErrorCode::KvUnavailable,
                "KV WAL metadata is unavailable",
            )),
        }
    }

    /// Produce a consistent standalone SQLite backup using the online API.
    pub fn online_backup(&self, destination: &Path) -> Result<(), PlatformError> {
        let parent = destination.parent().ok_or_else(invariant)?;
        fs::validate_owned_dir(parent)?;
        fs::ensure_file_secure(destination)?;
        let source = self.open_connection(false)?;
        let mut target = Connection::open_with_flags(
            destination,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql)?;
        {
            let backup = rusqlite::backup::Backup::new(&source, &mut target).map_err(map_sql)?;
            backup
                .run_to_completion(128, Duration::from_millis(1), None)
                .map_err(map_sql)?;
        }
        quick_check_conn(&target)?;
        verify_identity(&target, self.account_id, self.resource_id)?;
        drop(target);
        fs::chmod(destination, DATABASE_FILE_MODE)?;
        let file = fs::open_nofollow(destination, false, true)?;
        file.sync_all().map_err(|_| storage_unavailable())?;
        fs::fsync_dir(parent)
    }

    fn open_connection(&self, write: bool) -> Result<Connection, PlatformError> {
        fs::validate_owned_file(&self.path, true)?;
        let fd = fs::open_nofollow(&self.path, false, write)?;
        fs::validate_authority_fd(&fd)?;
        drop(fd);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&self.path, flags).map_err(map_sql)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(map_sql)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA busy_timeout = 5000;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -8192;",
        )
        .map_err(map_sql)?;
        apply_quota(&conn, self.quota_bytes)?;
        verify_identity(&conn, self.account_id, self.resource_id)?;
        verify_schema(&conn)?;
        Ok(conn)
    }
}

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

#[allow(clippy::needless_pass_by_value)]
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
