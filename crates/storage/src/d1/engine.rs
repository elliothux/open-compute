//! D1 database identity and public execution value types.

use super::D1DatabaseRecord;
use crate::fs;
use open_compute_core::{AccountId, D1Config, ErrorCode, PlatformError, ResourceId};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// On-disk D1 format version.
pub const D1_DATABASE_SCHEMA_VERSION: u32 = 1;
/// Maximum UTF-8 bytes in one prepared SQL string.
pub const D1_MAX_SQL_BYTES: usize = 100_000;
/// Maximum SQL transfer body generated or ingested by one D1 operation.
pub const D1_MAX_TRANSFER_SQL_BYTES: usize = 64 * 1024 * 1024;
/// Maximum SQLite parameter slots.
pub const D1_MAX_BOUND_PARAMS: usize = 100;
/// Maximum result or schema columns.
pub const D1_MAX_COLUMNS: usize = 100;
/// Maximum bytes in one value or materialized row.
pub const D1_MAX_VALUE_OR_ROW_BYTES: usize = 2_000_000;
/// Maximum statements represented by the private D1 batch frame.
pub const D1_MAX_BATCH_STATEMENTS: usize = u16::MAX as usize;
/// Maximum statements that can fit inside one maximum-length SQL input.
pub const D1_MAX_EXEC_STATEMENTS: usize = D1_MAX_SQL_BYTES;

const INTERNAL_SCHEMA: &str = "
CREATE TABLE __open_compute_meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TABLE __open_compute_migrations (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  applied_at_ms INTEGER NOT NULL,
  UNIQUE(name, sha256)
) STRICT;";

/// One normalized value accepted by the private D1 protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum D1Value {
    /// SQLite NULL.
    Null,
    /// SQLite INTEGER.
    Integer(i64),
    /// SQLite REAL; callers must reject non-finite values.
    Real(f64),
    /// SQLite UTF-8 TEXT.
    Text(String),
    /// SQLite BLOB transported as bounded binary bytes.
    Blob(Vec<u8>),
}

impl D1Value {
    pub(crate) fn byte_len(&self) -> usize {
        match self {
            Self::Null => 1,
            Self::Integer(_) | Self::Real(_) => 8,
            Self::Text(value) => value.len(),
            Self::Blob(value) => value.len(),
        }
    }
}

/// Flat statement DTO sent only for a terminal operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct D1Statement {
    /// One non-empty SQLite statement.
    pub sql: String,
    /// Positionally bound values.
    pub params: Vec<D1Value>,
}

/// Operator limits applied to one terminal operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct D1QueryLimits {
    /// Maximum materialized rows.
    pub max_result_rows: usize,
    /// Maximum encoded result bytes.
    pub max_result_bytes: usize,
    /// Maximum approximate VM opcodes.
    pub max_vm_steps: u64,
    /// Wall deadline.
    pub timeout: Duration,
}

impl D1QueryLimits {
    /// Build query limits from validated configuration.
    pub fn query(config: &D1Config) -> Result<Self, PlatformError> {
        Self::from_config(config, config.query_timeout_ms)
    }

    /// Build shared batch limits from validated configuration.
    pub fn batch(config: &D1Config) -> Result<Self, PlatformError> {
        Self::from_config(config, config.batch_timeout_ms)
    }

    fn from_config(config: &D1Config, timeout_ms: u64) -> Result<Self, PlatformError> {
        Ok(Self {
            max_result_rows: usize::try_from(config.max_result_rows).map_err(|_| limit_error())?,
            max_result_bytes: usize::try_from(config.max_result_bytes)
                .map_err(|_| limit_error())?,
            max_vm_steps: config.max_vm_steps,
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

/// SQL execution timings nested under `D1Meta.timings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct D1QueryTimings {
    /// Duration of SQL execution by the database instance, excluding transport.
    pub sql_duration_ms: f64,
}

/// D1-compatible local execution metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct D1Meta {
    /// SQLite compile/step/materialization wall milliseconds.
    pub duration: f64,
    /// Logical page count times page size.
    pub size_after: u64,
    /// Local estimate equal to materialized output rows.
    pub rows_read: u64,
    /// Local estimate equal to SQLite changes.
    pub rows_written: u64,
    /// Last SQLite rowid converted to JavaScript Number later.
    pub last_row_id: i64,
    /// Whether SQLite classified the statement as non-readonly.
    pub changed_db: bool,
    /// Rows changed by this statement.
    pub changes: u64,
    /// Optional region identity; omitted on the single local primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub served_by_region: Option<String>,
    /// Optional colo identity; omitted on the single local primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub served_by_colo: Option<String>,
    /// True when the local primary executed the query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub served_by_primary: Option<bool>,
    /// Optional SQL-only timings for the last attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings: Option<D1QueryTimings>,
    /// Attempt count including automatic retries; local execution is one attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_attempts: Option<u32>,
}

impl D1Meta {
    /// Local-primary metadata with required core fields and stable optional locals.
    #[must_use]
    pub fn local(
        duration: f64,
        changes: u64,
        last_row_id: i64,
        changed_db: bool,
        size_after: u64,
        rows_read: u64,
        rows_written: u64,
    ) -> Self {
        Self {
            duration,
            size_after,
            rows_read,
            rows_written,
            last_row_id,
            changed_db,
            changes,
            served_by_region: None,
            served_by_colo: None,
            served_by_primary: Some(true),
            timings: Some(D1QueryTimings {
                sql_duration_ms: duration,
            }),
            total_attempts: Some(1),
        }
    }
}

/// One statement result retaining both object and raw column order information.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct D1StatementResult {
    /// Ordered column names, including duplicates and magic names.
    pub columns: Vec<String>,
    /// Ordered row values.
    pub rows: Vec<Vec<D1Value>>,
    /// Stable execution metadata.
    pub meta: D1Meta,
}

/// `exec()` summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct D1ExecResult {
    /// Successfully executed statement count.
    pub count: u32,
    /// Total local execution wall milliseconds.
    pub duration: f64,
}

