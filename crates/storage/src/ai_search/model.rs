//! Public per-instance AI Search persistence value types.

use serde::Serialize;

/// Immutable input needed to queue one item generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAiSearchItemGeneration<'a> {
    /// Stable item identity.
    pub item_id: &'a str,
    /// Source-scoped object key.
    pub key: &'a str,
    /// Canonical source identity.
    pub source: &'a str,
    /// New one-based item generation.
    pub generation: u64,
    /// Frozen full-index generation.
    pub index_generation: u64,
    /// Immutable system-object key.
    pub object_key: &'a str,
    /// Exact object SHA-256.
    pub object_sha256: [u8; 32],
    /// Exact object byte length.
    pub object_size: u64,
    /// Canonical content type.
    pub content_type: &'a str,
    /// Canonical metadata JSON object.
    pub metadata_json: &'a [u8],
    /// Mutation timestamp.
    pub now_ms: i64,
}

/// One durable indexing claim and its fencing values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchJobClaim {
    /// Stable job identity.
    pub job_id: String,
    /// Secret claim fence, never suitable for logs or public responses.
    pub claim_token: [u8; 32],
    /// One-based claim attempt.
    pub attempt: u32,
    /// Lease expiration time.
    pub claim_until_ms: i64,
    /// Frozen instance config generation.
    pub config_generation: u64,
    /// Frozen full-index generation.
    pub index_generation: u64,
    /// First chunk ordinal not yet durably staged for this claim.
    pub next_batch_ordinal: u32,
    /// Exact item generation owned by this job.
    pub item: ClaimedAiSearchItem,
}

/// One source object and document identity frozen into an indexing claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedAiSearchItem {
    /// Stable item identity.
    pub item_id: String,
    /// Source-facing filename or key.
    pub key: String,
    /// One-based item generation.
    pub generation: u64,
    /// Exact system S3 object key.
    pub object_key: String,
    /// Exact object SHA-256.
    pub object_sha256: [u8; 32],
    /// Exact object bytes.
    pub object_size: u64,
    /// Canonical declared content type.
    pub content_type: String,
    /// Canonical item metadata JSON.
    pub metadata_json: Vec<u8>,
}

/// One exact object deletion claim fenced by a random lease token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchObjectGcClaim {
    /// Exact immutable system S3 key.
    pub object_key: String,
    /// Exact object SHA-256.
    pub object_sha256: [u8; 32],
    /// Exact object byte length.
    pub object_size: u64,
    /// Secret claim fence, never suitable for logs or responses.
    pub claim_token: [u8; 32],
    /// One-based deletion attempt.
    pub attempt: u32,
    /// Lease expiration in Unix milliseconds.
    pub claim_until_ms: i64,
}

/// One staged chunk ready for an activation transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedAiSearchChunk<'a> {
    /// Stable content/config-derived identity.
    pub chunk_id: &'a str,
    /// Zero-based chunk ordinal.
    pub ordinal: u32,
    /// Start offset in normalized UTF-8 text.
    pub start_byte: u64,
    /// Exclusive end offset in normalized UTF-8 text.
    pub end_byte: u64,
    /// Normalized chunk text.
    pub text: &'a str,
    /// Exact little-endian f32 embedding bytes, absent for keyword-only indexes.
    pub embedding_f32le: Option<&'a [u8]>,
    /// Precomputed finite vector norm, absent for keyword-only indexes.
    pub vector_norm: Option<f64>,
    /// Canonical materialized metadata JSON object.
    pub metadata_json: &'a [u8],
}

/// Frozen persistence contract for one AI Search instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchInstanceStorageContract<'a> {
    /// Stable resource identity.
    pub resource_id: &'a str,
    /// SHA-256 of the resolved embedding model contract.
    pub model_contract_sha256: [u8; 32],
    /// Canonical JSON for the complete immutable model contract.
    pub model_contract_json: &'a [u8],
    /// Canonical JSON for the validated public instance configuration.
    pub public_config_json: &'a [u8],
    /// Exact vector dimensions, or zero for a keyword-only instance.
    pub dimensions: u32,
    /// Whether vector chunks are stored and queried.
    pub vector_enabled: bool,
    /// Whether one of the FTS indexes is active.
    pub keyword_enabled: bool,
}

