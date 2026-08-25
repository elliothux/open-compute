//! Durable D1 product rows in `control.sqlite`.

use crate::{ControlDb, ResourceRecord};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, ResourceId, ResourceState,
};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::str::FromStr;

/// Product-specific catalog row for one live D1 database.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct D1DatabaseRecord {
    /// Shared resource identity and lifecycle.
    pub resource: ResourceRecord,
    /// Canonical private locator.
    #[serde(skip)]
    pub storage_key: String,
    /// Tenant database format version.
    pub schema_version: u32,
    /// Frozen file quota.
    pub quota_bytes: u64,
    /// Last successful cold-open time.
    pub last_opened_at_ms: Option<i64>,
    /// Last successful quick-check time.
    pub last_quick_check_ms: Option<i64>,
    /// Last successful backup time.
    pub last_backup_at_ms: Option<i64>,
    /// Restore intent while the shared resource is creating.
    #[serde(skip)]
    pub restore_backup_id: Option<String>,
}

/// Durable D1 backup lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum D1BackupState {
    /// Backup reservation exists.
    Creating,
    /// Immutable snapshot is verified in system S3.
    Ready,
    /// Backup creation failed.
    Failed,
    /// Object deletion is in progress.
    Deleting,
    /// Backup identity is retired.
    Tombstoned,
}

impl D1BackupState {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }
}

impl FromStr for D1BackupState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "creating" => Ok(Self::Creating),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// One immutable or in-progress D1 backup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct D1BackupRecord {
    /// Backup UUID.
    pub id: String,
    /// Source D1 resource.
    pub source_resource_id: ResourceId,
    /// Durable lifecycle.
    pub state: D1BackupState,
    /// Private system object key.
    #[serde(skip)]
    pub object_key: Option<String>,
    /// Verified SHA-256.
    #[serde(skip)]
    pub sha256: Option<[u8; 32]>,
    /// Verified snapshot size.
    pub size_bytes: Option<u64>,
    /// D1 format version stored by the snapshot.
    pub d1_schema_version: u32,
    /// Tenant `PRAGMA user_version` stored by the snapshot.
    pub sqlite_user_version: u32,
    /// Reservation time.
    pub created_at_ms: i64,
    /// Terminal-state time.
    pub completed_at_ms: Option<i64>,
    /// Stable failure code.
    pub error_code: Option<String>,
}

