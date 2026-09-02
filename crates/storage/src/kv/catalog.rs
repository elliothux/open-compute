//! Durable KV product rows in `control.sqlite`.

use crate::catalog_page::{CatalogColumns, build_catalog_sql, record_catalog_cursor};
use crate::{
    CatalogCursor, CatalogDirection, CatalogListPage, CatalogSort, ControlDb, ResourceRecord,
    normalize_catalog_limit, search_as_resource_id,
};
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId, ResourceState};
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::Serialize;
use std::str::FromStr;

/// Product-specific catalog row for one live KV namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvNamespaceRecord {
    /// Shared P0.3 resource identity.
    pub resource: ResourceRecord,
    /// Canonical relative locator, never exposed through public HTTP.
    #[serde(skip)]
    pub storage_key: String,
    /// Namespace SQLite schema version.
    pub schema_version: u32,
    /// Frozen SQLite file quota.
    pub quota_bytes: u64,
    /// Last successful cold-open timestamp.
    pub last_opened_at_ms: Option<i64>,
    /// Last successful quick-check timestamp.
    pub last_quick_check_ms: Option<i64>,
    /// Last successful online-backup timestamp.
    pub last_backup_at_ms: Option<i64>,
    /// Durable restore intent while the shared resource is still creating.
    #[serde(skip)]
    pub restore_backup_id: Option<String>,
}

/// Durable backup lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KvBackupState {
    /// Backup row is reserved and upload has not completed.
    Creating,
    /// Immutable object and digest are verified.
    Ready,
    /// Backup creation failed without affecting the source namespace.
    Failed,
    /// Object deletion is in progress.
    Deleting,
    /// Backup identity is permanently retired.
    Tombstoned,
}

impl KvBackupState {
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

impl FromStr for KvBackupState {
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

/// One immutable or in-progress namespace backup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvBackupRecord {
    /// Backup `UUIDv7` string.
    pub id: String,
    /// Original namespace identity.
    pub source_resource_id: ResourceId,
    /// Durable backup lifecycle.
    pub state: KvBackupState,
    /// Host-generated system object key.
    #[serde(skip)]
    pub object_key: Option<String>,
    /// Verified SHA-256.
    #[serde(skip)]
    pub sha256: Option<[u8; 32]>,
    /// Verified object size.
    pub size_bytes: Option<u64>,
    /// Namespace database schema stored in the backup.
    pub kv_schema_version: u32,
    /// Reservation time.
    pub created_at_ms: i64,
    /// Terminal-state time.
    pub completed_at_ms: Option<i64>,
    /// Stable failure code only.
    pub error_code: Option<String>,
}

/// Product catalog repository over the central control database.
#[derive(Clone, Copy, Debug)]
pub struct KvNamespaceRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> KvNamespaceRepository<'a> {
    /// Bind the control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Insert the immutable product row for a creating KV resource, idempotently.
    pub fn ensure_namespace(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        quota_bytes: u64,
    ) -> Result<KvNamespaceRecord, PlatformError> {
        self.ensure_namespace_with_restore(resource, storage_key, schema_version, quota_bytes, None)
    }

    /// Insert the immutable product row for a restore-as-new lifecycle.
    pub fn ensure_restoring_namespace(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        quota_bytes: u64,
        backup_id: &str,
    ) -> Result<KvNamespaceRecord, PlatformError> {
        if uuid::Uuid::parse_str(backup_id)
            .ok()
            .is_none_or(|id| id.hyphenated().to_string() != backup_id)
        {
            return Err(invariant());
        }
        self.ensure_namespace_with_restore(
            resource,
            storage_key,
            schema_version,
            quota_bytes,
            Some(backup_id),
        )
    }

