//! Per-instance SQLite authority for AI Search indexing.

mod batch;
mod catalog;
mod config;
mod ingest_gc;
mod inspection;
mod jobs;
mod model;
mod paths;
mod query;

pub use catalog::{AiSearchCatalog, AiSearchInstanceRecord, AiSearchNamespaceRecord};
pub use inspection::{inspect_ai_search_instance, inspect_ai_search_object_references};
pub use model::{
    AiSearchChunkRecord, AiSearchInstanceAuthority, AiSearchInstanceInspection,
    AiSearchInstanceStorageContract, AiSearchItemRecord, AiSearchJobClaim, AiSearchJobRecord,
    AiSearchLogRecord, AiSearchObjectGcClaim, AiSearchObjectReference, ClaimedAiSearchItem,
    NewAiSearchItemGeneration, StagedAiSearchChunk,
};
pub use paths::AiSearchPaths;

use open_compute_core::{ErrorCode, PlatformError};
use rand::TryRngCore as _;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Current Day1 per-instance AI Search schema version.
pub const AI_SEARCH_SCHEMA_VERSION: u32 = 1;
const SCHEMA: &str = include_str!("schema.sql");
const MAX_ITEMS_PER_INSTANCE: i64 = 10_000;
const MAX_CHUNKS_PER_ITEM: usize = 10_000;
const MAX_CHUNKS_PER_INSTANCE: i64 = 100_000;
const MAX_SOURCE_BYTES_PER_INSTANCE: i64 = 1_073_741_824;
const MAX_QUEUED_JOBS_PER_INSTANCE: i64 = 10_000;
const MAX_QUEUED_BYTES_PER_INSTANCE: i64 = 1_073_741_824;
const MAX_EXTRACTED_TEXT_BYTES_PER_ITEM: usize = 16 * 1024 * 1024;
const MAX_INDEX_LOGICAL_BYTES: i64 = 1_073_741_824;
const MAX_TERMINAL_JOB_HISTORY: i64 = 1_000;
const MAX_LOG_HISTORY: i64 = 1_000;

/// Authoritative per-instance AI Search database.
#[derive(Debug)]
pub struct AiSearchStore {
    connection: Mutex<Connection>,
    dimensions: usize,
    vector_enabled: bool,
    active_dimensions: usize,
    active_vector_enabled: bool,
}

impl AiSearchStore {
    /// Exact embedding dimensions frozen into this instance, or zero for keyword-only.
    #[must_use]
    pub const fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Whether this instance requires an embedding for every activated chunk.
    #[must_use]
    pub const fn vector_enabled(&self) -> bool {
        self.vector_enabled
    }