/// Secret-free instance authority summary used by info, health, and coordinator reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchInstanceInspection {
    /// Frozen public configuration JSON.
    pub public_config_json: Vec<u8>,
    /// Frozen model contract JSON.
    pub model_contract_json: Vec<u8>,
    /// Contract used for pending indexing work; differs only during full reindex.
    pub indexing_model_contract_json: Vec<u8>,
    /// Public configuration used for pending indexing work.
    pub indexing_public_config_json: Vec<u8>,
    /// Current public config generation.
    pub config_generation: u64,
    /// Active full-index generation.
    pub active_index_generation: u64,
    /// Monotonic fence advanced whenever any active item generation changes.
    pub active_epoch: u64,
    /// Number of catalog items.
    pub item_count: u64,
    /// Number of active chunks.
    pub active_chunk_count: u64,
    /// Number of due or executing jobs.
    pub pending_job_count: u64,
    /// Whether a full reindex contract is fenced from the still-active index.
    pub reindex_pending: bool,
}

/// One exact immutable system-object reference retained by this instance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchObjectReference {
    /// Exact system S3 key.
    pub object_key: String,
    /// Exact object SHA-256.
    pub object_sha256: [u8; 32],
    /// Exact logical object bytes.
    pub object_size: u64,
}

/// Read-only verified instance authority used during startup reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchInstanceAuthority {
    /// Stable resource identity stored inside the database.
    pub resource_id: String,
    /// Frozen model contract digest.
    pub model_contract_sha256: [u8; 32],
    /// Exact vector dimensions, or zero for keyword-only.
    pub dimensions: u32,
    /// Whether vector indexing is active.
    pub vector_enabled: bool,
    /// Whether keyword indexing is active.
    pub keyword_enabled: bool,
    /// Secret-free current instance inspection.
    pub inspection: AiSearchInstanceInspection,
}

/// One bounded item catalog row returned to the private AI Search backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchItemRecord {
    /// Stable item identity.
    pub id: String,
    /// Source-local object key or filename.
    pub key: String,
    /// Durable indexing status.
    pub status: String,
    /// Active generation, when indexing has completed.
    pub active_generation: Option<u64>,
    /// Desired generation.
    pub desired_generation: u64,
    /// Canonical custom metadata JSON.
    pub metadata_json: Vec<u8>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Last mutation timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
    /// Active or desired immutable source object.
    pub object: AiSearchObjectReference,
    /// Declared source content type.
    pub content_type: String,
    /// Number of active chunks.
    pub chunks_count: u64,
}

/// One bounded indexing job summary returned to the private backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchJobRecord {
    /// Stable opaque job identity.
    pub id: String,
    /// Job source token.
    pub source: String,
    /// Optional user description.
    pub description: Option<String>,
    /// Durable internal state.
    pub state: String,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// First claim timestamp.
    pub started_at_ms: Option<i64>,
    /// Terminal timestamp.
    pub ended_at_ms: Option<i64>,
    /// Last mutation timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// One active indexed chunk with its source item context.
#[derive(Clone, Debug, PartialEq)]
pub struct AiSearchChunkRecord {
    /// Stable chunk identity.
    pub id: String,
    /// Owning item identity.
    pub item_id: String,
    /// Zero-based ordinal within the item.
    pub ordinal: u32,
    /// Start byte in normalized text.
    pub start_byte: u64,
    /// Exclusive end byte in normalized text.
    pub end_byte: u64,
    /// Normalized chunk text.
    pub text: String,
    /// Decoded exact embedding, when vector indexing is enabled.
    pub embedding: Option<Vec<f32>>,
    /// Canonical chunk metadata JSON.
    pub metadata_json: Vec<u8>,
    /// Source object key or filename.
    pub item_key: String,
    /// Item creation timestamp.
    pub item_created_at_ms: i64,
}

/// One sanitized retained log row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSearchLogRecord {
    /// Monotonic per-database sequence.
    pub sequence: u64,
    /// Stable message code, never source text or provider content.
    pub message_code: String,
    /// Numeric message severity/type.
    pub message_type: u32,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
}
