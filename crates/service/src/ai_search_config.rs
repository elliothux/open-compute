//! Canonical tenant-visible AI Search instance configuration.

use open_compute_core::{
    AiConfig, AiGenerationCapability, ErrorCode, PlatformError, ResolvedEmbeddingModelContract,
    ResolvedTokenizerContract,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_CONFIG_BYTES: usize = 64 * 1024;
const DEFAULT_CHUNK_SIZE: u32 = 512;
const DEFAULT_CHUNK_OVERLAP_PERCENT: u8 = 10;
const DEFAULT_SCORE_THRESHOLD: f64 = 0.4;
const DEFAULT_MAX_RESULTS: u8 = 10;

/// Strict Worker-facing configuration accepted by namespace `create()`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiSearchCreateInput {
    /// Instance key in the bound namespace.
    pub id: String,
    /// Query rewrite toggle.
    #[serde(default)]
    pub rewrite_query: bool,
    /// Reranking toggle.
    #[serde(default)]
    pub reranking: bool,
    /// Tenant-visible embedding alias.
    pub embedding_model: Option<String>,
    /// Chat generation model alias.
    pub ai_search_model: Option<String>,
    /// Query rewrite model alias.
    pub rewrite_model: Option<String>,
    /// Reranking model alias.
    pub reranking_model: Option<String>,
    /// Enabled retrieval indexes.
    pub index_method: Option<AiSearchIndexMethod>,
    /// Hybrid fusion method.
    pub fusion_method: Option<AiSearchFusionMethod>,
    /// Keyword index options.
    pub indexing_options: Option<AiSearchIndexingOptions>,
    /// Query defaults.
    pub retrieval_options: Option<AiSearchRetrievalOptions>,
    /// Whether recursive chunking is enabled.
    pub chunk: Option<bool>,
    /// Maximum tokens per chunk.
    pub chunk_size: Option<u32>,
    /// Chunk overlap percentage.
    pub chunk_overlap: Option<u8>,
    /// Default retrieval threshold.
    pub score_threshold: Option<f64>,
    /// Default maximum results.
    pub max_num_results: Option<u8>,
    /// Materialized metadata declarations.
    pub custom_metadata: Option<Vec<AiSearchMetadataField>>,
    /// Instance metadata.
    pub metadata: Option<BTreeMap<String, Value>>,
}

/// Optional vector and keyword index selection.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiSearchIndexMethod {
    /// Enable vector retrieval.
    #[serde(default)]
    pub vector: bool,
    /// Enable FTS keyword retrieval.
    #[serde(default)]
    pub keyword: bool,
}

/// Supported hybrid fusion methods.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSearchFusionMethod {
    /// Maximum normalized branch score.
    Max,
    /// Normalized reciprocal-rank fusion.
    Rrf,
}

/// Keyword index construction options.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiSearchIndexingOptions {
    /// FTS tokenizer.
    pub keyword_tokenizer: Option<AiSearchKeywordTokenizer>,
}

/// Keyword tokenizer supported by the SQLite FTS authority.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSearchKeywordTokenizer {
    /// Unicode porter tokenizer.
    Porter,
    /// SQLite trigram tokenizer.
    Trigram,
}

/// Default query behavior persisted with an instance.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiSearchRetrievalOptions {
    /// Keyword token conjunction mode.
    pub keyword_match_mode: Option<AiSearchKeywordMatchMode>,
}

/// Keyword match mode.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSearchKeywordMatchMode {
    /// Require every query token.
    And,
    /// Accept any query token.
    Or,
}

/// One custom metadata field materialized for filtering.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiSearchMetadataField {
    /// Dot-free field name.
    pub field_name: String,
    /// Declared field type.
    pub data_type: AiSearchMetadataType,
}

/// Custom metadata scalar type.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSearchMetadataType {
    /// String value.
    Text,
    /// Finite JSON number.
    Number,
    /// Boolean value.
    Boolean,
    /// RFC3339 datetime string.
    Datetime,
}

/// Fully resolved, canonical public instance configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedAiSearchConfig {
    /// Instance identity.
    pub id: String,
    /// Query rewrite toggle.
    pub rewrite_query: bool,
    /// Reranking toggle.
    pub reranking: bool,
    /// Resolved public embedding alias, absent for keyword-only instances.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Chat model alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_search_model: Option<String>,
    /// Rewrite model alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite_model: Option<String>,
    /// Rerank model alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reranking_model: Option<String>,
    /// Frozen index selection.
    pub index_method: AiSearchIndexMethod,
    /// Hybrid fusion method.
    pub fusion_method: AiSearchFusionMethod,
    /// Keyword index settings.
    pub indexing_options: AiSearchIndexingOptions,
    /// Query settings.
    pub retrieval_options: AiSearchRetrievalOptions,
    /// Chunking toggle.
    pub chunk: bool,
    /// Token chunk size.
    pub chunk_size: u32,
    /// Overlap percentage.
    pub chunk_overlap: u8,
    /// Default score threshold.
    pub score_threshold: f64,
    /// Default result limit.
    pub max_num_results: u8,
    /// Materialized metadata declarations.
    pub custom_metadata: Vec<AiSearchMetadataField>,
    /// Canonical instance metadata.
    pub metadata: BTreeMap<String, Value>,
}

