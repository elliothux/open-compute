//! Operator-owned AI backends, model mappings, and immutable embedding profiles.

mod backend;
mod vlm;

use crate::{ErrorCode, PlatformError};
pub use backend::{AiAuthConfig, AiBackendConfig, AiBackendProtocol};
use backend::{canonical_endpoint, headers_digest};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
pub use vlm::{AiVlmModelConfig, ResolvedVlmModelContract};

const MAX_BACKEND_NAME_BYTES: usize = 64;
const MAX_MODEL_NAME_BYTES: usize = 256;
const MAX_REVISION_BYTES: usize = 256;
const MAX_VECTOR_DIMENSIONS: u32 = 1_536;
const MAX_MODEL_TOKENS: u32 = 4_000_000;

/// Operator-owned AI backends, model aliases, profiles, and bounded client limits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    /// Alias selected at instance creation when the tenant omits `embedding_model`.
    pub default_embedding_model: Option<String>,
    /// Generation alias selected when AI Search chat omits a model.
    pub default_generation_model: Option<String>,
    /// Optional VLM alias used to describe admitted images.
    pub default_vlm_model: Option<String>,
    /// Maximum provider requests active across the process.
    pub max_provider_in_flight: u16,
    /// Maximum strings in one embeddings request.
    pub max_embedding_inputs_per_batch: u16,
    /// Maximum serialized embeddings request bytes.
    pub max_embedding_request_bytes: u64,
    /// Maximum serialized embeddings response bytes.
    pub max_embedding_response_bytes: u64,
    /// Maximum concurrent VLM requests across the process.
    pub max_vlm_in_flight: u16,
    /// Maximum images described for one document.
    pub max_vlm_images_per_document: u16,
    /// Maximum complete serialized VLM request bytes.
    pub max_vlm_request_bytes: u64,
    /// Maximum serialized VLM response bytes.
    pub max_vlm_response_bytes: u64,
    /// Provider request deadline in milliseconds.
    pub provider_timeout_ms: u64,
    /// End-to-end query deadline in milliseconds.
    pub query_timeout_ms: u64,
    /// Named operation-specific OpenAI-compatible backends.
    pub backends: BTreeMap<String, AiBackendConfig>,
    /// Named embedding behavior and tokenizer profiles.
    pub embedding_profiles: BTreeMap<String, AiEmbeddingProfileConfig>,
    /// Cloudflare public embedding alias to operator model mapping.
    pub embedding_models: BTreeMap<String, AiEmbeddingModelConfig>,
    /// Cloudflare public generation/rewrite/rerank alias to operator mapping.
    pub generation_models: BTreeMap<String, AiGenerationModelConfig>,
    /// Operator mappings for image-description models.
    pub vlm_models: BTreeMap<String, AiVlmModelConfig>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            default_embedding_model: None,
            default_generation_model: None,
            default_vlm_model: None,
            max_provider_in_flight: 16,
            max_embedding_inputs_per_batch: 96,
            max_embedding_request_bytes: 2 * 1024 * 1024,
            max_embedding_response_bytes: 16 * 1024 * 1024,
            max_vlm_in_flight: 2,
            max_vlm_images_per_document: 16,
            max_vlm_request_bytes: 8 * 1024 * 1024,
            max_vlm_response_bytes: 1024 * 1024,
            provider_timeout_ms: 30_000,
            query_timeout_ms: 15_000,
            backends: BTreeMap::new(),
            embedding_profiles: BTreeMap::new(),
            embedding_models: BTreeMap::new(),
            generation_models: BTreeMap::new(),
            vlm_models: BTreeMap::new(),
        }
    }
}

impl AiConfig {
    pub(super) fn resolve_paths(&mut self, base: &Path) -> Result<(), PlatformError> {
        for backend in self.backends.values_mut() {
            match &mut backend.auth {
                AiAuthConfig::Bearer { secret } | AiAuthConfig::Header { secret, .. } => {
                    super::resolve_secret_path(base, secret)?;
                }
                AiAuthConfig::None => {}
            }
        }
        for profile in self.embedding_profiles.values_mut() {
            profile.tokenizer.artifact.path =
                super::resolve_host_path(base, &profile.tokenizer.artifact.path)?;
        }
        Ok(())
    }