    fn ensure_namespace_with_restore(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        quota_bytes: u64,
        restore_backup_id: Option<&str>,
    ) -> Result<KvNamespaceRecord, PlatformError> {
        if resource.kind != open_compute_core::BindingKind::KvNamespace
            || resource.state != ResourceState::Creating
            || schema_version == 0
            || schema_version != resource.driver_schema_version
            || quota_bytes < 256 * 1024 * 1024
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO kv_namespaces
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
            let record = read_namespace_conn(tx, resource.account_id, resource.id)?;
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

    /// Read one live namespace while concealing cross-account identity.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<KvNamespaceRecord, PlatformError> {
        self.db
            .with_read(|conn| read_namespace_conn(conn, account_id, resource_id))
    }

    /// List live product rows in stable display-name order.
    pub fn list(&self, account_id: AccountId) -> Result<Vec<KvNamespaceRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                        r.availability_code, r.spec_generation, r.driver_schema_version,
                        r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                        k.storage_key, k.schema_version, k.quota_bytes,
                        k.last_opened_at_ms, k.last_quick_check_ms, k.last_backup_at_ms,
                        k.restore_backup_id
                 FROM resources r JOIN kv_namespaces k ON k.resource_id = r.id
                 WHERE r.account_id = ?1 AND r.kind = 'kv_namespace'
                   AND r.state != 'tombstoned'
                 ORDER BY r.name, r.id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([account_id.to_string()], map_namespace)
                .map_err(|_| invariant())?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|_| invariant())?);
            }
            Ok(records)
        })
    }

    /// List one bounded, filtered, and sorted page of live namespaces.
    #[allow(clippy::too_many_arguments)]
    pub fn list_page(
        &self,
        account_id: AccountId,
        search: Option<&str>,
        status: Option<ResourceState>,
        sort: CatalogSort,
        direction: CatalogDirection,
        after: Option<CatalogCursor>,
        limit: u16,
    ) -> Result<CatalogListPage<KvNamespaceRecord>, PlatformError> {
        let limit = normalize_catalog_limit(limit);
        let fetch = u32::from(limit).saturating_add(1);
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let exact_id = search.and_then(search_as_resource_id);
        let search_needle = if exact_id.is_some() {
            None
        } else {
            search.map(str::to_lowercase)
        };
        let query = build_catalog_sql(
            "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                    r.availability_code, r.spec_generation, r.driver_schema_version,
                    r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                    k.storage_key, k.schema_version, k.quota_bytes,
                    k.last_opened_at_ms, k.last_quick_check_ms, k.last_backup_at_ms,
                    k.restore_backup_id
             FROM resources r JOIN kv_namespaces k ON k.resource_id = r.id
             WHERE r.account_id = ? AND r.kind = 'kv_namespace' AND r.state != 'tombstoned'",
            CatalogColumns {
                id: "r.id",
                name: "r.name",
                state: "r.state",
                created_at: "r.created_at_ms",
                updated_at: "r.updated_at_ms",
            },
            account_id.to_string(),
            search_needle,
            exact_id.map(|id| id.to_string()),
            status.map(|value| value.as_str().to_string()),
            sort,
            direction,
            after,
            fetch,
        )?;
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(&query.text).map_err(|_| invariant())?;
            let rows = statement
                .query_map(params_from_iter(query.values), map_namespace)
                .map_err(|_| invariant())?;
            let mut records = collect_namespace_rows(rows)?;
            let next_cursor = if records.len() > usize::from(limit) {
                records.pop();
                records.last().map(|record| {
                    record_catalog_cursor(
                        sort,
                        direction,
                        &record.resource.name,
                        record.resource.created_at_ms,
                        record.resource.updated_at_ms,
                        &record.resource.id.to_string(),
                    )
                })
            } else {
                None
            };
            Ok(CatalogListPage {
                items: records,
                next_cursor,
            })
        })
    }

    /// Record a successful cold open without changing resource generation.
    pub fn record_open(&self, resource_id: ResourceId, now_ms: i64) -> Result<(), PlatformError> {
        self.update_time(resource_id, "last_opened_at_ms", now_ms)
    }

    /// Record a successful integrity check.
    pub fn record_quick_check(
        &self,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.update_time(resource_id, "last_quick_check_ms", now_ms)
    }

    /// Record a successful immutable backup.
    pub fn record_backup(&self, resource_id: ResourceId, now_ms: i64) -> Result<(), PlatformError> {
        self.update_time(resource_id, "last_backup_at_ms", now_ms)
    }

    fn update_time(
        self,
        resource_id: ResourceId,
        column: &'static str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let sql = match column {
            "last_opened_at_ms" => {
                "UPDATE kv_namespaces SET last_opened_at_ms = ?1 WHERE resource_id = ?2"
            }
            "last_quick_check_ms" => {
                "UPDATE kv_namespaces SET last_quick_check_ms = ?1 WHERE resource_id = ?2"
            }
            "last_backup_at_ms" => {
                "UPDATE kv_namespaces SET last_backup_at_ms = ?1 WHERE resource_id = ?2"
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

    /// Reserve a host-generated backup identity.
    pub fn create_backup(
        &self,
        source: ResourceId,
        backup_id: &str,
        schema_version: u32,
        idempotency_key: &str,
        request_fingerprint: &[u8; 32],
        now_ms: i64,
    ) -> Result<KvBackupRecord, PlatformError> {
        if uuid::Uuid::parse_str(backup_id)
            .ok()
            .is_none_or(|id| id.hyphenated().to_string() != backup_id)
            || schema_version == 0
            || idempotency_key.is_empty()
            || idempotency_key.len() > 128
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let existing: Option<(String, Vec<u8>)> = tx
                .query_row(
                    "SELECT id, request_fingerprint FROM kv_backups
                     WHERE source_resource_id = ?1 AND idempotency_key = ?2",
                    params![source.to_string(), idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| invariant())?;
            if let Some((existing_id, fingerprint)) = existing {
                if fingerprint.as_slice() != request_fingerprint {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "KV backup idempotency fingerprint does not match",
                    ));
                }
                return read_backup_conn(tx, &existing_id);
            }
            tx.execute(
                "INSERT INTO kv_backups
                 (id, source_resource_id, state, object_key, sha256, size_bytes,
                  kv_schema_version, created_at_ms, completed_at_ms, error_code,
                  idempotency_key, request_fingerprint)
                 VALUES (?1, ?2, 'creating', NULL, NULL, NULL, ?3, ?4, NULL, NULL, ?5, ?6)",
                params![
                    backup_id,
                    source.to_string(),
                    i64::from(schema_version),
                    now_ms,
                    idempotency_key,
                    request_fingerprint.as_slice(),
                ],
            )
            .map_err(|_| invariant())?;
            read_backup_conn(tx, backup_id)
        })
    }

    /// Mark a backup ready after object upload and readback verification.
    pub fn complete_backup(
        &self,
        backup_id: &str,
        object_key: &str,
        sha256: &[u8; 32],
        size_bytes: u64,
        now_ms: i64,
    ) -> Result<KvBackupRecord, PlatformError> {
        if object_key.is_empty() || object_key.contains("..") || !object_key.contains("backups/kv/")
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE kv_backups SET state = 'ready', object_key = ?1, sha256 = ?2,
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
                .map_err(|_| invariant())?;
            if changed != 1 {
                return Err(invariant());
            }
            let backup = read_backup_conn(tx, backup_id)?;
            if tx
                .execute(
                    "UPDATE kv_namespaces SET last_backup_at_ms = ?1 WHERE resource_id = ?2",
                    params![now_ms, backup.source_resource_id.to_string()],
                )
                .map_err(|_| invariant())?
                != 1
            {
                return Err(invariant());
            }
            Ok(backup)
        })
    }

    /// Mark an in-progress backup failed with a stable error code.
    pub fn fail_backup(
        &self,
        backup_id: &str,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<KvBackupRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            if tx
                .execute(
                    "UPDATE kv_backups SET state = 'failed', completed_at_ms = ?1,
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

    /// Read one backup scoped through its source account.
    pub fn get_backup(
        &self,
        account_id: AccountId,
        backup_id: &str,
    ) -> Result<KvBackupRecord, PlatformError> {
        self.db.with_read(|conn| {
            let owned: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM kv_backups b JOIN resources r
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

    /// List backups for one account without exposing physical object keys.
    pub fn list_backups(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<KvBackupRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT b.id, b.source_resource_id, b.state, b.object_key, b.sha256,
                        b.size_bytes, b.kv_schema_version, b.created_at_ms,
                        b.completed_at_ms, b.error_code
                 FROM kv_backups b JOIN resources r ON r.id = b.source_resource_id
                 WHERE r.account_id = ?1 ORDER BY b.created_at_ms, b.id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([account_id.to_string()], map_backup)
                .map_err(|_| invariant())?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|_| invariant())?);
            }
            Ok(records)
        })
    }

    /// Permanently retire a failed backup or a ready backup whose object was deleted.
    pub fn tombstone_backup(
        &self,
        account_id: AccountId,
        backup_id: &str,
        now_ms: i64,
    ) -> Result<KvBackupRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let owned: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM kv_backups b JOIN resources r
                   ON r.id = b.source_resource_id WHERE b.id = ?1 AND r.account_id = ?2)",
                    params![backup_id, account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            if !owned {
                return Err(not_found());
            }
            let changed = tx
                .execute(
                    "UPDATE kv_backups SET state = 'tombstoned', object_key = NULL,
                        sha256 = NULL, size_bytes = NULL, completed_at_ms = ?1
                 WHERE id = ?2 AND state IN ('ready', 'failed')",
                    params![now_ms, backup_id],
                )
                .map_err(|_| invariant())?;
            if changed != 1 {
                return Err(invariant());
            }
            read_backup_conn(tx, backup_id)
        })
    }
}

