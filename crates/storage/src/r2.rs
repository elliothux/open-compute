//! Logical R2 bucket locator authority in `control.sqlite`.

use crate::{ControlDb, ResourceRecord};
use open_compute_core::{AccountId, BindingKind, ErrorCode, PlatformError, ResourceId};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::str::FromStr as _;

/// Product schema stored for P0.5 logical buckets.
pub const R2_SCHEMA_VERSION: u32 = 1;

/// Frozen locator and limits for one logical R2 bucket.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct R2BucketRecord {
    /// Shared resource lifecycle and account authority.
    #[serde(flatten)]
    pub resource: ResourceRecord,
    /// Host-only S3 prefix. Control API serializers must omit this field.
    #[serde(skip_serializing)]
    pub physical_prefix: String,
    /// Product schema version.
    pub schema_version: u32,
    /// Object limit frozen when the bucket is created.
    pub max_object_bytes: u64,
    /// Frozen provider authority digest. Control API serializers must omit it.
    #[serde(skip_serializing)]
    pub provider_config_sha256: [u8; 32],
    /// First durable deletion attempt, if any.
    pub delete_started_at_ms: Option<i64>,
    /// Last successful or failed provider probe timestamp.
    pub last_probe_at_ms: Option<i64>,
}

/// Typed repository for logical R2 bucket locators.
#[derive(Clone, Copy, Debug)]
pub struct R2BucketRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> R2BucketRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Insert or recover the immutable locator for a creating resource.
    pub fn ensure_bucket(
        &self,
        resource: &ResourceRecord,
        physical_prefix: &str,
        max_object_bytes: u64,
        provider_config_sha256: &[u8; 32],
    ) -> Result<R2BucketRecord, PlatformError> {
        validate_locator(
            resource,
            physical_prefix,
            max_object_bytes,
            provider_config_sha256,
        )?;
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO r2_buckets
                 (resource_id, physical_prefix, schema_version, max_object_bytes,
                  provider_config_sha256,
                  created_at_ms, delete_started_at_ms, last_probe_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)
                 ON CONFLICT(resource_id) DO NOTHING",
                params![
                    resource.id.to_string(),
                    physical_prefix,
                    i64::from(R2_SCHEMA_VERSION),
                    i64::try_from(max_object_bytes).map_err(|_| invariant())?,
                    provider_config_sha256.as_slice(),
                    resource.created_at_ms,
                ],
            )
            .map_err(|_| invariant())?;
            let bucket = read_bucket(tx, resource.account_id, resource.id)?;
            if bucket.physical_prefix != physical_prefix
                || bucket.max_object_bytes != max_object_bytes
                || bucket.provider_config_sha256 != *provider_config_sha256
                || bucket.schema_version != R2_SCHEMA_VERSION
            {
                return Err(invariant());
            }
            Ok(bucket)
        })
    }

    /// Read one account-scoped bucket locator.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<R2BucketRecord, PlatformError> {
        self.db
            .with_read(|conn| read_bucket(conn, account_id, resource_id))
    }

    /// List live and transitional buckets for one account.
    pub fn list(&self, account_id: AccountId) -> Result<Vec<R2BucketRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(&format!("{SELECT_BUCKETS} ORDER BY r.name, r.id"))
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([account_id.to_string()], map_bucket)
                .map_err(|_| db_error())?;
            let mut buckets = Vec::new();
            for row in rows {
                buckets.push(row.map_err(|_| invariant())?);
            }
            Ok(buckets)
        })
    }

    /// List every live and transitional bucket for host maintenance.
    pub fn list_all(&self) -> Result<Vec<R2BucketRecord>, PlatformError> {
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "{SELECT_ALL_BUCKETS} ORDER BY r.account_id, r.name, r.id"
                ))
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([], map_bucket)
                .map_err(|_| db_error())?;
            let mut buckets = Vec::new();
            for row in rows {
                buckets.push(row.map_err(|_| invariant())?);
            }
            Ok(buckets)
        })
    }

    /// Persist the deletion fence before any remote object is removed.
    pub fn mark_delete_started(
        &self,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_buckets
                     SET delete_started_at_ms = COALESCE(delete_started_at_ms, ?1)
                     WHERE resource_id = ?2
                       AND (SELECT state FROM resources
                            WHERE id = r2_buckets.resource_id) = 'deleting'",
                    params![now_ms, resource_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotReady,
                    "R2 bucket is not in the deleting lifecycle",
                ));
            }
            Ok(())
        })
    }

    /// Record a provider health probe without changing immutable identity.
    pub fn mark_probed(&self, resource_id: ResourceId, now_ms: i64) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE r2_buckets SET last_probe_at_ms = ?1 WHERE resource_id = ?2",
                    params![now_ms, resource_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(not_found());
            }
            Ok(())
        })
    }
}