    /// Validate the complete declared catalog without resolving secrets or using the network.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.max_provider_in_flight == 0
            || self.max_provider_in_flight > 256
            || self.max_embedding_inputs_per_batch == 0
            || self.max_embedding_inputs_per_batch > 512
            || self.max_embedding_request_bytes == 0
            || self.max_embedding_request_bytes > 16 * 1024 * 1024
            || self.max_embedding_response_bytes == 0
            || self.max_embedding_response_bytes > 256 * 1024 * 1024
            || self.max_vlm_in_flight == 0
            || self.max_vlm_in_flight > 16
            || self.max_vlm_in_flight > self.max_provider_in_flight
            || self.max_vlm_images_per_document == 0
            || self.max_vlm_images_per_document > 16
            || self.max_vlm_request_bytes < 4 * 1024
            || self.max_vlm_request_bytes > 32 * 1024 * 1024
            || self.max_vlm_response_bytes == 0
            || self.max_vlm_response_bytes > 4 * 1024 * 1024
            || self.provider_timeout_ms == 0
            || self.provider_timeout_ms > 5 * 60 * 1_000
            || self.query_timeout_ms == 0
            || self.query_timeout_ms > 5 * 60 * 1_000
        {
            return Err(invalid_limit());
        }
        if self.query_timeout_ms > self.provider_timeout_ms {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "ai.query_timeout_ms must not exceed ai.provider_timeout_ms",
            ));
        }
        for (name, backend) in &self.backends {
            validate_name(name, MAX_BACKEND_NAME_BYTES, "AI backend name is invalid")?;
            backend.validate()?;
        }
        for (name, profile) in &self.embedding_profiles {
            validate_model_alias(name)?;
            profile.validate()?;
        }
        for (alias, model) in &self.embedding_models {
            validate_model_alias(alias)?;
            model.validate(&self.backends, &self.embedding_profiles)?;
        }
        for (alias, model) in &self.generation_models {
            validate_model_alias(alias)?;
            model.validate(&self.backends)?;
        }
        for (alias, model) in &self.vlm_models {
            validate_model_alias(alias)?;
            model.validate(&self.backends)?;
        }
        if let Some(default) = &self.default_embedding_model
            && !self.embedding_models.contains_key(default)
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "ai.default_embedding_model does not name a configured embedding model",
            ));
        }
        if let Some(default) = &self.default_generation_model
            && !self.generation_models.contains_key(default)
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "ai.default_generation_model does not name a configured generation model",
            ));
        }
        if let Some(default) = &self.default_vlm_model
            && !self.vlm_models.contains_key(default)
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "ai.default_vlm_model does not name a configured VLM model",
            ));
        }
        Ok(())
    }

    /// Resolve the optional default VLM into a secret-free immutable contract.
    pub fn resolve_default_vlm_model(
        &self,
    ) -> Result<Option<ResolvedVlmModelContract>, PlatformError> {
        self.validate()?;
        let Some(alias) = self.default_vlm_model.as_deref() else {
            return Ok(None);
        };
        let model = self.vlm_models.get(alias).ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "VLM model alias is not configured",
            )
        })?;
        let backend = self.backends.get(&model.backend).ok_or_else(|| {
            PlatformError::new(ErrorCode::ConfigInvalid, "VLM backend is not configured")
        })?;
        let endpoint = canonical_endpoint(&backend.endpoint)?;
        let headers_sha256 = headers_digest(&backend.headers)?;
        let auth_header_name = backend.auth.header_name();
        let mut contract = ResolvedVlmModelContract {
            alias: alias.to_owned(),
            backend_name: model.backend.clone(),
            protocol: backend.protocol.as_str().to_owned(),
            endpoint_sha256: hex::encode(Sha256::digest(endpoint.as_str().as_bytes())),
            auth_kind: backend.auth.kind_token().to_owned(),
            auth_header_name,
            headers_sha256,
            remote_model: model.remote_model.clone(),
            provider_revision: model.provider_revision.clone(),
            max_input_width: model.max_input_width,
            max_input_height: model.max_input_height,
            max_input_pixels: model.max_input_pixels,
            max_encoded_image_bytes: model.max_encoded_image_bytes,
            max_output_tokens: model.max_output_tokens,
            max_images_per_document: self.max_vlm_images_per_document,
            max_request_bytes: self.max_vlm_request_bytes,
            max_response_bytes: self.max_vlm_response_bytes,
            prompt_revision: "image-description-v1".to_owned(),
            image_preprocessing_revision: "lanczos3-jpeg-q90-white-v1".to_owned(),
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = digest_canonical(&contract)?;
        Ok(Some(contract))
    }

    /// Resolve one tenant-visible embedding alias into a secret-free immutable contract.
    pub fn resolve_embedding_model(
        &self,
        alias: Option<&str>,
    ) -> Result<ResolvedEmbeddingModelContract, PlatformError> {
        self.validate()?;
        let alias = self.embedding_alias(alias)?;
        let model = self
            .embedding_models
            .get(alias)
            .ok_or_else(missing_embedding_model)?;
        let backend = self
            .backends
            .get(&model.backend)
            .filter(|backend| backend.protocol == AiBackendProtocol::OpenAiEmbeddingsV1)
            .ok_or_else(missing_embedding_backend)?;
        let profile = self
            .embedding_profiles
            .get(&model.profile)
            .ok_or_else(missing_embedding_profile)?;
        let endpoint = canonical_endpoint(&backend.endpoint)?;
        let headers_sha256 = headers_digest(&backend.headers)?;
        let auth_kind = backend.auth.kind_token();
        let auth_header_name = backend.auth.header_name();
        let backend_contract_sha256 = digest_canonical(&BackendContractDigest {
            protocol: backend.protocol.as_str(),
            endpoint: endpoint.as_str(),
            auth_kind,
            auth_header_name: auth_header_name.as_deref(),
            headers_sha256: &headers_sha256,
        })?;
        let profile_contract_sha256 = profile.contract_sha256()?;
        let mut contract = ResolvedEmbeddingModelContract {
            embedding_alias: alias.to_owned(),
            backend_name: model.backend.clone(),
            backend_contract_sha256,
            protocol: backend.protocol.as_str().to_owned(),
            endpoint_sha256: hex::encode(Sha256::digest(endpoint.as_str().as_bytes())),
            auth_kind: auth_kind.to_owned(),
            auth_header_name,
            headers_sha256,
            remote_model: model.remote_model.clone(),
            provider_revision: model.provider_revision.clone(),
            profile: model.profile.clone(),
            profile_contract_sha256,
            dimensions: profile.dimensions,
            send_dimensions: profile.send_dimensions,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: profile.max_input_tokens,
            tokenizer: profile.tokenizer.kind,
            tokenizer_revision: profile.tokenizer.revision.clone(),
            tokenizer_artifact_sha256: profile.tokenizer.artifact.sha256.clone(),
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = digest_canonical(&contract)?;
        Ok(contract)
    }

    /// Resolve one embedding alias into the tokenizer-only contract used by
    /// keyword-only AI Search instances.
    pub fn resolve_tokenizer(
        &self,
        alias: Option<&str>,
    ) -> Result<ResolvedTokenizerContract, PlatformError> {
        self.validate()?;
        let alias = self.embedding_alias(alias)?;
        let model = self
            .embedding_models
            .get(alias)
            .ok_or_else(missing_embedding_model)?;
        let profile = self
            .embedding_profiles
            .get(&model.profile)
            .ok_or_else(missing_embedding_profile)?;
        let mut contract = ResolvedTokenizerContract {
            embedding_alias: alias.to_owned(),
            profile: model.profile.clone(),
            profile_contract_sha256: profile.contract_sha256()?,
            tokenizer: profile.tokenizer.kind,
            tokenizer_revision: profile.tokenizer.revision.clone(),
            tokenizer_artifact_sha256: profile.tokenizer.artifact.sha256.clone(),
            max_input_tokens: profile.max_input_tokens,
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = digest_canonical(&contract)?;
        Ok(contract)
    }

    fn embedding_alias<'a>(&'a self, alias: Option<&'a str>) -> Result<&'a str, PlatformError> {
        match alias {
            Some(alias) => Ok(alias),
            None => self.default_embedding_model.as_deref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "AI Search requires an explicit or operator-default embedding model",
                )
            }),
        }
    }
}

