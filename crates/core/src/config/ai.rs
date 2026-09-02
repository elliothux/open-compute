//! Operator-owned AI provider and immutable model-catalog configuration.

use super::SecretReference;
use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::path::{Component, PathBuf};
use url::Url;

const PROVIDER_ADAPTER: &str = "openai_v1";
const MAX_PROVIDER_NAME_BYTES: usize = 64;
const MAX_MODEL_NAME_BYTES: usize = 256;
const MAX_REVISION_BYTES: usize = 256;

/// Operator-owned AI providers, model aliases, and bounded client limits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    /// Alias selected at instance creation when the tenant omits `embedding_model`.
    pub default_embedding_model: Option<String>,
    /// Generation alias selected when AI Search chat omits a model.
    pub default_generation_model: Option<String>,
    /// Maximum provider requests active across the process.
    pub max_provider_in_flight: u16,
    /// Maximum strings in one embeddings request.
    pub max_embedding_inputs_per_batch: u16,
    /// Maximum serialized embeddings request bytes.
    pub max_embedding_request_bytes: u64,
    /// Maximum serialized embeddings response bytes.
    pub max_embedding_response_bytes: u64,
    /// Provider request deadline in milliseconds.
    pub provider_timeout_ms: u64,
    /// End-to-end query deadline in milliseconds.
    pub query_timeout_ms: u64,
    /// Named OpenAI-compatible provider endpoints.
    pub providers: BTreeMap<String, AiProviderConfig>,
    /// Cloudflare public embedding alias to frozen operator model mapping.
    pub embedding_models: BTreeMap<String, AiEmbeddingModelConfig>,
    /// Cloudflare public generation/rewrite/rerank alias to operator mapping.
    pub generation_models: BTreeMap<String, AiGenerationModelConfig>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            default_embedding_model: None,
            default_generation_model: None,
            max_provider_in_flight: 16,
            max_embedding_inputs_per_batch: 96,
            max_embedding_request_bytes: 2 * 1024 * 1024,
            max_embedding_response_bytes: 16 * 1024 * 1024,
            provider_timeout_ms: 30_000,
            query_timeout_ms: 15_000,
            providers: BTreeMap::new(),
            embedding_models: BTreeMap::new(),
            generation_models: BTreeMap::new(),
        }
    }
}