/// One ordered control-plane migration request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D1Migration {
    /// Positive contiguous migration identity.
    pub id: u32,
    /// Stable filename-like display name.
    pub name: String,
    /// Caller-supplied SHA-256 which must match `sql`.
    pub sha256: [u8; 32],
    /// SQL parsed by SQLite's tail pointer.
    pub sql: String,
}

/// Persisted migration ledger row, excluding SQL text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct D1MigrationRecord {
    /// Migration identity.
    pub id: u32,
    /// Stable name.
    pub name: String,
    /// Lowercase SHA-256.
    pub sha256: String,
    /// Successful commit time.
    pub applied_at_ms: i64,
}

/// Authority for one tenant SQLite file. Connections are opened per operation;
/// the service-level lane provides strict per-database serialization.
#[derive(Clone, Debug)]
pub struct D1Engine {
    pub(crate) path: PathBuf,
    pub(crate) account_id: AccountId,
    pub(crate) resource_id: ResourceId,
    pub(crate) quota_bytes: u64,
}

impl D1Engine {
    /// Create a new hardened D1 database at an unpublished staging path.
    pub fn create(
        path: &Path,
        account_id: AccountId,
        resource_id: ResourceId,
        created_at_ms: i64,
        quota_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if path.exists() || quota_bytes < 64 * 1024 * 1024 {
            return Err(identity_error());
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_open_error(&error))?;
        fs::chmod(path, 0o600)?;
        super::hardening::configure_connection(&connection, quota_bytes)?;
        connection
            .execute_batch(INTERNAL_SCHEMA)
            .map_err(map_internal_error)?;
        let values = [
            ("format", b"open-compute-d1".to_vec()),
            ("schema_version", b"1".to_vec()),
            ("resource_id", resource_id.to_string().into_bytes()),
            ("account_id", account_id.to_string().into_bytes()),
            ("created_at_ms", created_at_ms.to_string().into_bytes()),
            ("session_version", b"0".to_vec()),
        ];
        for (key, value) in values {
            connection
                .execute(
                    "INSERT INTO __open_compute_meta(key, value) VALUES (?1, ?2)",
                    params![key, value],
                )
                .map_err(map_internal_error)?;
        }
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(map_internal_error)?;
        drop(connection);
        let file = fs::open_nofollow(path, false, true)?;
        fs::validate_authority_fd(&file)?;
        file.sync_all().map_err(|_| {
            PlatformError::new(ErrorCode::PathInvalid, "failed to fsync D1 database")
        })?;
        drop(file);
        fs::fsync_dir(path.parent().ok_or_else(identity_error)?)?;
        let engine = Self {
            path: path.to_path_buf(),
            account_id,
            resource_id,
            quota_bytes,
        };
        engine.verify_identity()?;
        engine.quick_check()?;
        Ok(engine)
    }

    /// Open an existing database only after catalog and embedded identity agree.
    pub fn from_record(path: PathBuf, record: &D1DatabaseRecord) -> Result<Self, PlatformError> {
        if record.schema_version != D1_DATABASE_SCHEMA_VERSION
            || record.resource.driver_schema_version != D1_DATABASE_SCHEMA_VERSION
            || record.quota_bytes < 64 * 1024 * 1024
        {
            return Err(identity_error());
        }
        let engine = Self {
            path,
            account_id: record.resource.account_id,
            resource_id: record.resource.id,
            quota_bytes: record.quota_bytes,
        };
        engine.verify_identity()?;
        Ok(engine)
    }