/// Product catalog repository over the central control database.
#[derive(Clone, Copy, Debug)]
pub struct D1DatabaseRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> D1DatabaseRepository<'a> {
    /// Bind the control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Insert the immutable product locator for a creating D1 resource.
    pub fn ensure_database(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        quota_bytes: u64,
    ) -> Result<D1DatabaseRecord, PlatformError> {
        self.ensure_database_inner(resource, storage_key, schema_version, quota_bytes, None)
    }

    /// Insert the immutable locator for restore-as-new.
    pub fn ensure_restoring_database(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        quota_bytes: u64,
        backup_id: &str,
    ) -> Result<D1DatabaseRecord, PlatformError> {
        validate_uuid(backup_id)?;
        self.ensure_database_inner(
            resource,
            storage_key,
            schema_version,
            quota_bytes,
            Some(backup_id),
        )
    }

    fn ensure_database_inner(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        quota_bytes: u64,
        restore_backup_id: Option<&str>,
    ) -> Result<D1DatabaseRecord, PlatformError> {
        if resource.kind != BindingKind::D1Database
            || resource.state != ResourceState::Creating
            || schema_version == 0
            || schema_version != resource.driver_schema_version
            || quota_bytes < 64 * 1024 * 1024
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO d1_databases
                 (resource_id, storage_key, schema_version, quota_bytes, created_at_ms,
                  last_opened_at_ms, last_quick_check_ms, last_backup_at_ms, restore_backup_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, ?6)
                 ON CONFLICT(resource_id) DO NOTHING",
                params![
                    resource.id.to_string(),
                    storage_key,
                    i64::from(schema_version),
                    i64::try_from(quota_bytes).map_err(|_| invariant())?,
                    resource.created_at_ms,
                    restore_backup_id,
                ],
            )
            .map_err(|_| invariant())?;
            let record = read_database_conn(tx, resource.account_id, resource.id)?;
            if record.storage_key != storage_key
                || record.schema_version != schema_version
                || record.quota_bytes != quota_bytes
                || record.restore_backup_id.as_deref() != restore_backup_id
            {
                return Err(invariant());
            }
            Ok(record)
        })
    }

    /// Read one database while concealing cross-account identities.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<D1DatabaseRecord, PlatformError> {
        self.db
            .with_read(|conn| read_database_conn(conn, account_id, resource_id))
    }

    /// List live databases in stable display-name order.
    pub fn list(&self, account_id: AccountId) -> Result<Vec<D1DatabaseRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                        r.availability_code, r.spec_generation, r.driver_schema_version,
                        r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                        d.storage_key, d.schema_version, d.quota_bytes,
                        d.last_opened_at_ms, d.last_quick_check_ms, d.last_backup_at_ms,
                        d.restore_backup_id
                 FROM resources r JOIN d1_databases d ON d.resource_id = r.id
                 WHERE r.account_id = ?1 AND r.kind = 'd1_database'
                   AND r.state != 'tombstoned'
                 ORDER BY r.name, r.id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([account_id.to_string()], map_database)
                .map_err(|_| invariant())?;
            rows.map(|row| row.map_err(|_| invariant())).collect()
        })
    }

    /// Record a successful cold open.
    pub fn record_open(&self, resource_id: ResourceId, now_ms: i64) -> Result<(), PlatformError> {
        self.update_time(resource_id, "last_opened_at_ms", now_ms)
    }

    /// Record a successful quick-check.
    pub fn record_quick_check(
        &self,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.update_time(resource_id, "last_quick_check_ms", now_ms)
    }

    fn update_time(
        self,
        resource_id: ResourceId,
        column: &'static str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let sql = match column {
            "last_opened_at_ms" => {
                "UPDATE d1_databases SET last_opened_at_ms = ?1 WHERE resource_id = ?2"
            }
            "last_quick_check_ms" => {
                "UPDATE d1_databases SET last_quick_check_ms = ?1 WHERE resource_id = ?2"
            }
            _ => return Err(invariant()),
        };
        self.db.with_immediate(|tx| {
            if tx
                .execute(sql, params![now_ms, resource_id.to_string()])
                .map_err(|_| invariant())?
                != 1
            {
                return Err(not_found());
            }
            Ok(())
        })
    }

    /// Reserve a host-generated backup identity idempotently.
    #[allow(clippy::too_many_arguments)]
    pub fn create_backup(
        &self,
        source: ResourceId,
        backup_id: &str,
        schema_version: u32,
        sqlite_user_version: u32,
        idempotency_key: &str,
        request_fingerprint: &[u8; 32],
        now_ms: i64,
    ) -> Result<D1BackupRecord, PlatformError> {
        validate_uuid(backup_id)?;
        if schema_version == 0 || idempotency_key.is_empty() || idempotency_key.len() > 128 {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let existing: Option<(String, Vec<u8>)> = tx
                .query_row(
                    "SELECT id, request_fingerprint FROM d1_backups
                 WHERE source_resource_id = ?1 AND idempotency_key = ?2",
                    params![source.to_string(), idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| invariant())?;
            if let Some((id, fingerprint)) = existing {
                if fingerprint.as_slice() != request_fingerprint {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "D1 backup idempotency fingerprint does not match",
                    ));
                }
                return read_backup_conn(tx, &id);
            }
            tx.execute(
                "INSERT INTO d1_backups
                 (id, source_resource_id, state, object_key, sha256, size_bytes,
                  d1_schema_version, sqlite_user_version, created_at_ms, completed_at_ms,
                  error_code, idempotency_key, request_fingerprint)
                 VALUES (?1, ?2, 'creating', NULL, NULL, NULL, ?3, ?4, ?5, NULL, NULL, ?6, ?7)",
                params![
                    backup_id,
                    source.to_string(),
                    i64::from(schema_version),
                    i64::from(sqlite_user_version),
                    now_ms,
                    idempotency_key,
                    request_fingerprint.as_slice()
                ],
            )
            .map_err(|_| invariant())?;
            read_backup_conn(tx, backup_id)
        })
    }

    /// Mark an uploaded and verified snapshot ready.
    pub fn complete_backup(
        &self,
        backup_id: &str,
        object_key: &str,
        sha256: &[u8; 32],
        size_bytes: u64,
        now_ms: i64,
    ) -> Result<D1BackupRecord, PlatformError> {
        if object_key.is_empty() || object_key.contains("..") || !object_key.contains("backups/d1/")
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            if tx
                .execute(
                    "UPDATE d1_backups SET state = 'ready', object_key = ?1, sha256 = ?2,
                    size_bytes = ?3, completed_at_ms = ?4, error_code = NULL
                 WHERE id = ?5 AND state = 'creating'",
                    params![
                        object_key,
                        sha256.as_slice(),
                        i64::try_from(size_bytes).map_err(|_| invariant())?,
                        now_ms,
                        backup_id
                    ],
                )
                .map_err(|_| invariant())?
                != 1
            {
                return Err(invariant());
            }
            let record = read_backup_conn(tx, backup_id)?;
            if tx
                .execute(
                    "UPDATE d1_databases SET last_backup_at_ms = ?1 WHERE resource_id = ?2",
                    params![now_ms, record.source_resource_id.to_string()],
                )
                .map_err(|_| invariant())?
                != 1
            {
                return Err(invariant());
            }
            Ok(record)
        })
    }

    /// Mark an in-progress backup failed with a stable sanitized error code.
    pub fn fail_backup(
        &self,
        backup_id: &str,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<D1BackupRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            if tx
                .execute(
                    "UPDATE d1_backups SET state = 'failed', object_key = NULL,
                        sha256 = NULL, size_bytes = NULL, completed_at_ms = ?1,
                        error_code = ?2 WHERE id = ?3 AND state = 'creating'",
                    params![now_ms, code.as_str(), backup_id],
                )
                .map_err(|_| invariant())?
                != 1
            {
                return Err(invariant());
            }
            read_backup_conn(tx, backup_id)
        })
    }

    /// Retire a backup after its exact data and manifest objects are removed.
    pub fn tombstone_backup(
        &self,
        account_id: AccountId,
        backup_id: &str,
        now_ms: i64,
    ) -> Result<D1BackupRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let owned: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM d1_backups b JOIN resources r
                     ON r.id = b.source_resource_id
                     WHERE b.id = ?1 AND r.account_id = ?2)",
                    params![backup_id, account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            if !owned {
                return Err(not_found());
            }
            tx.execute(
                "UPDATE d1_backups SET state = 'tombstoned', object_key = NULL,
                    sha256 = NULL, size_bytes = NULL, completed_at_ms = ?1,
                    error_code = NULL
                 WHERE id = ?2 AND state != 'tombstoned'",
                params![now_ms, backup_id],
            )
            .map_err(|_| invariant())?;
            read_backup_conn(tx, backup_id)
        })
    }

    /// Read one account-scoped backup.
    pub fn get_backup(
        &self,
        account_id: AccountId,
        backup_id: &str,
    ) -> Result<D1BackupRecord, PlatformError> {
        self.db.with_read(|conn| {
            let owned: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM d1_backups b JOIN resources r
                 ON r.id = b.source_resource_id WHERE b.id = ?1 AND r.account_id = ?2)",
                    params![backup_id, account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            if !owned {
                return Err(not_found());
            }
            read_backup_conn(conn, backup_id)
        })
    }

    /// List backups for one database.
    pub fn list_backups(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<Vec<D1BackupRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT b.id, b.source_resource_id, b.state, b.object_key, b.sha256,
                        b.size_bytes, b.d1_schema_version, b.sqlite_user_version,
                        b.created_at_ms, b.completed_at_ms, b.error_code
                 FROM d1_backups b JOIN resources r ON r.id = b.source_resource_id
                 WHERE r.account_id = ?1 AND b.source_resource_id = ?2
                 ORDER BY b.created_at_ms, b.id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map(
                    params![account_id.to_string(), resource_id.to_string()],
                    map_backup,
                )
                .map_err(|_| invariant())?;
            rows.map(|row| row.map_err(|_| invariant())).collect()
        })
    }
}