impl AiConfig {
    /// Validate the complete declared catalog without resolving secret values or using the network.
    pub fn validate(&self) -> Result<(), PlatformError> {
        if self.max_provider_in_flight == 0
            || self.max_provider_in_flight > 256
            || self.max_embedding_inputs_per_batch == 0
            || self.max_embedding_inputs_per_batch > 512
            || self.max_embedding_request_bytes == 0
            || self.max_embedding_request_bytes > 16 * 1024 * 1024
            || self.max_embedding_response_bytes == 0
            || self.max_embedding_response_bytes > 256 * 1024 * 1024
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
        for (name, provider) in &self.providers {
            validate_name(name, MAX_PROVIDER_NAME_BYTES, "AI provider name is invalid")?;
            provider.validate()?;
        }
        for (alias, model) in &self.embedding_models {
            validate_model_alias(alias)?;
            model.validate(alias, &self.providers)?;
        }
        for (alias, model) in &self.generation_models {
            validate_model_alias(alias)?;
            model.validate(&self.providers)?;
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
        Ok(())
    }

    /// Resolve one tenant-visible embedding alias into a secret-free immutable contract.
    pub fn resolve_embedding_model(
        &self,
        alias: Option<&str>,
    ) -> Result<ResolvedEmbeddingModelContract, PlatformError> {
        self.validate()?;
        let alias = match alias {
            Some(alias) => alias,
            None => self.default_embedding_model.as_deref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "AI Search requires an explicit or operator-default embedding model",
                )
            })?,
        };
        let model = self.embedding_models.get(alias).ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "embedding model alias is not in the operator catalog",
            )
        })?;
        let provider = self.providers.get(&model.provider).ok_or_else(|| {
            PlatformError::new(ErrorCode::ConfigInvalid, "embedding provider is missing")
        })?;
        let base = canonical_base_url(&provider.base_url)?;
        let mut endpoint = base.clone();
        endpoint.set_path("/v1/embeddings");
        let auth_kind = provider.auth.kind_token();
        let provider_contract_sha256 = digest_canonical(&ProviderContractDigest {
            adapter: PROVIDER_ADAPTER,
            base_url: base.as_str(),
            auth_kind,
        })?;
        let endpoint_sha256 = hex::encode(Sha256::digest(endpoint.as_str().as_bytes()));
        let mut contract = ResolvedEmbeddingModelContract {
            embedding_alias: alias.to_owned(),
            provider_name: model.provider.clone(),
            provider_contract_sha256,
            protocol: "openai_v1_embeddings".to_owned(),
            endpoint_sha256,
            auth_kind: auth_kind.to_owned(),
            remote_model: model.remote_model.clone(),
            model_revision: model.model_revision.clone(),
            dimensions: model.dimensions,
            request_dimensions: model.request_dimensions,
            metric: model.metric,
            max_input_tokens: model.max_input_tokens,
            tokenizer: model.tokenizer,
            tokenizer_revision: model.tokenizer_revision.clone(),
            tokenizer_artifact_sha256: model.tokenizer_artifact.sha256.clone(),
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
        let alias = match alias {
            Some(alias) => alias,
            None => self.default_embedding_model.as_deref().ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "AI Search requires an operator-default tokenizer model",
                )
            })?,
        };
        let model = self.embedding_models.get(alias).ok_or_else(|| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "tokenizer model alias is not in the operator catalog",
            )
        })?;
        let mut contract = ResolvedTokenizerContract {
            embedding_alias: alias.to_owned(),
            tokenizer: model.tokenizer,
            tokenizer_revision: model.tokenizer_revision.clone(),
            tokenizer_artifact_sha256: model.tokenizer_artifact.sha256.clone(),
            max_input_tokens: model.max_input_tokens,
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = digest_canonical(&contract)?;
        Ok(contract)
    }
}

/// One fixed OpenAI-compatible provider endpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiProviderConfig {
    /// Canonical `/v1` root; the platform appends fixed route names.
    pub base_url: String,
    /// Explicit authentication policy.
    pub auth: AiAuthConfig,
}

