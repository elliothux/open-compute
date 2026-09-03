//! Per-index SQLite authority and ordered durable Vectorize mutation engine.

mod persistence;
mod read_snapshot;
mod schema;

pub use read_snapshot::VectorizeReadSnapshot;

use open_compute_core::PlatformError;
use open_compute_search::FilterExpr;
use persistence::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use schema::SCHEMA;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Current per-index SQLite schema version.
pub const VECTORIZE_SCHEMA_VERSION: u32 = 1;
const MAX_BATCH_ITEMS: usize = 1_000;
const MAX_ID_BYTES: usize = 64;
const MAX_NAMESPACE_BYTES: usize = 64;
const MAX_METADATA_BYTES: usize = 10 * 1_024;
type EncodedMutationItem = (String, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>);

/// Durable Vectorize mutation kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VectorMutationKind {
    /// Add only when every ID is absent.
    Insert,
    /// Fully replace existing vector records.
    Upsert,
    /// Delete applied IDs.
    Delete,
}

impl VectorMutationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }
}

/// Durable mutation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VectorMutationState {
    /// Committed but not applied.
    Queued,
    /// Leased by the mutation coordinator.
    Claimed,
    /// Atomically applied to visible vectors.
    Applied,
    /// Permanently failed and blocks the processed frontier.
    Failed,
}

/// One fully validated mutation item.
#[derive(Clone, Debug)]
pub struct VectorMutationInput {
    /// Public vector identifier.
    pub id: String,
    /// Optional namespace.
    pub namespace: Option<String>,
    /// Exact vector values for insert/upsert; absent for delete.
    pub values: Option<Vec<f32>>,
    /// Optional metadata object.
    pub metadata: Option<Value>,
}

/// One mutation receipt and durable status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorMutation {
    /// Host-generated immutable mutation ID.
    pub mutation_id: String,
    /// Index-local monotonic sequence.
    pub sequence: u64,
    /// Mutation kind.
    pub kind: VectorMutationKind,
    /// Current durable state.
    pub state: VectorMutationState,
    /// Number of batch items.
    pub item_count: u32,
    /// Stable error code for a permanent failure.
    pub error_code: Option<String>,
}

/// One applied vector record.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorRecord {
    /// Vector identifier.
    pub id: String,
    /// Optional namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Authoritative decoded f32 values.
    pub values: Vec<f32>,
    /// Optional complete metadata object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Stable index description and applied mutation frontier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorizeDescription {
    /// Frozen dimensions.
    pub dimensions: u32,
    /// Frozen metric token.
    pub metric: String,
    /// Applied vector count.
    pub vector_count: u64,
    /// Highest contiguous applied sequence, exposed as pinned numeric `processedUpToMutation`.
    pub processed_sequence: u64,
    /// Host-generated ID of the highest contiguous applied mutation.
    pub processed_mutation_id: Option<String>,
    /// Completion time of the highest contiguous applied mutation.
    pub processed_at_ms: Option<i64>,
    /// Metadata index generation.
    pub metadata_generation: u64,
}

/// One materialized metadata-index declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorMetadataIndex {
    /// Dot-separated property path.
    pub property_name: String,
    /// Frozen scalar index type.
    pub index_type: String,
}

/// Thread-safe owner of one per-index SQLite database.
#[derive(Debug)]
pub struct VectorizeEngine {
    connection: Mutex<Connection>,
    dimensions: usize,
    quota_vectors: u64,
    quota_bytes: u64,
}