    /// Open or create a database and verify its immutable instance identity and
    /// model contract.
    pub fn open(
        path: &Path,
        contract: &AiSearchInstanceStorageContract<'_>,
        now_ms: i64,
    ) -> Result<Self, PlatformError> {
        validate_identity(contract.resource_id)?;
        if (!contract.vector_enabled && contract.dimensions != 0)
            || (contract.vector_enabled && contract.dimensions == 0)
            || (!contract.vector_enabled && !contract.keyword_enabled)
        {
            return Err(limit_error());
        }
        let model_digest: [u8; 32] = Sha256::digest(contract.model_contract_json).into();
        if model_digest != contract.model_contract_sha256 || !valid_instance_contract(contract) {
            return Err(invariant_error());
        }
        let parent = path.parent().ok_or_else(invariant_error)?;
        crate::fs::validate_owned_dir(parent)?;
        crate::fs::ensure_file_secure(path)?;
        let file = crate::fs::open_nofollow(path, false, true)?;
        crate::fs::validate_authority_fd(&file)?;
        drop(file);
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; \
                 PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
            )
            .map_err(sql_error)?;
        connection.execute_batch(SCHEMA).map_err(sql_error)?;
        connection
            .execute(
                "INSERT OR IGNORE INTO instance_meta
                 (singleton, schema_version, resource_id, model_contract_sha256,
                  previous_model_contract_sha256, transition_model_contract_sha256,
                  previous_model_contract_json,
                  previous_public_config_json, previous_dimensions,
                  previous_vector_enabled, previous_keyword_enabled,
                  model_contract_json, public_config_json, dimensions, vector_enabled,
                  keyword_enabled, active_index_generation, active_epoch,
                  config_generation, created_at_ms, updated_at_ms)
                 VALUES (1, ?1, ?2, ?3, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                         ?4, ?5, ?6, ?7, ?8, 1, 1, 1, ?9, ?9)",
                params![
                    i64::from(AI_SEARCH_SCHEMA_VERSION),
                    contract.resource_id,
                    contract.model_contract_sha256,
                    contract.model_contract_json,
                    contract.public_config_json,
                    i64::from(contract.dimensions),
                    contract.vector_enabled,
                    contract.keyword_enabled,
                    now_ms
                ],
            )
            .map_err(sql_error)?;
        let matches: bool = connection
            .query_row(
                "SELECT schema_version=?1 AND resource_id=?2 AND model_contract_sha256=?3
                   AND model_contract_json=?4 AND public_config_json=?5
                   AND dimensions=?6 AND vector_enabled=?7 AND keyword_enabled=?8
                 FROM instance_meta WHERE singleton=1",
                params![
                    i64::from(AI_SEARCH_SCHEMA_VERSION),
                    contract.resource_id,
                    contract.model_contract_sha256,
                    contract.model_contract_json,
                    contract.public_config_json,
                    i64::from(contract.dimensions),
                    contract.vector_enabled,
                    contract.keyword_enabled
                ],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if !matches {
            return Err(invariant_error());
        }
        let active: (i64, bool) = connection
            .query_row(
                "SELECT COALESCE(previous_dimensions, dimensions),
                        COALESCE(previous_vector_enabled, vector_enabled)
                   FROM instance_meta WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sql_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
            dimensions: usize::try_from(contract.dimensions).map_err(|_| limit_error())?,
            vector_enabled: contract.vector_enabled,
            active_dimensions: usize::try_from(active.0).map_err(|_| invariant_error())?,
            active_vector_enabled: active.1,
        })
    }

    /// Checkpoint committed WAL state before snapshot or lifecycle quarantine.
    pub fn checkpoint(&self, truncate: bool) -> Result<(), PlatformError> {
        let mode = if truncate { "TRUNCATE" } else { "PASSIVE" };
        self.lock()?
            .query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |_| Ok(()))
            .map_err(sql_error)
    }

    /// Create one user job and atomically queue a new item generation.
    pub fn enqueue_item_generation(
        &self,
        job_id: &str,
        item: &NewAiSearchItemGeneration<'_>,
    ) -> Result<(), PlatformError> {
        validate_identity(job_id)?;
        validate_item(item)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        enforce_enqueue_quotas(&transaction, item)?;
        prune_terminal_jobs(&transaction)?;
        let config_generation: i64 = transaction
            .query_row(
                "SELECT config_generation FROM instance_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO index_jobs
                 (id, source, state, config_generation, index_generation, attempt,
                  next_attempt_at_ms, cancel_requested, created_at_ms, updated_at_ms)
                 VALUES (?1, 'user', 'queued', ?2, ?3, 0, ?4, 0, ?4, ?4)",
                params![
                    job_id,
                    config_generation,
                    to_i64(item.index_generation)?,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO items
                 (id, source, key, status, desired_generation, metadata_json,
                  created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?6)
                 ON CONFLICT(source, key) DO UPDATE SET
                   status='queued', desired_generation=excluded.desired_generation,
                   metadata_json=excluded.metadata_json, updated_at_ms=excluded.updated_at_ms",
                params![
                    item.item_id,
                    item.source,
                    item.key,
                    to_i64(item.generation)?,
                    item.metadata_json,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        let stored_id: String = transaction
            .query_row(
                "SELECT id FROM items WHERE source=?1 AND key=?2",
                params![item.source, item.key],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if stored_id != item.item_id {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "AI Search item identity changed for an existing source key",
            ));
        }
        transaction
            .execute(
                "INSERT INTO item_generations
                 (item_id, generation, index_generation, state, object_key,
                  object_sha256, object_size, content_type, created_at_ms)
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, ?8)",
                params![
                    item.item_id,
                    to_i64(item.generation)?,
                    to_i64(item.index_generation)?,
                    item.object_key,
                    item.object_sha256,
                    to_i64(item.object_size)?,
                    item.content_type,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO index_job_items
                 (job_id, item_id, item_generation, index_generation, state,
                  next_batch_ordinal, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'queued', 0, ?5)",
                params![
                    job_id,
                    item.item_id,
                    to_i64(item.generation)?,
                    to_i64(item.index_generation)?,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        append_item_log(&transaction, item.item_id, "queued", item.now_ms)?;
        append_job_log(&transaction, job_id, "queued", 0, item.now_ms)?;
        ingest_gc::queue_unreferenced_objects(&transaction, item.now_ms)?;
        prune_generation_history(&transaction)?;
        transaction.commit().map_err(sql_error)
    }

    fn validate_chunks(&self, chunks: &[StagedAiSearchChunk<'_>]) -> Result<(), PlatformError> {
        validate_chunk_batch(chunks, 0, self.vector_enabled, self.dimensions)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, PlatformError> {
        self.connection.lock().map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "AI Search database lock is poisoned",
            )
        })
    }
}

fn decode_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiSearchItemRecord> {
    let digest: Vec<u8> = row.get(9)?;
    let object_sha256 = digest
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(AiSearchItemRecord {
        id: row.get(0)?,
        key: row.get(1)?,
        status: row.get(2)?,
        active_generation: row.get(3)?,
        desired_generation: row.get(4)?,
        metadata_json: row.get(5)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
        object: AiSearchObjectReference {
            object_key: row.get(8)?,
            object_sha256,
            object_size: row.get(10)?,
        },
        content_type: row.get(11)?,
        chunks_count: row.get(12)?,
    })
}

fn decode_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiSearchJobRecord> {
    Ok(AiSearchJobRecord {
        id: row.get(0)?,
        source: row.get(1)?,
        description: row.get(2)?,
        state: row.get(3)?,
        created_at_ms: row.get(4)?,
        started_at_ms: row.get(5)?,
        ended_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

fn decode_chunk(
    row: &rusqlite::Row<'_>,
    dimensions: usize,
    vector_enabled: bool,
) -> rusqlite::Result<AiSearchChunkRecord> {
    let bytes: Option<Vec<u8>> = row.get(6)?;
    let embedding = bytes.map(|bytes| {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|part| f32::from_le_bytes([part[0], part[1], part[2], part[3]]))
            .collect::<Vec<_>>()
    });
    if embedding
        .as_ref()
        .is_some_and(|value| value.len() != dimensions)
        || vector_enabled != embedding.is_some()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(AiSearchChunkRecord {
        id: row.get(0)?,
        item_id: row.get(1)?,
        ordinal: row.get(2)?,
        start_byte: row.get(3)?,
        end_byte: row.get(4)?,
        text: row.get(5)?,
        embedding,
        metadata_json: row.get(7)?,
        item_key: row.get(8)?,
        item_created_at_ms: row.get(9)?,
    })
}

fn decode_log(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiSearchLogRecord> {
    Ok(AiSearchLogRecord {
        sequence: row.get(0)?,
        message_code: row.get(1)?,
        message_type: row.get(2)?,
        created_at_ms: row.get(3)?,
    })
}

fn decode_logs(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<AiSearchLogRecord>,
    >,
) -> Result<Vec<AiSearchLogRecord>, PlatformError> {
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(sql_error)?);
    }
    Ok(output)
}

fn validate_identity(value: &str) -> Result<(), PlatformError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(limit_error());
    }
    Ok(())
}

fn validate_item(item: &NewAiSearchItemGeneration<'_>) -> Result<(), PlatformError> {
    validate_identity(item.item_id)?;
    if item.key.is_empty()
        || item.key.len() > 1024
        || item.source.is_empty()
        || item.source.len() > 256
        || item.generation == 0
        || item.index_generation == 0
        || item.object_key.is_empty()
        || item.object_size == 0
        || item.object_size > 4_194_304
        || item.content_type.is_empty()
        || !canonical_json_object(item.metadata_json, 65_536)
    {
        return Err(limit_error());
    }
    Ok(())
}

fn validate_chunk_batch(
    chunks: &[StagedAiSearchChunk<'_>],
    first_ordinal: u32,
    vector_enabled: bool,
    dimensions: usize,
) -> Result<(), PlatformError> {
    if chunks.len() > MAX_CHUNKS_PER_ITEM {
        return Err(limit_error());
    }
    let mut text_bytes = 0_usize;
    for (ordinal, chunk) in chunks.iter().enumerate() {
        validate_identity(chunk.chunk_id)?;
        let expected = usize::try_from(first_ordinal)
            .ok()
            .and_then(|first| first.checked_add(ordinal));
        if usize::try_from(chunk.ordinal).ok() != expected
            || chunk.end_byte < chunk.start_byte
            || chunk.text.is_empty()
            || !canonical_json_object(chunk.metadata_json, 65_536)
        {
            return Err(limit_error());
        }
        text_bytes = text_bytes
            .checked_add(chunk.text.len())
            .ok_or_else(limit_error)?;
        if text_bytes > MAX_EXTRACTED_TEXT_BYTES_PER_ITEM {
            return Err(quota_error());
        }
        match (vector_enabled, chunk.embedding_f32le, chunk.vector_norm) {
            (false, None, None) => {}
            (true, Some(bytes), Some(norm)) => {
                let expected = dimensions.checked_mul(4).ok_or_else(limit_error)?;
                if bytes.len() != expected || !norm.is_finite() || norm <= 0.0 {
                    return Err(limit_error());
                }
                let mut squared = 0.0_f64;
                for value in bytes.as_chunks::<4>().0 {
                    let value = f32::from_le_bytes([value[0], value[1], value[2], value[3]]);
                    if !value.is_finite() {
                        return Err(limit_error());
                    }
                    squared += f64::from(value) * f64::from(value);
                }
                let actual = squared.sqrt();
                if actual == 0.0 || (actual - norm).abs() > 1e-9 * actual.max(norm) {
                    return Err(limit_error());
                }
            }
            _ => return Err(limit_error()),
        }
    }
    Ok(())
}

fn enforce_enqueue_quotas(
    transaction: &rusqlite::Transaction<'_>,
    item: &NewAiSearchItemGeneration<'_>,
) -> Result<(), PlatformError> {
    let existing: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE source=?1 AND key=?2)",
            params![item.source, item.key],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let item_count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
        .map_err(sql_error)?;
    if (!existing && item_count >= MAX_ITEMS_PER_INSTANCE)
        || pending_job_count(transaction)? >= MAX_QUEUED_JOBS_PER_INSTANCE
    {
        return Err(quota_error());
    }
    let retained_source_bytes: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(g.object_size), 0)
               FROM items i JOIN item_generations g
                 ON g.item_id=i.id AND g.generation=i.desired_generation
              WHERE NOT (i.source=?1 AND i.key=?2)",
            params![item.source, item.key],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let object_size = to_i64(item.object_size)?;
    if retained_source_bytes
        .checked_add(object_size)
        .is_none_or(|bytes| bytes > MAX_SOURCE_BYTES_PER_INSTANCE)
    {
        return Err(quota_error());
    }
    let queued_bytes: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(g.object_size), 0)
               FROM index_jobs j JOIN index_job_items ji ON ji.job_id=j.id
               JOIN item_generations g
                 ON g.item_id=ji.item_id AND g.generation=ji.item_generation
              WHERE j.state IN ('queued','claimed','retry_wait','cancelling')",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if queued_bytes
        .checked_add(object_size)
        .is_none_or(|bytes| bytes > MAX_QUEUED_BYTES_PER_INSTANCE)
    {
        return Err(quota_error());
    }
    Ok(())
}