const SELECT_BUCKETS: &str = "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
            r.availability_code, r.spec_generation, r.driver_schema_version,
            r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
            b.physical_prefix, b.schema_version, b.max_object_bytes,
            b.provider_config_sha256, b.delete_started_at_ms, b.last_probe_at_ms
     FROM r2_buckets b JOIN resources r ON r.id = b.resource_id
     WHERE r.account_id = ?1 AND r.kind = 'r2_bucket'";

const SELECT_ALL_BUCKETS: &str =
    "SELECT r.id, r.account_id, r.kind, r.name, r.state, r.availability,
            r.availability_code, r.spec_generation, r.driver_schema_version,
            r.created_at_ms, r.updated_at_ms, r.deleted_at_ms,
            b.physical_prefix, b.schema_version, b.max_object_bytes,
            b.provider_config_sha256, b.delete_started_at_ms, b.last_probe_at_ms
     FROM r2_buckets b JOIN resources r ON r.id = b.resource_id
     WHERE r.kind = 'r2_bucket'";

fn read_bucket(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
) -> Result<R2BucketRecord, PlatformError> {
    conn.query_row(
        &format!("{SELECT_BUCKETS} AND r.id = ?2"),
        params![account_id.to_string(), resource_id.to_string()],
        map_bucket,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(not_found)
}

fn map_bucket(row: &rusqlite::Row<'_>) -> rusqlite::Result<R2BucketRecord> {
    let resource_id: String = row.get(0)?;
    let account_id: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let state: String = row.get(4)?;
    let availability: String = row.get(5)?;
    let generation: i64 = row.get(7)?;
    let driver_schema: i64 = row.get(8)?;
    let product_schema: i64 = row.get(13)?;
    let max_object_bytes: i64 = row.get(14)?;
    let provider_config_sha256: Vec<u8> = row.get(15)?;
    Ok(R2BucketRecord {
        resource: ResourceRecord {
            id: ResourceId::from_str(&resource_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
            account_id: AccountId::from_str(&account_id)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            kind: BindingKind::from_str(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
            name: row.get(3)?,
            state: open_compute_core::ResourceState::from_str(&state)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            availability: open_compute_core::ResourceAvailability::from_str(&availability)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            availability_code: row.get(6)?,
            spec_generation: u64::try_from(generation)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            driver_schema_version: u32::try_from(driver_schema)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            created_at_ms: row.get(9)?,
            updated_at_ms: row.get(10)?,
            deleted_at_ms: row.get(11)?,
        },
        physical_prefix: row.get(12)?,
        schema_version: u32::try_from(product_schema).map_err(|_| rusqlite::Error::InvalidQuery)?,
        max_object_bytes: u64::try_from(max_object_bytes)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        provider_config_sha256: provider_config_sha256
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        delete_started_at_ms: row.get(16)?,
        last_probe_at_ms: row.get(17)?,
    })
}

fn validate_locator(
    resource: &ResourceRecord,
    physical_prefix: &str,
    max_object_bytes: u64,
    provider_config_sha256: &[u8; 32],
) -> Result<(), PlatformError> {
    if resource.kind != BindingKind::R2Bucket
        || resource.state != open_compute_core::ResourceState::Creating
        || resource.driver_schema_version != R2_SCHEMA_VERSION
        || max_object_bytes == 0
        || provider_config_sha256.iter().all(|byte| *byte == 0)
        || !physical_prefix.ends_with('/')
        || physical_prefix.starts_with('/')
        || physical_prefix.contains("..")
        || physical_prefix.contains('\\')
        || !physical_prefix.contains(&resource.id.to_string())
    {
        return Err(invariant());
    }
    Ok(())
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceNotFound,
        "R2 bucket was not found in the requested scope",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "R2 bucket locator invariant failed",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "R2 bucket catalog operation failed")
}

#[cfg(test)]
#[path = "r2_tests.rs"]
mod tests;