impl VectorizeEngine {
    /// Open or create one exact index, refusing mismatched persisted identity/configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        path: &Path,
        resource_id: &str,
        dimensions: u32,
        metric: &str,
        quota_vectors: u64,
        quota_bytes: u64,
        busy_timeout_ms: u64,
    ) -> Result<Self, PlatformError> {
        if !(1..=1_536).contains(&dimensions)
            || !matches!(metric, "cosine" | "euclidean" | "dot-product")
            || quota_vectors == 0
            || quota_bytes < 1_048_576
        {
            return Err(invalid());
        }
        let parent = path.parent().ok_or_else(invalid)?;
        crate::fs::validate_owned_dir(parent)?;
        crate::fs::ensure_file_secure(path)?;
        let file = crate::fs::open_nofollow(path, false, true)?;
        crate::fs::validate_authority_fd(&file)?;
        drop(file);
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| unavailable())?;
        connection
            .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
            .map_err(|_| unavailable())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=FULL;
                 PRAGMA foreign_keys=ON;
                 PRAGMA trusted_schema=OFF;",
            )
            .map_err(|_| unavailable())?;
        connection.execute_batch(SCHEMA).map_err(|_| corrupt())?;
        connection
            .execute(
                "INSERT INTO index_meta
                 (singleton, resource_id, schema_version, dimensions, metric, quota_vectors,
                  quota_bytes, vector_count, next_sequence, processed_sequence, metadata_generation)
                 VALUES (1, ?1, 1, ?2, ?3, ?4, ?5, 0, 1, 0, 0)
                 ON CONFLICT(singleton) DO NOTHING",
                params![
                    resource_id,
                    i64::from(dimensions),
                    metric,
                    i64::try_from(quota_vectors).map_err(|_| invalid())?,
                    i64::try_from(quota_bytes).map_err(|_| invalid())?,
                ],
            )
            .map_err(|_| corrupt())?;
        let persisted: (String, i64, i64, String, i64, i64) = connection
            .query_row(
                "SELECT resource_id, schema_version, dimensions, metric, quota_vectors, quota_bytes
                 FROM index_meta WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .map_err(|_| corrupt())?;
        if persisted.0 != resource_id
            || persisted.1 != i64::from(VECTORIZE_SCHEMA_VERSION)
            || persisted.2 != i64::from(dimensions)
            || persisted.3 != metric
            || persisted.4 != i64::try_from(quota_vectors).map_err(|_| invalid())?
            || persisted.5 != i64::try_from(quota_bytes).map_err(|_| invalid())?
        {
            return Err(corrupt());
        }
        Ok(Self {
            connection: Mutex::new(connection),
            dimensions: usize::try_from(dimensions).map_err(|_| invalid())?,
            quota_vectors,
            quota_bytes,
        })
    }

    /// Read frozen config, visible count, and contiguous processed frontier.
    pub fn describe(&self) -> Result<VectorizeDescription, PlatformError> {
        self.lock()?
            .query_row(
                "SELECT dimensions, metric, vector_count, processed_sequence, metadata_generation,
                    (SELECT mutation_id FROM vector_mutations
                     WHERE sequence = index_meta.processed_sequence AND state = 'applied'),
                    (SELECT completed_at_ms FROM vector_mutations
                     WHERE sequence = index_meta.processed_sequence AND state = 'applied')
             FROM index_meta WHERE singleton = 1",
                [],
                |row| {
                    Ok(VectorizeDescription {
                        dimensions: row.get(0)?,
                        metric: row.get(1)?,
                        vector_count: row.get(2)?,
                        processed_sequence: row.get(3)?,
                        metadata_generation: row.get(4)?,
                        processed_mutation_id: row.get(5)?,
                        processed_at_ms: row.get(6)?,
                    })
                },
            )
            .map_err(|_| corrupt())
    }

    /// Run SQLite quick-check and verify the persisted visible count.
    pub fn quick_check(&self) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let check: String = connection
            .pragma_query_value(None, "quick_check", |row| row.get(0))
            .map_err(|_| corrupt())?;
        let (stored, actual): (i64, i64) = connection
            .query_row(
                "SELECT vector_count, (SELECT COUNT(*) FROM vectors)
                 FROM index_meta WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| corrupt())?;
        if check != "ok" || stored != actual {
            return Err(corrupt());
        }
        Ok(())
    }

    /// Checkpoint WAL state before quarantine or snapshot.
    pub fn checkpoint(&self, truncate: bool) -> Result<(), PlatformError> {
        let mode = if truncate { "TRUNCATE" } else { "PASSIVE" };
        self.lock()?
            .pragma_update(None, "wal_checkpoint", mode)
            .map_err(|_| unavailable())
    }

    /// Durable-enqueue a complete mutation batch; queued payload is not query-visible.
    pub fn enqueue(
        &self,
        kind: VectorMutationKind,
        items: &[VectorMutationInput],
        now_ms: i64,
    ) -> Result<VectorMutation, PlatformError> {
        let encoded = self.validate_batch(kind, items)?;
        let mutation_id = uuid::Uuid::now_v7().hyphenated().to_string();
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(|_| unavailable())?;
        let sequence: i64 = tx
            .query_row(
                "SELECT next_sequence FROM index_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| corrupt())?;
        let payload_bytes = encoded.iter().try_fold(0_i64, |sum, item| {
            let bytes = item.2.as_ref().map_or(0, Vec::len)
                + item.3.as_ref().map_or(0, Vec::len)
                + item.0.len()
                + item.1.as_ref().map_or(0, String::len);
            sum.checked_add(i64::try_from(bytes).map_err(|_| invalid())?)
                .ok_or_else(invalid)
        })?;
        tx.execute(
            "INSERT INTO vector_mutations
             (mutation_id, sequence, kind, state, claim_token, claim_until_ms, attempt,
              next_attempt_at_ms, item_count, payload_bytes, error_code, created_at_ms,
              completed_at_ms)
             VALUES (?1, ?2, ?3, 'queued', NULL, NULL, 0, ?6, ?4, ?5, NULL, ?6, NULL)",
            params![
                mutation_id,
                sequence,
                kind.as_str(),
                i64::try_from(encoded.len()).map_err(|_| invalid())?,
                payload_bytes,
                now_ms
            ],
        )
        .map_err(|_| unavailable())?;
        for (ordinal, (id, namespace, values, metadata)) in encoded.iter().enumerate() {
            tx.execute(
                "INSERT INTO vector_mutation_items
                 (mutation_id, ordinal, vector_id, namespace, values_f32le, metadata_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    mutation_id,
                    i64::try_from(ordinal).map_err(|_| invalid())?,
                    id,
                    namespace,
                    values,
                    metadata
                ],
            )
            .map_err(|_| unavailable())?;
        }
        validate_pending_projection(&tx, self.quota_vectors, self.quota_bytes)?;
        tx.execute(
            "UPDATE index_meta SET next_sequence = next_sequence + 1 WHERE singleton = 1",
            [],
        )
        .map_err(|_| corrupt())?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(VectorMutation {
            mutation_id,
            sequence: u64::try_from(sequence).map_err(|_| corrupt())?,
            kind,
            state: VectorMutationState::Queued,
            item_count: u32::try_from(encoded.len()).map_err(|_| invalid())?,
            error_code: None,
        })
    }

    /// Claim and apply the next sequence. A permanent failure remains the blocking frontier.
    pub fn apply_next(&self, now_ms: i64) -> Result<Option<VectorMutation>, PlatformError> {
        let token = uuid::Uuid::now_v7().hyphenated().to_string();
        let Some(_) = self.claim_next(&token, now_ms, 30_000)? else {
            return Ok(None);
        };
        self.apply_claimed(&token, now_ms)
    }

    /// Return whether the contiguous frontier is held by an unexpired lease.
    pub fn frontier_is_claimed(&self, now_ms: i64) -> Result<bool, PlatformError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM vector_mutations AS mutation
                    JOIN index_meta AS meta ON meta.singleton = 1
                    WHERE mutation.sequence = meta.processed_sequence + 1
                      AND mutation.state = 'claimed'
                      AND mutation.claim_until_ms > ?1
                )",
                [now_ms],
                |row| row.get(0),
            )
            .map_err(|_| corrupt())
    }

    /// Durably lease the next contiguous mutation, reclaiming only an expired lease.
    pub fn claim_next(
        &self,
        claim_token: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<VectorMutation>, PlatformError> {
        if claim_token.is_empty() || claim_token.len() > 128 || lease_ms <= 0 {
            return Err(invalid());
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(|_| unavailable())?;
        let processed: i64 = tx
            .query_row(
                "SELECT processed_sequence FROM index_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| corrupt())?;
        let next = read_mutation_at(&tx, processed.saturating_add(1))?;
        let Some(mut mutation) = next else {
            return Ok(None);
        };
        if mutation.state == VectorMutationState::Failed {
            return Err(frontier_blocked());
        }
        if mutation.state == VectorMutationState::Applied {
            return Err(corrupt());
        }
        if mutation.state == VectorMutationState::Claimed {
            let claim_until: Option<i64> = tx
                .query_row(
                    "SELECT claim_until_ms FROM vector_mutations WHERE mutation_id = ?1",
                    [&mutation.mutation_id],
                    |row| row.get(0),
                )
                .map_err(|_| corrupt())?;
            if claim_until.is_some_and(|until| until > now_ms) {
                return Ok(None);
            }
        }
        let claim_until = now_ms.checked_add(lease_ms).ok_or_else(limit)?;
        if tx
            .execute(
                "UPDATE vector_mutations SET state = 'claimed', claim_token = ?1,
                 claim_until_ms = ?2, attempt = attempt + 1
                 WHERE mutation_id = ?3 AND (state = 'queued'
                    OR (state = 'claimed' AND claim_until_ms <= ?4))",
                params![
                    claim_token.as_bytes(),
                    claim_until,
                    mutation.mutation_id,
                    now_ms
                ],
            )
            .map_err(|_| unavailable())?
            != 1
        {
            return Ok(None);
        }
        tx.commit().map_err(|_| unavailable())?;
        mutation.state = VectorMutationState::Claimed;
        Ok(Some(mutation))
    }

    /// Apply the mutation currently fenced by the exact claim token.
    pub fn apply_claimed(
        &self,
        claim_token: &str,
        now_ms: i64,
    ) -> Result<Option<VectorMutation>, PlatformError> {
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(|_| unavailable())?;
        let processed: i64 = tx
            .query_row(
                "SELECT processed_sequence FROM index_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| corrupt())?;
        let Some(mut mutation) = read_mutation_at(&tx, processed.saturating_add(1))? else {
            return Ok(None);
        };
        let (stored_token, claim_until): (Option<Vec<u8>>, Option<i64>) = tx
            .query_row(
                "SELECT claim_token, claim_until_ms FROM vector_mutations WHERE mutation_id = ?1",
                [&mutation.mutation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| corrupt())?;
        if mutation.state != VectorMutationState::Claimed
            || stored_token.as_deref() != Some(claim_token.as_bytes())
            || claim_until.is_none_or(|until| until <= now_ms)
        {
            return Err(frontier_blocked());
        }
        let items = read_items(&tx, &mutation.mutation_id)?;
        let validation = validate_persisted_items(&items, mutation.kind, self.dimensions);
        if let Err(error) = validation {
            mark_failed(&tx, &mutation.mutation_id, error.code().as_str(), now_ms)?;
            tx.commit().map_err(|_| unavailable())?;
            return Err(error);
        }
        if matches!(
            mutation.kind,
            VectorMutationKind::Insert | VectorMutationKind::Upsert
        ) {
            let current: i64 = tx
                .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))
                .map_err(|_| corrupt())?;
            let mut added = 0_u64;
            for item in &items {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM vectors WHERE vector_id = ?1)",
                        [&item.id],
                        |row| row.get(0),
                    )
                    .map_err(|_| corrupt())?;
                if !exists {
                    added = added.checked_add(1).ok_or_else(limit)?;
                }
            }
            if u64::try_from(current)
                .map_err(|_| corrupt())?
                .checked_add(added)
                .is_none_or(|count| count > self.quota_vectors)
            {
                mark_failed(&tx, &mutation.mutation_id, "VECTOR_QUOTA_EXCEEDED", now_ms)?;
                tx.commit().map_err(|_| unavailable())?;
                return Err(limit());
            }
            let current_bytes: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(
                        length(CAST(vector_id AS BLOB))
                        + COALESCE(length(CAST(namespace AS BLOB)), 0)
                        + length(values_f32le)
                        + COALESCE(length(metadata_json), 0)
                    ), 0) FROM vectors",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| corrupt())?;
            let mut final_bytes = u64::try_from(current_bytes).map_err(|_| corrupt())?;
            for item in &items {
                let existing_bytes: Option<i64> = tx
                    .query_row(
                        "SELECT length(CAST(vector_id AS BLOB))
                           + COALESCE(length(CAST(namespace AS BLOB)), 0)
                           + length(values_f32le)
                           + COALESCE(length(metadata_json), 0)
                         FROM vectors WHERE vector_id = ?1",
                        [&item.id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| corrupt())?;
                if mutation.kind == VectorMutationKind::Insert && existing_bytes.is_some() {
                    continue;
                }
                if let Some(existing_bytes) = existing_bytes {
                    final_bytes = final_bytes
                        .checked_sub(u64::try_from(existing_bytes).map_err(|_| corrupt())?)
                        .ok_or_else(corrupt)?;
                }
                final_bytes = final_bytes
                    .checked_add(logical_record_bytes(item)?)
                    .ok_or_else(limit)?;
            }
            if final_bytes > self.quota_bytes {
                mark_failed(
                    &tx,
                    &mutation.mutation_id,
                    "VECTOR_BYTE_QUOTA_EXCEEDED",
                    now_ms,
                )?;
                tx.commit().map_err(|_| unavailable())?;
                return Err(limit());
            }
        }
        match mutation.kind {
            VectorMutationKind::Insert | VectorMutationKind::Upsert => {
                for item in &items {
                    apply_write(&tx, item, mutation.sequence, mutation.kind)?;
                }
            }
            VectorMutationKind::Delete => {
                for item in &items {
                    tx.execute("DELETE FROM vectors WHERE vector_id = ?1", [&item.id])
                        .map_err(|_| unavailable())?;
                }
            }
        }
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))
            .map_err(|_| corrupt())?;
        tx.execute(
            "UPDATE index_meta SET vector_count = ?1, processed_sequence = ?2 WHERE singleton = 1",
            params![
                count,
                i64::try_from(mutation.sequence).map_err(|_| corrupt())?
            ],
        )
        .map_err(|_| corrupt())?;
        tx.execute(
            "UPDATE vector_mutations SET state = 'applied', claim_token = NULL,
             claim_until_ms = NULL, completed_at_ms = ?1
             WHERE mutation_id = ?2 AND state = 'claimed' AND claim_token = ?3",
            params![now_ms, mutation.mutation_id, claim_token.as_bytes()],
        )
        .map_err(|_| corrupt())?;
        prune_applied_mutation_payload(&tx, &mutation.mutation_id, mutation.sequence)?;
        tx.commit().map_err(|_| unavailable())?;
        mutation.state = VectorMutationState::Applied;
        Ok(Some(mutation))
    }

    /// Read applied records for IDs in request order; missing IDs are omitted.
    pub fn get_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>, PlatformError> {
        let connection = self.lock()?;
        read_snapshot::get_by_ids_from_connection(&connection, self.dimensions, ids)
    }

    /// Stream applied candidates in stable row order with one decoded row resident at a time.
    ///
    /// The callback must apply metadata filtering before scoring. Returning an error stops the
    /// scan without changing authority.
    pub fn scan_candidates(
        &self,
        namespace: Option<&str>,
        metadata_filter: Option<&FilterExpr>,
        visit: impl FnMut(VectorRecord) -> Result<(), PlatformError>,
    ) -> Result<u64, PlatformError> {
        let connection = self.lock()?;
        read_snapshot::scan_candidates_from_connection(
            &connection,
            self.dimensions,
            namespace,
            metadata_filter,
            visit,
        )
    }

    /// Create one metadata index and materialize terms for all applied vectors.
    pub fn create_metadata_index(
        &self,
        property_name: &str,
        property_type: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if !valid_property_path(property_name)
            || !matches!(property_type, "string" | "number" | "boolean")
        {
            return Err(invalid());
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(|_| unavailable())?;
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM metadata_indexes", [], |row| {
                row.get(0)
            })
            .map_err(|_| corrupt())?;
        if count >= 10 {
            return Err(limit());
        }
        tx.execute(
            "INSERT INTO metadata_indexes(property_name, property_type, created_at_ms)
             VALUES (?1, ?2, ?3)",
            params![property_name, property_type, now_ms],
        )
        .map_err(|_| conflict())?;
        let mut statement = tx
            .prepare(
                "SELECT vector_rowid, metadata_json FROM vectors WHERE metadata_json IS NOT NULL",
            )
            .map_err(|_| corrupt())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|_| corrupt())?;
        let materialized = rows.collect::<Result<Vec<_>, _>>().map_err(|_| corrupt())?;
        drop(statement);
        for (rowid, bytes) in materialized {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
            insert_terms(&tx, rowid, property_name, property_type, &value)?;
        }
        tx.execute("UPDATE index_meta SET metadata_generation = metadata_generation + 1 WHERE singleton = 1", [])
            .map_err(|_| corrupt())?;
        tx.commit().map_err(|_| unavailable())
    }

    /// Return the exact set of indexed metadata property paths.
    pub fn indexed_properties(&self) -> Result<BTreeSet<String>, PlatformError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT property_name FROM metadata_indexes ORDER BY property_name")
            .map_err(|_| corrupt())?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|_| corrupt())?;
        rows.collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| corrupt())
    }

    /// Return every metadata-index declaration in property-name order.
    pub fn metadata_indexes(&self) -> Result<Vec<VectorMetadataIndex>, PlatformError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT property_name, property_type FROM metadata_indexes ORDER BY property_name",
            )
            .map_err(|_| corrupt())?;
        let rows = statement
            .query_map([], |row| {
                Ok(VectorMetadataIndex {
                    property_name: row.get(0)?,
                    index_type: row.get(1)?,
                })
            })
            .map_err(|_| corrupt())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|_| corrupt())
    }

    /// Delete one metadata index and its materialized terms atomically.
    pub fn delete_metadata_index(&self, property_name: &str) -> Result<(), PlatformError> {
        if !valid_property_path(property_name) {
            return Err(invalid());
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(|_| unavailable())?;
        let deleted = tx
            .execute(
                "DELETE FROM metadata_indexes WHERE property_name=?1",
                [property_name],
            )
            .map_err(|_| corrupt())?;
        if deleted != 1 {
            return Err(not_found());
        }
        tx.execute("UPDATE index_meta SET metadata_generation = metadata_generation + 1 WHERE singleton = 1", [])
            .map_err(|_| corrupt())?;
        tx.commit().map_err(|_| unavailable())
    }

    fn validate_batch(
        &self,
        kind: VectorMutationKind,
        items: &[VectorMutationInput],
    ) -> Result<Vec<EncodedMutationItem>, PlatformError> {
        if items.is_empty() || items.len() > MAX_BATCH_ITEMS {
            return Err(limit());
        }
        let mut ids = BTreeSet::new();
        let mut encoded = Vec::with_capacity(items.len());
        for item in items {
            if !ids.insert(item.id.as_str()) {
                continue;
            }
            if !valid_identity(&item.id, MAX_ID_BYTES)
                || item
                    .namespace
                    .as_deref()
                    .is_some_and(|value| !valid_identity(value, MAX_NAMESPACE_BYTES))
            {
                return Err(invalid());
            }
            let values = match (kind, item.values.as_deref()) {
                (VectorMutationKind::Delete, None) => None,
                (VectorMutationKind::Insert | VectorMutationKind::Upsert, Some(values)) => {
                    Some(encode_values(values, self.dimensions)?)
                }
                _ => return Err(invalid()),
            };
            if kind == VectorMutationKind::Delete
                && (item.namespace.is_some() || item.metadata.is_some())
            {
                return Err(invalid());
            }
            let metadata = item.metadata.as_ref().map(canonical_metadata).transpose()?;
            encoded.push((item.id.clone(), item.namespace.clone(), values, metadata));
        }
        Ok(encoded)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, PlatformError> {
        self.connection.lock().map_err(|_| unavailable())
    }
}