fn pending_job_count(transaction: &rusqlite::Transaction<'_>) -> Result<i64, PlatformError> {
    transaction
        .query_row(
            "SELECT COUNT(*) FROM index_jobs
              WHERE state IN ('queued','claimed','retry_wait','cancelling')",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)
}

fn prune_terminal_jobs(transaction: &rusqlite::Transaction<'_>) -> Result<(), PlatformError> {
    transaction
        .execute(
            "DELETE FROM index_jobs WHERE id IN (
               SELECT id FROM index_jobs
                WHERE state IN ('completed','error','cancelled','outdated')
                ORDER BY updated_at_ms DESC, id DESC LIMIT -1 OFFSET ?1)",
            [MAX_TERMINAL_JOB_HISTORY],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn prune_generation_history(transaction: &rusqlite::Transaction<'_>) -> Result<(), PlatformError> {
    transaction
        .execute(
            "DELETE FROM item_generations AS generation
               WHERE generation.generation NOT IN (
                 SELECT active_generation FROM items WHERE id=generation.item_id
                 UNION
                 SELECT desired_generation FROM items WHERE id=generation.item_id)
                 AND NOT EXISTS (
                   SELECT 1 FROM index_job_items ji
                    WHERE ji.item_id=generation.item_id
                      AND ji.item_generation=generation.generation)",
            [],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn append_item_log(
    transaction: &rusqlite::Transaction<'_>,
    item_id: &str,
    message_code: &str,
    now_ms: i64,
) -> Result<(), PlatformError> {
    transaction
        .execute(
            "INSERT INTO item_logs(item_id, action, message_code, created_at_ms)
             VALUES (?1, 'index', ?2, ?3)",
            params![item_id, message_code, now_ms],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "DELETE FROM item_logs WHERE item_id=?1 AND sequence NOT IN
               (SELECT sequence FROM item_logs WHERE item_id=?1
                 ORDER BY sequence DESC LIMIT ?2)",
            params![item_id, MAX_LOG_HISTORY],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn append_job_log(
    transaction: &rusqlite::Transaction<'_>,
    job_id: &str,
    message_code: &str,
    message_type: i64,
    now_ms: i64,
) -> Result<(), PlatformError> {
    transaction
        .execute(
            "INSERT INTO job_logs(job_id, message_code, message_type, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![job_id, message_code, message_type, now_ms],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "DELETE FROM job_logs WHERE job_id=?1 AND sequence NOT IN
               (SELECT sequence FROM job_logs WHERE job_id=?1
                 ORDER BY sequence DESC LIMIT ?2)",
            params![job_id, MAX_LOG_HISTORY],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn canonical_json_object(bytes: &[u8], max_bytes: usize) -> bool {
    if bytes.len() > max_bytes {
        return false;
    }
    let Ok(object) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(bytes)
    else {
        return false;
    };
    serde_json::to_vec(&object).is_ok_and(|canonical| canonical == bytes)
}

fn valid_instance_contract(contract: &AiSearchInstanceStorageContract<'_>) -> bool {
    if contract.public_config_json.len() > 65_536 || contract.model_contract_json.len() > 65_536 {
        return false;
    }
    let Ok(public) = serde_json::from_slice::<serde_json::Value>(contract.public_config_json)
    else {
        return false;
    };
    let Ok(model) = serde_json::from_slice::<serde_json::Value>(contract.model_contract_json)
    else {
        return false;
    };
    let Some(public) = public.as_object() else {
        return false;
    };
    let index = public
        .get("index_method")
        .and_then(serde_json::Value::as_object);
    let vector = index
        .and_then(|index| index.get("vector"))
        .and_then(serde_json::Value::as_bool);
    let keyword = index
        .and_then(|index| index.get("keyword"))
        .and_then(serde_json::Value::as_bool);
    let valid_public = vector == Some(contract.vector_enabled)
        && keyword == Some(contract.keyword_enabled)
        && public
            .get("chunk")
            .is_some_and(serde_json::Value::is_boolean)
        && public
            .get("chunk_size")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| value > 0)
        && public
            .get("chunk_overlap")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| value <= 30)
        && public
            .get("score_threshold")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        && public
            .get("max_num_results")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| (1..=50).contains(&value))
        && public
            .get("fusion_method")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| matches!(value, "max" | "rrf"))
        && public
            .get("custom_metadata")
            .is_some_and(serde_json::Value::is_array)
        && public
            .get("metadata")
            .is_some_and(serde_json::Value::is_object);
    if !valid_public {
        return false;
    }
    let Some(model) = model.as_object() else {
        return false;
    };
    if contract.vector_enabled {
        model.get("dimensions").and_then(serde_json::Value::as_u64)
            == Some(u64::from(contract.dimensions))
            && model.get("metric").and_then(serde_json::Value::as_str) == Some("cosine")
            && model
                .get("tokenizer")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.is_empty())
            && model
                .get("tokenizerRevision")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.is_empty())
            && model
                .get("tokenizerArtifactSha256")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| {
                    value.len() == 64
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
    } else {
        model.get("kind").and_then(serde_json::Value::as_str) == Some("keyword_only")
            && model
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64)
                == Some(1)
            && model
                .get("tokenizerContract")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|tokenizer| {
                    tokenizer
                        .get("embeddingAlias")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.is_empty())
                        && tokenizer
                            .get("tokenizer")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|value| !value.is_empty())
                        && tokenizer
                            .get("tokenizerRevision")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|value| !value.is_empty())
                        && tokenizer
                            .get("tokenizerArtifactSha256")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|value| {
                                value.len() == 64
                                    && value.bytes().all(|byte| {
                                        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                                    })
                            })
                        && tokenizer
                            .get("maxInputTokens")
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|value| value > 0)
                        && tokenizer
                            .get("contractSha256")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|value| !value.is_empty())
                })
    }
}

fn to_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| limit_error())
}

fn to_u64(value: i64) -> Result<u64, PlatformError> {
    u64::try_from(value).map_err(|_| invariant_error())
}

fn sql_error(_: rusqlite::Error) -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "AI Search database operation failed",
    )
}

fn invariant_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "AI Search persisted authority is inconsistent",
    )
}

fn limit_error() -> PlatformError {
    PlatformError::new(ErrorCode::LimitInvalid, "AI Search input exceeds a limit")
}

fn quota_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::QuotaExceeded,
        "AI Search instance quota was exceeded",
    )
}

#[cfg(test)]
mod tests;