/// Frozen operator mapping from a public embedding alias to a backend model and profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiEmbeddingModelConfig {
    /// Operation-specific backend entry name.
    pub backend: String,
    /// Model value sent to the provider.
    pub remote_model: String,
    /// Optional immutable revision identifier actually supported by the provider.
    #[serde(default)]
    pub provider_revision: Option<String>,
    /// Embedding profile entry name.
    pub profile: String,
}

impl AiEmbeddingModelConfig {
    fn validate(
        &self,
        backends: &BTreeMap<String, AiBackendConfig>,
        profiles: &BTreeMap<String, AiEmbeddingProfileConfig>,
    ) -> Result<(), PlatformError> {
        if !backends
            .get(&self.backend)
            .is_some_and(|backend| backend.protocol == AiBackendProtocol::OpenAiEmbeddingsV1)
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI embedding model references an incompatible backend",
            ));
        }
        if !profiles.contains_key(&self.profile) {
            return Err(missing_embedding_profile());
        }
        validate_nonempty(&self.remote_model, MAX_MODEL_NAME_BYTES)?;
        validate_optional_nonempty(self.provider_revision.as_deref(), MAX_REVISION_BYTES)
    }
}

/// Exact vector and tokenizer behavior shared by one or more embedding model mappings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiEmbeddingProfileConfig {
    /// Exact returned vector dimensions.
    pub dimensions: u32,
    /// Maximum public input tokens accepted by open-compute.
    pub max_input_tokens: u32,
    /// Send the profile dimensions as the `OpenAI` `dimensions` request member.
    #[serde(default)]
    pub send_dimensions: bool,
    /// Exact offline tokenizer contract.
    pub tokenizer: AiTokenizerConfig,
}