fn read_namespace_conn(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
) -> Result<KvNamespaceRecord, PlatformError> {
    conn.query_row(
        "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
                r.availability_code, r.spec_generation, r.driver_schema_version,
                r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
                k.storage_key, k.schema_version, k.quota_bytes,
                k.last_opened_at_ms, k.last_quick_check_ms, k.last_backup_at_ms,
                k.restore_backup_id
         FROM resources r JOIN kv_namespaces k ON k.resource_id = r.id
         WHERE r.account_id = ?1 AND r.id = ?2 AND r.kind = 'kv_namespace'",
        params![account_id.to_string(), resource_id.to_string()],
        map_namespace,
    )
    .optional()
    .map_err(|_| invariant())?
    .ok_or_else(not_found)
}

fn map_namespace(row: &rusqlite::Row<'_>) -> rusqlite::Result<KvNamespaceRecord> {
    let resource = crate::resources::map_resource_offset(row, 0)?;
    let schema: i64 = row.get(13)?;
    let quota: i64 = row.get(14)?;
    Ok(KvNamespaceRecord {
        resource,
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
) -> Result<KvBackupRecord, PlatformError> {
    conn.query_row(
        "SELECT id, source_resource_id, state, object_key, sha256, size_bytes,
                kv_schema_version, created_at_ms, completed_at_ms, error_code
         FROM kv_backups WHERE id = ?1",
        [backup_id],
        map_backup,
    )
    .optional()
    .map_err(|_| invariant())?
    .ok_or_else(not_found)
}

fn map_backup(row: &rusqlite::Row<'_>) -> rusqlite::Result<KvBackupRecord> {
    let resource: String = row.get(1)?;
    let state: String = row.get(2)?;
    let digest: Option<Vec<u8>> = row.get(4)?;
    let size: Option<i64> = row.get(5)?;
    let schema: i64 = row.get(6)?;
    Ok(KvBackupRecord {
        id: row.get(0)?,
        source_resource_id: ResourceId::from_str(&resource)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: KvBackupState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        object_key: row.get(3)?,
        sha256: digest
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        size_bytes: size
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        kv_schema_version: u32::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(7)?,
        completed_at_ms: row.get(8)?,
        error_code: row.get(9)?,
    })
}

fn collect_namespace_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> Result<KvNamespaceRecord, rusqlite::Error>,
    >,
) -> Result<Vec<KvNamespaceRecord>, PlatformError> {
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|_| invariant())?);
    }
    Ok(records)
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "KV catalog authority invariant failed",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "KV resource was not found")
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
