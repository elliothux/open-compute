//! Durable Vectorize product rows in `control.sqlite`.

use crate::{ControlDb, ResourceRecord, ResourceRepository};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, ResourceId, ResourceState,
};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::str::FromStr;

/// Immutable product catalog row for one live Vectorize index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorizeIndexRecord {
    /// Shared resource identity and lifecycle.
    pub resource: ResourceRecord,
    /// Canonical private locator.
    #[serde(skip)]
    pub storage_key: String,
    /// Per-index database format version.
    pub schema_version: u32,
    /// Exact frozen vector dimensions.
    pub dimensions: u32,
    /// Frozen metric token.
    pub metric: String,
    /// Maximum applied vectors.
    pub quota_vectors: u64,
    /// Maximum SQLite bytes admitted for the index.
    pub quota_bytes: u64,
}

/// Product catalog repository over the central control database.
#[derive(Clone, Copy, Debug)]
pub struct VectorizeIndexRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> VectorizeIndexRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Insert the immutable locator and contract for a creating Vectorize index.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_index(
        self,
        resource: &ResourceRecord,
        storage_key: &str,
        schema_version: u32,
        dimensions: u32,
        metric: &str,
        quota_vectors: u64,
        quota_bytes: u64,
    ) -> Result<VectorizeIndexRecord, PlatformError> {
        if resource.kind != BindingKind::VectorizeIndex
            || resource.state != ResourceState::Creating
            || schema_version != 1
            || schema_version != resource.driver_schema_version
            || !(32..=1_536).contains(&dimensions)
            || !matches!(metric, "cosine" | "euclidean" | "dot-product")
            || quota_vectors == 0
            || quota_vectors > 200_000
            || quota_bytes < 1_048_576
        {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO vectorize_indexes
                 (resource_id, storage_key, schema_version, dimensions, metric,
                  quota_vectors, quota_bytes, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(resource_id) DO NOTHING",
                params![
                    resource.id.to_string(),
                    storage_key,
                    i64::from(schema_version),
                    i64::from(dimensions),
                    metric,
                    i64::try_from(quota_vectors).map_err(|_| invariant())?,
                    i64::try_from(quota_bytes).map_err(|_| invariant())?,
                    resource.created_at_ms,
                ],
            )
            .map_err(|_| invariant())?;
            let stored = read_product(tx, resource)?;
            if stored.storage_key != storage_key
                || stored.schema_version != schema_version
                || stored.dimensions != dimensions
                || stored.metric != metric
                || stored.quota_vectors != quota_vectors
                || stored.quota_bytes != quota_bytes
            {
                return Err(invariant());
            }
            Ok(stored)
        })
    }

    /// Read one account-scoped index without exposing cross-account identities.
    pub fn get(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<VectorizeIndexRecord, PlatformError> {
        let resource = ResourceRepository::new(self.db).get(account_id, resource_id)?;
        self.db.with_read(|conn| read_product(conn, &resource))
    }

    /// List live indexes in stable resource display-name order.
    pub fn list(&self, account_id: AccountId) -> Result<Vec<VectorizeIndexRecord>, PlatformError> {
        let resources =
            ResourceRepository::new(self.db).list(account_id, Some(BindingKind::VectorizeIndex))?;
        self.db.with_read(|conn| {
            resources
                .iter()
                .map(|resource| read_product(conn, resource))
                .collect()
        })
    }

    /// Enumerate one stable page of ready indexes after an optional identity cursor.
    pub fn ready_indexes_after(
        &self,
        after: Option<(AccountId, ResourceId)>,
        limit: u32,
    ) -> Result<Vec<VectorizeIndexRecord>, PlatformError> {
        if limit == 0 || limit > 10_000 {
            return Err(invariant());
        }
        let identities = self.db.with_read(|conn| {
            let (sql, parameters): (&str, Vec<rusqlite::types::Value>) = match after {
                Some((account, resource)) => (
                    "SELECT account_id, id FROM resources
                     WHERE kind = 'vectorize_index' AND state = 'ready'
                       AND (account_id > ?1 OR (account_id = ?1 AND id > ?2))
                     ORDER BY account_id, id LIMIT ?3",
                    vec![
                        account.to_string().into(),
                        resource.to_string().into(),
                        i64::from(limit).into(),
                    ],
                ),
                None => (
                    "SELECT account_id, id FROM resources
                     WHERE kind = 'vectorize_index' AND state = 'ready'
                     ORDER BY account_id, id LIMIT ?1",
                    vec![i64::from(limit).into()],
                ),
            };
            let mut statement = conn.prepare(sql).map_err(|_| invariant())?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(parameters), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|_| invariant())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|_| invariant())
        })?;
        identities
            .into_iter()
            .map(|(account, resource)| {
                self.get(
                    AccountId::from_str(&account).map_err(|_| invariant())?,
                    ResourceId::from_str(&resource).map_err(|_| invariant())?,
                )
            })
            .collect()
    }

    /// Enumerate the first stable page of ready indexes.
    pub fn ready_indexes(&self, limit: u32) -> Result<Vec<VectorizeIndexRecord>, PlatformError> {
        self.ready_indexes_after(None, limit)
    }
}

fn read_product(
    conn: &rusqlite::Connection,
    resource: &ResourceRecord,
) -> Result<VectorizeIndexRecord, PlatformError> {
    if resource.kind != BindingKind::VectorizeIndex {
        return Err(not_found());
    }
    let row = conn
        .query_row(
            "SELECT storage_key, schema_version, dimensions, metric, quota_vectors, quota_bytes
             FROM vectorize_indexes WHERE resource_id = ?1",
            [resource.id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|_| invariant())?
        .ok_or_else(not_found)?;
    Ok(VectorizeIndexRecord {
        resource: resource.clone(),
        storage_key: row.0,
        schema_version: u32::try_from(row.1).map_err(|_| invariant())?,
        dimensions: u32::try_from(row.2).map_err(|_| invariant())?,
        metric: row.3,
        quota_vectors: u64::try_from(row.4).map_err(|_| invariant())?,
        quota_bytes: u64::try_from(row.5).map_err(|_| invariant())?,
    })
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Vectorize catalog authority invariant failed",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "Vectorize index was not found")
}