impl AiEmbeddingProfileConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        if self.dimensions == 0
            || self.dimensions > MAX_VECTOR_DIMENSIONS
            || self.max_input_tokens == 0
            || self.max_input_tokens > MAX_MODEL_TOKENS
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI embedding profile limits are invalid",
            ));
        }
        self.tokenizer.validate()
    }

    fn contract_sha256(&self) -> Result<String, PlatformError> {
        digest_canonical(&ProfileContractDigest {
            dimensions: self.dimensions,
            max_input_tokens: self.max_input_tokens,
            send_dimensions: self.send_dimensions,
            tokenizer: self.tokenizer.kind,
            tokenizer_revision: &self.tokenizer.revision,
            tokenizer_artifact_sha256: &self.tokenizer.artifact.sha256,
        })
    }
}

/// Frozen similarity metric used by AI Search embedding indexes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiEmbeddingMetric {
    /// Cosine similarity.
    Cosine,
}

/// Tokenizer families with an explicit operator-pinned revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiTokenizer {
    /// BGE-M3 tokenizer.
    BgeM3,
    /// `OpenAI` `cl100k` tokenizer.
    Cl100kBase,
    /// Qwen 3 tokenizer.
    Qwen3,
    /// `EmbeddingGemma` tokenizer.
    EmbeddingGemma,
    /// Gemini embedding tokenizer.
    Gemini,
    /// Operator-defined Hugging Face tokenizer JSON without a product family claim.
    Custom,
}

/// Exact tokenizer identity and offline artifact used by an embedding profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiTokenizerConfig {
    /// Known tokenizer family or `custom` for an operator-defined tokenizer.
    pub kind: AiTokenizer,
    /// Operator-pinned tokenizer revision identity.
    pub revision: String,
    /// Exact offline Hugging Face tokenizer artifact.
    pub artifact: AiTokenizerArtifactConfig,
}

impl AiTokenizerConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        validate_nonempty(&self.revision, MAX_REVISION_BYTES)?;
        self.artifact.validate()
    }
}

/// Operator-pinned offline Hugging Face `tokenizer.json` artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiTokenizerArtifactConfig {
    /// Absolute path opened without following a final symlink during service load.
    pub path: PathBuf,
    /// Lowercase SHA-256 of the exact artifact bytes.
    pub sha256: String,
}

impl AiTokenizerArtifactConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        if !self.path.is_absolute()
            || self
                .path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI tokenizer artifact path or SHA-256 is invalid",
            ));
        }
        Ok(())
    }
}

/// Frozen operator mapping for generation, rewrite, or rerank calls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiGenerationModelConfig {
    /// Operation-specific backend entry name.
    pub backend: String,
    /// Model value sent to the provider.
    pub remote_model: String,
    /// Optional immutable revision identifier actually supported by the provider.
    #[serde(default)]
    pub provider_revision: Option<String>,
    /// Maximum context tokens accepted by the adapter.
    pub max_context_tokens: u32,
    /// Explicit capabilities admitted for this alias.
    pub capabilities: BTreeSet<AiGenerationCapability>,
}

impl AiGenerationModelConfig {
    fn validate(&self, backends: &BTreeMap<String, AiBackendConfig>) -> Result<(), PlatformError> {
        if !backends
            .get(&self.backend)
            .is_some_and(|backend| backend.protocol == AiBackendProtocol::OpenAiChatCompletionsV1)
            || self.max_context_tokens == 0
            || self.max_context_tokens > MAX_MODEL_TOKENS
            || self.capabilities.is_empty()
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI generation model contract is invalid",
            ));
        }
        validate_nonempty(&self.remote_model, MAX_MODEL_NAME_BYTES)?;
        validate_optional_nonempty(self.provider_revision.as_deref(), MAX_REVISION_BYTES)
    }
}

/// Explicit backend operations implemented for a generation model alias.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiGenerationCapability {
    /// Non-stream and SSE chat completions.
    Chat,
    /// Query rewrite through the chat-completions adapter.
    Rewrite,
    /// Candidate reranking through a separately validated adapter contract.
    Rerank,
}