fn read_database_conn(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
) -> Result<D1DatabaseRecord, PlatformError> {
    conn.query_row(
        "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                r.availability_code, r.spec_generation, r.driver_schema_version,
                r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                d.storage_key, d.schema_version, d.quota_bytes,
                d.last_opened_at_ms, d.last_quick_check_ms, d.last_backup_at_ms,
                d.restore_backup_id
         FROM resources r JOIN d1_databases d ON d.resource_id = r.id
         WHERE r.account_id = ?1 AND r.id = ?2 AND r.kind = 'd1_database'",
        params![account_id.to_string(), resource_id.to_string()],
        map_database,
    )
    .optional()
    .map_err(|_| invariant())?
    .ok_or_else(not_found)
}

fn map_database(row: &rusqlite::Row<'_>) -> rusqlite::Result<D1DatabaseRecord> {
    let schema: i64 = row.get(13)?;
    let quota: i64 = row.get(14)?;
    Ok(D1DatabaseRecord {
        resource: crate::resources::map_resource_offset(row, 0)?,
        storage_key: row.get(12)?,
        schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        quota_bytes: u64::try_from(quota).map_err(|_| rusqlite::Error::InvalidQuery)?,
        last_opened_at_ms: row.get(15)?,
        last_quick_check_ms: row.get(16)?,
        last_backup_at_ms: row.get(17)?,
        restore_backup_id: row.get(18)?,
    })
}