impl AiProviderConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        let url = canonical_base_url(&self.base_url)?;
        match &self.auth {
            AiAuthConfig::Bearer { secret } => secret.validate("ai.providers.*.auth.secret")?,
            AiAuthConfig::None => {
                if url.scheme() != "http" || !url_host_is_loopback(&url) {
                    return Err(PlatformError::new(
                        ErrorCode::ConfigInvalid,
                        "AI provider auth kind none is allowed only for loopback HTTP",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Explicit provider authentication policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AiAuthConfig {
    /// Send one resolved secret as an HTTP Bearer credential.
    Bearer {
        /// Symbolic secret reference; its value never enters config serialization or model contracts.
        secret: SecretReference,
    },
    /// Send no credential; accepted only for explicit loopback HTTP.
    None,
}

impl AiAuthConfig {
    /// Stable secret-free token included in the provider contract.
    #[must_use]
    pub fn kind_token(&self) -> &'static str {
        match self {
            Self::Bearer { .. } => "bearer",
            Self::None => "none",
        }
    }
}

/// Frozen operator mapping for one current Cloudflare embedding alias.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiEmbeddingModelConfig {
    /// Provider entry name.
    pub provider: String,
    /// Model value sent to the provider.
    pub remote_model: String,
    /// Operator-pinned model revision identity.
    pub model_revision: String,
    /// Exact returned vector dimensions.
    pub dimensions: u32,
    /// Optional dimensions parameter sent to providers that support it.
    pub request_dimensions: Option<u32>,
    /// Frozen similarity metric.
    pub metric: AiEmbeddingMetric,
    /// Maximum public input tokens.
    pub max_input_tokens: u32,
    /// Known tokenizer family used by chunking.
    pub tokenizer: AiTokenizer,
    /// Operator-pinned tokenizer revision identity.
    pub tokenizer_revision: String,
    /// Exact offline Hugging Face tokenizer artifact.
    pub tokenizer_artifact: AiTokenizerArtifactConfig,
}

impl AiEmbeddingModelConfig {
    fn validate(
        &self,
        alias: &str,
        providers: &BTreeMap<String, AiProviderConfig>,
    ) -> Result<(), PlatformError> {
        if !providers.contains_key(&self.provider) {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI embedding model references an unknown provider",
            ));
        }
        validate_nonempty(&self.remote_model, MAX_MODEL_NAME_BYTES)?;
        validate_nonempty(&self.model_revision, MAX_REVISION_BYTES)?;
        validate_nonempty(&self.tokenizer_revision, MAX_REVISION_BYTES)?;
        self.tokenizer_artifact.validate()?;
        let (dimensions, max_input_tokens) =
            cloudflare_embedding_contract(alias).ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "AI embedding alias is outside the pinned Cloudflare model catalog",
                )
            })?;
        if self.dimensions != dimensions
            || self.max_input_tokens != max_input_tokens
            || self.metric != AiEmbeddingMetric::Cosine
            || self
                .request_dimensions
                .is_some_and(|value| value != self.dimensions)
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI embedding model conflicts with the pinned Cloudflare contract",
            ));
        }
        Ok(())
    }
}

/// Similarity metric admitted for the pinned AI Search embedding catalog.
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
    /// Provider entry name.
    pub provider: String,
    /// Model value sent to the provider.
    pub remote_model: String,
    /// Operator-pinned model revision identity.
    pub model_revision: String,
    /// Maximum context tokens accepted by the adapter.
    pub max_context_tokens: u32,
    /// Explicit capabilities admitted for this alias.
    pub capabilities: BTreeSet<AiGenerationCapability>,
}

impl AiGenerationModelConfig {
    fn validate(
        &self,
        providers: &BTreeMap<String, AiProviderConfig>,
    ) -> Result<(), PlatformError> {
        if !providers.contains_key(&self.provider)
            || self.max_context_tokens == 0
            || self.max_context_tokens > 4_000_000
            || self.capabilities.is_empty()
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "AI generation model contract is invalid",
            ));
        }
        validate_nonempty(&self.remote_model, MAX_MODEL_NAME_BYTES)?;
        validate_nonempty(&self.model_revision, MAX_REVISION_BYTES)
    }
}

/// Explicit provider operations implemented for a generation model alias.
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
    /// Operator provider entry name.
    pub provider_name: String,
    /// Digest of adapter, canonical base URL, and auth kind.
    pub provider_contract_sha256: String,
    /// Fixed provider protocol token.
    pub protocol: String,
    /// Digest of the canonical embeddings endpoint URL.
    pub endpoint_sha256: String,
    /// Secret-free authentication kind.
    pub auth_kind: String,
    /// Provider model value.
    pub remote_model: String,
    /// Operator-pinned model revision.
    pub model_revision: String,
    /// Exact vector dimensions.
    pub dimensions: u32,
    /// Optional requested output dimensions.
    pub request_dimensions: Option<u32>,
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
    /// Operator model alias that owns the tokenizer artifact.
    pub embedding_alias: String,
    /// Frozen tokenizer family.
    pub tokenizer: AiTokenizer,
    /// Operator-pinned tokenizer revision.
    pub tokenizer_revision: String,
    /// SHA-256 of the exact offline tokenizer artifact.
    pub tokenizer_artifact_sha256: String,
    /// Maximum input tokens inherited from the pinned model catalog.
    pub max_input_tokens: u32,
    /// Digest of this complete contract with this field empty.
    pub contract_sha256: String,
}