/// Secret-free immutable embedding contract stored with an AI Search instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedEmbeddingModelContract {
    /// Tenant-visible Cloudflare model alias.
    pub embedding_alias: String,
    /// Operator backend entry name.
    pub backend_name: String,
    /// Digest of backend protocol, endpoint, auth shape, and static headers.
    pub backend_contract_sha256: String,
    /// Fixed backend protocol token.
    pub protocol: String,
    /// Digest of the canonical final endpoint URL.
    pub endpoint_sha256: String,
    /// Secret-free authentication kind.
    pub auth_kind: String,
    /// Normalized custom authentication header name, when used.
    pub auth_header_name: Option<String>,
    /// Digest of normalized static header names and values.
    pub headers_sha256: String,
    /// Provider model value.
    pub remote_model: String,
    /// Optional provider-supported immutable model revision.
    pub provider_revision: Option<String>,
    /// Operator embedding profile name.
    pub profile: String,
    /// Digest of the expanded embedding profile.
    pub profile_contract_sha256: String,
    /// Exact vector dimensions.
    pub dimensions: u32,
    /// Whether the dimensions request member is sent.
    pub send_dimensions: bool,
    /// Frozen similarity metric.
    pub metric: AiEmbeddingMetric,
    /// Maximum input tokens.
    pub max_input_tokens: u32,
    /// Frozen tokenizer family.
    pub tokenizer: AiTokenizer,
    /// Operator-pinned tokenizer revision.
    pub tokenizer_revision: String,
    /// SHA-256 of the exact offline tokenizer artifact; its local path is never persisted.
    pub tokenizer_artifact_sha256: String,
    /// Digest of this complete contract with this field empty.
    pub contract_sha256: String,
}

/// Frozen tokenizer-only contract used when an AI Search instance has no vector index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedTokenizerContract {
    /// Operator model alias that selects the tokenizer profile.
    pub embedding_alias: String,
    /// Operator embedding profile name.
    pub profile: String,
    /// Digest of the expanded embedding profile.
    pub profile_contract_sha256: String,
    /// Frozen tokenizer family.
    pub tokenizer: AiTokenizer,
    /// Operator-pinned tokenizer revision.
    pub tokenizer_revision: String,
    /// SHA-256 of the exact offline tokenizer artifact.
    pub tokenizer_artifact_sha256: String,
    /// Maximum input tokens inherited from the profile.
    pub max_input_tokens: u32,
    /// Digest of this complete contract with this field empty.
    pub contract_sha256: String,
}

#[derive(Serialize)]
struct BackendContractDigest<'a> {
    protocol: &'a str,
    endpoint: &'a str,
    auth_kind: &'a str,
    auth_header_name: Option<&'a str>,
    headers_sha256: &'a str,
}

#[derive(Serialize)]
struct ProfileContractDigest<'a> {
    dimensions: u32,
    max_input_tokens: u32,
    send_dimensions: bool,
    tokenizer: AiTokenizer,
    tokenizer_revision: &'a str,
    tokenizer_artifact_sha256: &'a str,
}

fn validate_model_alias(alias: &str) -> Result<(), PlatformError> {
    if alias.len() > MAX_MODEL_NAME_BYTES
        || !alias
            .starts_with(|character: char| character == '@' || character.is_ascii_alphanumeric())
        || alias.chars().any(char::is_control)
        || alias.contains(char::is_whitespace)
        || !alias.contains('/')
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "AI model or profile alias is invalid",
        ));
    }
    Ok(())
}

fn validate_name(value: &str, max: usize, message: &'static str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > max
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        || !value.starts_with(|character: char| character.is_ascii_alphabetic())
    {
        return Err(PlatformError::new(ErrorCode::ConfigInvalid, message));
    }
    Ok(())
}

fn validate_optional_nonempty(value: Option<&str>, max: usize) -> Result<(), PlatformError> {
    match value {
        Some(value) => validate_nonempty(value, max),
        None => Ok(()),
    }
}

fn validate_nonempty(value: &str, max: usize) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > max
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "AI model catalog text is invalid",
        ));
    }
    Ok(())
}

fn digest_canonical<T: Serialize>(value: &T) -> Result<String, PlatformError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "AI model contract is not serializable",
        )
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn missing_embedding_model() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "embedding model alias is not in the operator catalog",
    )
}

fn missing_embedding_backend() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "embedding backend is missing or incompatible",
    )
}

fn missing_embedding_profile() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "embedding profile is not in the operator catalog",
    )
}

fn invalid_limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::LimitInvalid,
        "AI provider limits are outside the hard platform bounds",
    )
}

#[cfg(test)]
mod tests;