fn read_backup_conn(
    conn: &rusqlite::Connection,
    backup_id: &str,
) -> Result<D1BackupRecord, PlatformError> {
    conn.query_row(
        "SELECT id, source_resource_id, state, object_key, sha256, size_bytes,
                d1_schema_version, sqlite_user_version, created_at_ms, completed_at_ms,
                error_code FROM d1_backups WHERE id = ?1",
        [backup_id],
        map_backup,
    )
    .optional()
    .map_err(|_| invariant())?
    .ok_or_else(not_found)
}

fn map_backup(row: &rusqlite::Row<'_>) -> rusqlite::Result<D1BackupRecord> {
    let resource: String = row.get(1)?;
    let state: String = row.get(2)?;
    let digest: Option<Vec<u8>> = row.get(4)?;
    let size: Option<i64> = row.get(5)?;
    let schema: i64 = row.get(6)?;
    let user_version: i64 = row.get(7)?;
    Ok(D1BackupRecord {
        id: row.get(0)?,
        source_resource_id: ResourceId::from_str(&resource)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: D1BackupState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_key: row.get(3)?,
        sha256: digest
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        size_bytes: size
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        d1_schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        sqlite_user_version: u32::try_from(user_version)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(8)?,
        completed_at_ms: row.get(9)?,
        error_code: row.get(10)?,
    })
}

fn validate_uuid(value: &str) -> Result<(), PlatformError> {
    if uuid::Uuid::parse_str(value)
        .ok()
        .is_none_or(|id| id.hyphenated().to_string() != value)
    {
        return Err(invariant());
    }
    Ok(())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "D1 catalog authority invariant failed",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "D1 resource was not found")
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