/// Persistent bytes and vector contract derived from a create request.
#[derive(Clone, Debug)]
pub struct PreparedAiSearchConfig {
    /// Canonical public config JSON.
    pub public_config_json: Vec<u8>,
    /// Canonical secret-free model contract JSON.
    pub model_contract_json: Vec<u8>,
    /// Model contract digest.
    pub model_contract_sha256: [u8; 32],
    /// Exact vector dimensions, or zero for keyword-only.
    pub dimensions: u32,
    /// Vector index toggle.
    pub vector_enabled: bool,
    /// Keyword index toggle.
    pub keyword_enabled: bool,
    /// Resolved embedding contract used to compose the provider client.
    pub embedding_contract: Option<ResolvedEmbeddingModelContract>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeywordOnlyModelContract {
    kind: KeywordOnlyContractKind,
    schema_version: u8,
    tokenizer_contract: ResolvedTokenizerContract,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum KeywordOnlyContractKind {
    KeywordOnly,
}

pub(crate) fn parse_keyword_only_tokenizer_contract(
    bytes: &[u8],
) -> Result<ResolvedTokenizerContract, PlatformError> {
    let contract: KeywordOnlyModelContract =
        serde_json::from_slice(bytes).map_err(|_| input_invalid())?;
    if contract.schema_version != 1 {
        return Err(input_invalid());
    }
    Ok(contract.tokenizer_contract)
}

impl AiSearchCreateInput {
    /// Validate against the operator catalog and resolve immutable defaults.
    pub fn prepare(self, catalog: &AiConfig) -> Result<PreparedAiSearchConfig, PlatformError> {
        validate_instance_id(&self.id)?;
        catalog.validate()?;
        let index = self.index_method.unwrap_or(AiSearchIndexMethod {
            vector: true,
            keyword: false,
        });
        if !index.vector && !index.keyword {
            return Err(input_invalid());
        }
        if (!index.keyword && (self.indexing_options.is_some() || self.retrieval_options.is_some()))
            || (!(index.vector && index.keyword) && self.fusion_method.is_some())
        {
            return Err(option_unsupported());
        }
        let embedding = if index.vector {
            Some(catalog.resolve_embedding_model(self.embedding_model.as_deref())?)
        } else {
            None
        };
        let keyword_tokenizer = if embedding.is_none() {
            Some(catalog.resolve_tokenizer(self.embedding_model.as_deref())?)
        } else {
            None
        };
        let ai_search_model = self
            .ai_search_model
            .or_else(|| catalog.default_generation_model.clone());
        validate_generation_model(
            catalog,
            ai_search_model.as_deref(),
            AiGenerationCapability::Chat,
            false,
        )?;
        validate_generation_model(
            catalog,
            self.rewrite_model.as_deref().or(ai_search_model.as_deref()),
            AiGenerationCapability::Rewrite,
            self.rewrite_query,
        )?;
        validate_generation_model(
            catalog,
            self.reranking_model.as_deref(),
            AiGenerationCapability::Rerank,
            self.reranking,
        )?;
        let chunk_enabled = self.chunk.unwrap_or(true);
        if !chunk_enabled && (self.chunk_size.is_some() || self.chunk_overlap.is_some()) {
            return Err(option_unsupported());
        }
        let chunk_size = self.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
        let max_input_tokens = embedding.as_ref().map_or_else(
            || {
                keyword_tokenizer
                    .as_ref()
                    .map_or(0, |contract| contract.max_input_tokens)
            },
            |contract| contract.max_input_tokens,
        );
        if chunk_size == 0 || chunk_size > max_input_tokens {
            return Err(limit());
        }
        let chunk_overlap = self.chunk_overlap.unwrap_or(DEFAULT_CHUNK_OVERLAP_PERCENT);
        if chunk_overlap > 30 {
            return Err(limit());
        }
        let score_threshold = self.score_threshold.unwrap_or(DEFAULT_SCORE_THRESHOLD);
        if !score_threshold.is_finite() || !(0.0..=1.0).contains(&score_threshold) {
            return Err(input_invalid());
        }
        let max_num_results = self.max_num_results.unwrap_or(DEFAULT_MAX_RESULTS);
        if !(1..=50).contains(&max_num_results) {
            return Err(limit());
        }
        let custom_metadata = self.custom_metadata.unwrap_or_default();
        validate_custom_metadata(&custom_metadata)?;
        let metadata = self.metadata.unwrap_or_default();
        validate_json(&Value::Object(metadata.clone().into_iter().collect()), 0, 0)?;
        let resolved = ResolvedAiSearchConfig {
            id: self.id,
            rewrite_query: self.rewrite_query,
            reranking: self.reranking,
            embedding_model: embedding.as_ref().map_or_else(
                || {
                    keyword_tokenizer
                        .as_ref()
                        .map(|contract| contract.embedding_alias.clone())
                },
                |contract| Some(contract.embedding_alias.clone()),
            ),
            ai_search_model,
            rewrite_model: self.rewrite_model,
            reranking_model: self.reranking_model,
            index_method: index,
            fusion_method: self.fusion_method.unwrap_or(AiSearchFusionMethod::Rrf),
            indexing_options: self.indexing_options.unwrap_or(AiSearchIndexingOptions {
                keyword_tokenizer: Some(AiSearchKeywordTokenizer::Porter),
            }),
            retrieval_options: self.retrieval_options.unwrap_or(AiSearchRetrievalOptions {
                keyword_match_mode: Some(AiSearchKeywordMatchMode::And),
            }),
            chunk: chunk_enabled,
            chunk_size,
            chunk_overlap,
            score_threshold,
            max_num_results,
            custom_metadata,
            metadata,
        };
        let public_config_json = serde_json::to_vec(&resolved).map_err(|_| input_invalid())?;
        if public_config_json.len() > MAX_CONFIG_BYTES {
            return Err(limit());
        }
        let model_contract_json = match (&embedding, keyword_tokenizer) {
            (Some(contract), None) => serde_json::to_vec(contract),
            (None, Some(tokenizer_contract)) => serde_json::to_vec(&KeywordOnlyModelContract {
                kind: KeywordOnlyContractKind::KeywordOnly,
                schema_version: 1,
                tokenizer_contract,
            }),
            _ => return Err(input_invalid()),
        }
        .map_err(|_| input_invalid())?;
        let model_contract_sha256 = Sha256::digest(&model_contract_json).into();
        Ok(PreparedAiSearchConfig {
            public_config_json,
            model_contract_json,
            model_contract_sha256,
            dimensions: embedding.as_ref().map_or(0, |contract| contract.dimensions),
            vector_enabled: index.vector,
            keyword_enabled: index.keyword,
            embedding_contract: embedding,
        })
    }
}

fn validate_generation_model(
    catalog: &AiConfig,
    alias: Option<&str>,
    capability: AiGenerationCapability,
    required: bool,
) -> Result<(), PlatformError> {
    let Some(alias) = alias else {
        return if required {
            Err(option_unsupported())
        } else {
            Ok(())
        };
    };
    let model = catalog
        .generation_models
        .get(alias)
        .ok_or_else(option_unsupported)?;
    if !model.capabilities.contains(&capability) {
        return Err(option_unsupported());
    }
    Ok(())
}

fn validate_instance_id(value: &str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > 32
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
    {
        return Err(input_invalid());
    }
    Ok(())
}

fn validate_custom_metadata(fields: &[AiSearchMetadataField]) -> Result<(), PlatformError> {
    if fields.len() > 5 {
        return Err(limit());
    }
    let mut names = BTreeSet::new();
    for field in fields {
        if field.field_name.is_empty()
            || field.field_name.len() > 256
            || field.field_name.contains('.')
            || field.field_name.starts_with('$')
            || field.field_name.chars().any(char::is_control)
            || !names.insert(&field.field_name)
        {
            return Err(input_invalid());
        }
    }
    Ok(())
}

fn validate_json(value: &Value, depth: usize, nodes: usize) -> Result<usize, PlatformError> {
    if depth > 16 || nodes >= 10_000 {
        return Err(limit());
    }
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(nodes + 1),
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => Ok(nodes + 1),
        Value::Number(_) => Err(input_invalid()),
        Value::Array(values) => values.iter().try_fold(nodes + 1, |count, value| {
            validate_json(value, depth + 1, count)
        }),
        Value::Object(values) => values.values().try_fold(nodes + 1, |count, value| {
            validate_json(value, depth + 1, count)
        }),
    }
}

fn input_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "AI Search configuration is invalid",
    )
}

fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingLimitExceeded,
        "AI Search configuration exceeds a fixed limit",
    )
}

fn option_unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingCapabilityUnsupported,
        "AI Search option is unsupported by the configured model catalog",
    )
}

#[cfg(test)]
#[path = "ai_search_config_tests.rs"]
mod tests;