#[derive(Serialize)]
struct ProviderContractDigest<'a> {
    adapter: &'static str,
    base_url: &'a str,
    auth_kind: &'a str,
}

fn canonical_base_url(value: &str) -> Result<Url, PlatformError> {
    let url = Url::parse(value).map_err(|_| invalid_provider_url())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/v1"
        || value != url.as_str()
        || (url.scheme() == "http" && !url_host_is_loopback(&url))
    {
        return Err(invalid_provider_url());
    }
    Ok(url)
}

fn url_host_is_loopback(url: &Url) -> bool {
    url.host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
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
            "AI model alias is invalid",
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

fn cloudflare_embedding_contract(alias: &str) -> Option<(u32, u32)> {
    match alias {
        "google-ai-studio/gemini-embedding-001" => Some((1536, 2048)),
        "openai/text-embedding-3-small" | "openai/text-embedding-3-large" => Some((1536, 8192)),
        "@cf/baai/bge-m3" | "@cf/baai/bge-large-en-v1.5" => Some((1024, 512)),
        "@cf/qwen/qwen3-embedding-0.6b" => Some((1024, 8192)),
        "@cf/google/embeddinggemma-300m" => Some((768, 512)),
        _ => None,
    }
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

fn invalid_provider_url() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "AI provider base_url must be a canonical HTTPS or loopback HTTP /v1/ root",
    )
}