    /// Verify embedded identity and format without exposing the path.
    pub fn verify_identity(&self) -> Result<(), PlatformError> {
        let connection = self.open()?;
        let expected = [
            ("format", "open-compute-d1".to_owned()),
            ("schema_version", D1_DATABASE_SCHEMA_VERSION.to_string()),
            ("resource_id", self.resource_id.to_string()),
            ("account_id", self.account_id.to_string()),
        ];
        for (key, value) in expected {
            let actual: Vec<u8> = connection
                .query_row(
                    "SELECT value FROM __open_compute_meta WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(|_| identity_error())?;
            if actual != value.as_bytes() {
                return Err(identity_error());
            }
        }
        let _ = read_session_version(&connection)?;
        Ok(())
    }

    /// Read the durable monotonic D1 session state version.
    pub fn session_version(&self) -> Result<u64, PlatformError> {
        read_session_version(&self.open()?)
    }

    /// Run a bounded fast integrity check.
    pub fn quick_check(&self) -> Result<(), PlatformError> {
        let connection = self.open()?;
        let result: String = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|error| map_open_error(&error))?;
        if result != "ok" {
            return Err(corrupt_error());
        }
        Ok(())
    }

    /// Read tenant `PRAGMA user_version` through the trusted control path.
    pub fn user_version(&self) -> Result<u32, PlatformError> {
        let connection = self.open()?;
        let value: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| map_open_error(&error))?;
        u32::try_from(value).map_err(|_| corrupt_error())
    }

    /// Force a WAL checkpoint before delete or backup lifecycle transitions.
    pub fn checkpoint(&self, truncate: bool) -> Result<(), PlatformError> {
        let connection = self.open()?;
        let sql = if truncate {
            "PRAGMA wal_checkpoint(TRUNCATE)"
        } else {
            "PRAGMA wal_checkpoint(PASSIVE)"
        };
        connection
            .execute_batch(sql)
            .map_err(|error| map_open_error(&error))
    }

    /// Return the current WAL byte length after validating the sidecar shape.
    pub fn wal_bytes(&self) -> Result<u64, PlatformError> {
        let mut name = self.path.as_os_str().to_os_string();
        name.push("-wal");
        let path = PathBuf::from(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                fs::validate_owned_file(&path, true).map_err(|_| identity_error())?;
                Ok(metadata.len())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(_) => Err(identity_error()),
        }
    }

    pub(crate) fn open(&self) -> Result<Connection, PlatformError> {
        fs::validate_owned_file(&self.path, true).map_err(|_| identity_error())?;
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_open_error(&error))?;
        super::hardening::configure_connection(&connection, self.quota_bytes)?;
        Ok(connection)
    }
}

pub(crate) fn map_open_error(error: &rusqlite::Error) -> PlatformError {
    use rusqlite::ffi::ErrorCode as SqliteCode;
    match error.sqlite_error_code() {
        Some(SqliteCode::DatabaseCorrupt | SqliteCode::NotADatabase) => corrupt_error(),
        Some(SqliteCode::DiskFull) => PlatformError::new(
            ErrorCode::D1DatabaseFull,
            "D1 database quota or disk capacity was reached",
        ),
        Some(SqliteCode::DatabaseBusy | SqliteCode::DatabaseLocked) => {
            PlatformError::new(ErrorCode::D1Overloaded, "D1 database is temporarily busy")
        }
        _ => PlatformError::new(ErrorCode::ResourceUnavailable, "D1 database is unavailable"),
    }
}

pub(crate) fn map_internal_error(_error: rusqlite::Error) -> PlatformError {
    PlatformError::new(
        ErrorCode::D1DatabaseCorrupt,
        "D1 internal schema operation failed",
    )
}

pub(crate) fn identity_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1IdentityMismatch,
        "D1 database identity does not match control authority",
    )
}

pub(crate) fn corrupt_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1DatabaseCorrupt,
        "D1 database failed integrity validation",
    )
}

pub(crate) fn read_session_version(connection: &Connection) -> Result<u64, PlatformError> {
    let actual: Vec<u8> = connection
        .query_row(
            "SELECT value FROM __open_compute_meta WHERE key = ?1",
            ["session_version"],
            |row| row.get(0),
        )
        .map_err(|_| identity_error())?;
    let text = std::str::from_utf8(&actual).map_err(|_| identity_error())?;
    text.parse::<u64>().map_err(|_| identity_error())
}

pub(crate) fn write_session_version(
    connection: &Connection,
    version: u64,
) -> Result<(), PlatformError> {
    let changed = connection
        .execute(
            "UPDATE __open_compute_meta SET value = ?1 WHERE key = ?2",
            params![version.to_string().into_bytes(), "session_version"],
        )
        .map_err(map_internal_error)?;
    if changed != 1 {
        return Err(identity_error());
    }
    Ok(())
}

pub(crate) fn bump_session_version(connection: &Connection) -> Result<u64, PlatformError> {
    let next = read_session_version(connection)?
        .checked_add(1)
        .ok_or_else(limit_error)?;
    write_session_version(connection, next)?;
    Ok(next)
}

pub(crate) fn limit_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1LimitError,
        "D1 operation exceeded a fixed limit",
    )
}