fn invalid_limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::LimitInvalid,
        "AI provider limits are outside the hard platform bounds",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> AiConfig {
        let mut config = AiConfig::default();
        config.providers.insert(
            "fixture".to_owned(),
            AiProviderConfig {
                base_url: "http://127.0.0.1:8080/v1".to_owned(),
                auth: AiAuthConfig::Bearer {
                    secret: SecretReference {
                        env: Some("AI_FIXTURE_KEY".to_owned()),
                        file: None,
                    },
                },
            },
        );
        config.embedding_models.insert(
            "@cf/qwen/qwen3-embedding-0.6b".to_owned(),
            AiEmbeddingModelConfig {
                provider: "fixture".to_owned(),
                remote_model: "@cf/qwen/qwen3-embedding-0.6b".to_owned(),
                model_revision: "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3".to_owned(),
                dimensions: 1024,
                request_dimensions: None,
                metric: AiEmbeddingMetric::Cosine,
                max_input_tokens: 8192,
                tokenizer: AiTokenizer::Qwen3,
                tokenizer_revision: "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3".to_owned(),
                tokenizer_artifact: AiTokenizerArtifactConfig {
                    path: PathBuf::from("/opt/open-compute/models/qwen3/tokenizer.json"),
                    sha256: "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"
                        .to_owned(),
                },
            },
        );
        config.default_embedding_model = Some("@cf/qwen/qwen3-embedding-0.6b".to_owned());
        config
    }

    #[test]
    fn fixture_catalog_resolves_to_stable_secret_free_contract() {
        let config = configured();
        config.validate().unwrap();
        let first = config.resolve_embedding_model(None).unwrap();
        let second = config
            .resolve_embedding_model(Some("@cf/qwen/qwen3-embedding-0.6b"))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.dimensions, 1024);
        assert_eq!(first.auth_kind, "bearer");
        let tokenizer = config.resolve_tokenizer(None).unwrap();
        assert_eq!(tokenizer.tokenizer, AiTokenizer::Qwen3);
        assert_eq!(tokenizer.max_input_tokens, 8192);
        assert_ne!(tokenizer.contract_sha256, first.contract_sha256);
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("AI_FIXTURE_KEY"));
        assert!(!json.contains("127.0.0.1"));
    }

    #[test]
    fn provider_and_catalog_drift_fail_closed() {
        let mut config = configured();
        config.providers.get_mut("fixture").unwrap().base_url = "http://example.com/v1".to_owned();
        assert!(config.validate().is_err());

        let mut config = configured();
        config
            .embedding_models
            .get_mut("@cf/qwen/qwen3-embedding-0.6b")
            .unwrap()
            .dimensions = 768;
        assert!(config.validate().is_err());

        let mut config = configured();
        config.default_embedding_model = Some("missing/model".to_owned());
        assert!(config.validate().is_err());

        let mut config = configured();
        config
            .embedding_models
            .get_mut("@cf/qwen/qwen3-embedding-0.6b")
            .unwrap()
            .tokenizer_artifact
            .path = PathBuf::from("relative/tokenizer.json");
        assert!(config.validate().is_err());

        let mut config = configured();
        config
            .embedding_models
            .get_mut("@cf/qwen/qwen3-embedding-0.6b")
            .unwrap()
            .tokenizer_artifact
            .sha256 = "ABC".repeat(21);
        assert!(config.validate().is_err());
    }

    #[test]
    fn anonymous_auth_is_only_for_loopback_http() {
        let mut config = configured();
        config.providers.get_mut("fixture").unwrap().base_url =
            "https://api.example.com/v1".to_owned();
        config.providers.get_mut("fixture").unwrap().auth = AiAuthConfig::None;
        assert!(config.validate().is_err());
        config.providers.get_mut("fixture").unwrap().auth = AiAuthConfig::Bearer {
            secret: SecretReference {
                env: Some("AI_FIXTURE_KEY".to_owned()),
                file: None,
            },
        };
        config.validate().unwrap();
    }

    #[test]
    fn every_operator_limit_and_timeout_relationship_is_bounded() {
        let invalid = [
            ("max_provider_in_flight", 0_u64),
            ("max_provider_in_flight", 257),
            ("max_embedding_inputs_per_batch", 0),
            ("max_embedding_inputs_per_batch", 513),
            ("max_embedding_request_bytes", 0),
            ("max_embedding_request_bytes", 16 * 1024 * 1024 + 1),
            ("max_embedding_response_bytes", 0),
            ("max_embedding_response_bytes", 256 * 1024 * 1024 + 1),
            ("provider_timeout_ms", 0),
            ("provider_timeout_ms", 300_001),
            ("query_timeout_ms", 0),
            ("query_timeout_ms", 300_001),
        ];
        for (field, value) in invalid {
            let mut config = configured();
            match field {
                "max_provider_in_flight" => config.max_provider_in_flight = value as u16,
                "max_embedding_inputs_per_batch" => {
                    config.max_embedding_inputs_per_batch = value as u16;
                }
                "max_embedding_request_bytes" => config.max_embedding_request_bytes = value,
                "max_embedding_response_bytes" => config.max_embedding_response_bytes = value,
                "provider_timeout_ms" => config.provider_timeout_ms = value,
                "query_timeout_ms" => config.query_timeout_ms = value,
                _ => unreachable!(),
            }
            assert_eq!(
                config.validate().unwrap_err().code(),
                ErrorCode::LimitInvalid,
                "{field}"
            );
        }
        let mut config = configured();
        config.provider_timeout_ms = 99;
        config.query_timeout_ms = 100;
        assert_eq!(
            config.validate().unwrap_err().code(),
            ErrorCode::LimitInvalid
        );
    }

    #[test]
    fn provider_urls_names_aliases_and_model_text_are_canonical() {
        for url in [
            "not-a-url",
            "ftp://example.com/v1",
            "https://user@example.com/v1",
            "https://example.com/v1?x=1",
            "https://example.com/v1#fragment",
            "https://example.com/other",
            "http://[::2]/v1",
        ] {
            let mut config = configured();
            config.providers.get_mut("fixture").unwrap().base_url = url.to_string();
            assert!(config.validate().is_err(), "{url}");
        }
        let mut loopback = configured();
        loopback.providers.get_mut("fixture").unwrap().auth = AiAuthConfig::None;
        loopback.validate().unwrap();
        assert_eq!(AiAuthConfig::None.kind_token(), "none");
        assert_eq!(
            configured().providers["fixture"].auth.kind_token(),
            "bearer"
        );

        for name in ["", "9provider", "bad provider", &"x".repeat(65)] {
            let mut config = configured();
            let provider = config.providers.remove("fixture").unwrap();
            config.providers.insert(name.to_string(), provider);
            assert!(config.validate().is_err(), "{name}");
        }
        for alias in [
            "missing-slash",
            " bad/model",
            "bad /model",
            &"x".repeat(257),
        ] {
            let mut config = configured();
            let model = config
                .embedding_models
                .remove("@cf/qwen/qwen3-embedding-0.6b")
                .unwrap();
            config.default_embedding_model = None;
            config.embedding_models.insert(alias.to_string(), model);
            assert!(config.validate().is_err(), "{alias}");
        }
        for field in ["remote_model", "model_revision", "tokenizer_revision"] {
            let mut config = configured();
            let model = config
                .embedding_models
                .get_mut("@cf/qwen/qwen3-embedding-0.6b")
                .unwrap();
            match field {
                "remote_model" => model.remote_model = " trailing ".to_string(),
                "model_revision" => model.model_revision.clear(),
                "tokenizer_revision" => model.tokenizer_revision = "bad\nrevision".to_string(),
                _ => unreachable!(),
            }
            assert!(config.validate().is_err(), "{field}");
        }
    }

    #[test]
    fn catalog_resolution_and_generation_validation_reject_drift() {
        let empty = AiConfig::default();
        assert!(empty.resolve_embedding_model(None).is_err());
        assert!(empty.resolve_tokenizer(None).is_err());
        let config = configured();
        assert!(
            config
                .resolve_embedding_model(Some("missing/model"))
                .is_err()
        );
        assert!(config.resolve_tokenizer(Some("missing/model")).is_err());

        for mutate in 0..5 {
            let mut config = configured();
            let model = config
                .embedding_models
                .get_mut("@cf/qwen/qwen3-embedding-0.6b")
                .unwrap();
            match mutate {
                0 => model.provider = "missing".to_string(),
                1 => model.max_input_tokens = 512,
                2 => model.request_dimensions = Some(768),
                3 => model.tokenizer_artifact.path = PathBuf::from("/opt/../tokenizer.json"),
                4 => model.tokenizer_artifact.sha256 = "g".repeat(64),
                _ => unreachable!(),
            }
            assert!(config.validate().is_err(), "mutation {mutate}");
        }

        let generation = AiGenerationModelConfig {
            provider: "fixture".to_string(),
            remote_model: "fixture-chat".to_string(),
            model_revision: "revision-1".to_string(),
            max_context_tokens: 8_192,
            capabilities: [
                AiGenerationCapability::Chat,
                AiGenerationCapability::Rewrite,
            ]
            .into_iter()
            .collect(),
        };
        let mut valid = configured();
        valid
            .generation_models
            .insert("fixture/chat".to_string(), generation.clone());
        valid.default_generation_model = Some("fixture/chat".to_string());
        valid.validate().unwrap();

        for mutate in 0..5 {
            let mut config = configured();
            let mut model = generation.clone();
            match mutate {
                0 => model.provider = "missing".to_string(),
                1 => model.max_context_tokens = 0,
                2 => model.max_context_tokens = 4_000_001,
                3 => model.capabilities.clear(),
                4 => model.remote_model = "".to_string(),
                _ => unreachable!(),
            }
            config
                .generation_models
                .insert("fixture/chat".to_string(), model);
            assert!(config.validate().is_err(), "generation mutation {mutate}");
        }
        let mut config = configured();
        config.default_generation_model = Some("missing/chat".to_string());
        assert!(config.validate().is_err());
    }
}
