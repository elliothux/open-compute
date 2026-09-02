//! Offline, digest-pinned tokenizer adapters for frozen AI Search contracts.

use crate::ai_search_coordinator::AiSearchTokenCounter;
use open_compute_core::{
    AiConfig, ErrorCode, PlatformError, ResolvedEmbeddingModelContract, ResolvedTokenizerContract,
};
use rustix::fs::{Mode, OFlags};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read as _;
use std::sync::Arc;
use tokenizers::Tokenizer;

const MAX_TOKENIZER_BYTES: u64 = 64 * 1024 * 1024;

/// Tokenizers loaded once from exact offline artifacts and selected by frozen contract.
#[derive(Clone)]
pub(crate) struct AiTokenizerRegistry {
    by_embedding_contract: BTreeMap<String, Arc<FrozenAiTokenizer>>,
    by_tokenizer_contract: BTreeMap<String, Arc<FrozenAiTokenizer>>,
}

impl std::fmt::Debug for AiTokenizerRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiTokenizerRegistry")
            .field("embedding_contracts", &self.by_embedding_contract.len())
            .field("tokenizer_contracts", &self.by_tokenizer_contract.len())
            .finish()
    }
}

impl AiTokenizerRegistry {
    /// Load and verify every tokenizer referenced by the operator catalog.
    pub(crate) fn load(config: &AiConfig) -> Result<Self, PlatformError> {
        config.validate()?;
        let mut artifacts = HashMap::<String, Arc<Tokenizer>>::new();
        let mut by_embedding_contract = BTreeMap::new();
        let mut by_tokenizer_contract = BTreeMap::new();
        for (alias, model) in &config.embedding_models {
            let embedding_contract = config.resolve_embedding_model(Some(alias))?;
            let tokenizer_contract = config.resolve_tokenizer(Some(alias))?;
            let tokenizer = if let Some(tokenizer) = artifacts.get(&model.tokenizer_artifact.sha256)
            {
                tokenizer.clone()
            } else {
                let bytes = read_artifact(&model.tokenizer_artifact.path)?;
                let digest = hex::encode(Sha256::digest(&bytes));
                if digest != model.tokenizer_artifact.sha256 {
                    return Err(integrity());
                }
                let tokenizer = Tokenizer::from_bytes(&bytes).map_err(|_| invalid())?;
                if tokenizer.get_truncation().is_some()
                    || tokenizer.get_padding().is_some()
                    || tokenizer
                        .encode("open-compute tokenizer probe", true)
                        .map_err(|_| invalid())?
                        .is_empty()
                {
                    return Err(invalid());
                }
                let tokenizer = Arc::new(tokenizer);
                artifacts.insert(model.tokenizer_artifact.sha256.clone(), tokenizer.clone());
                tokenizer
            };
            let frozen = Arc::new(FrozenAiTokenizer {
                embedding_contract: embedding_contract.clone(),
                tokenizer_contract: tokenizer_contract.clone(),
                tokenizer,
            });
            if by_embedding_contract
                .insert(embedding_contract.contract_sha256.clone(), frozen.clone())
                .is_some()
                || by_tokenizer_contract
                    .insert(tokenizer_contract.contract_sha256.clone(), frozen)
                    .is_some()
            {
                return Err(invalid());
            }
        }
        Ok(Self {
            by_embedding_contract,
            by_tokenizer_contract,
        })
    }

    /// Select only an exact current catalog contract; stale or forged contracts fail closed.
    pub(crate) fn for_contract(
        &self,
        contract: &ResolvedEmbeddingModelContract,
    ) -> Result<Arc<dyn AiSearchTokenCounter>, PlatformError> {
        let tokenizer = self
            .by_embedding_contract
            .get(&contract.contract_sha256)
            .filter(|tokenizer| tokenizer.embedding_contract == *contract)
            .cloned()
            .ok_or_else(contract_mismatch)?;
        Ok(tokenizer)
    }

    /// Select an exact tokenizer-only contract for a keyword-only instance.
    pub(crate) fn for_tokenizer_contract(
        &self,
        contract: &ResolvedTokenizerContract,
    ) -> Result<Arc<dyn AiSearchTokenCounter>, PlatformError> {
        let tokenizer = self
            .by_tokenizer_contract
            .get(&contract.contract_sha256)
            .filter(|tokenizer| tokenizer.tokenizer_contract == *contract)
            .cloned()
            .ok_or_else(contract_mismatch)?;
        Ok(tokenizer)
    }
}

#[derive(Clone)]
struct FrozenAiTokenizer {
    embedding_contract: ResolvedEmbeddingModelContract,
    tokenizer_contract: ResolvedTokenizerContract,
    tokenizer: Arc<Tokenizer>,
}

impl std::fmt::Debug for FrozenAiTokenizer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FrozenAiTokenizer")
            .field("family", &self.tokenizer_contract.tokenizer)
            .field("revision", &self.tokenizer_contract.tokenizer_revision)
            .field(
                "artifact_sha256",
                &self.tokenizer_contract.tokenizer_artifact_sha256,
            )
            .finish_non_exhaustive()
    }
}

impl AiSearchTokenCounter for FrozenAiTokenizer {
    fn count(&self, text: &str) -> usize {
        self.tokenizer
            .encode(text, true)
            .map_or(0, |encoding| encoding.len())
    }
}

fn read_artifact(path: &std::path::Path) -> Result<Vec<u8>, PlatformError> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| unavailable())?;
    let file = File::from(fd);
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_TOKENIZER_BYTES
    {
        return Err(invalid());
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| invalid())?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_TOKENIZER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() != capacity {
        return Err(integrity());
    }
    Ok(bytes)
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigPathInvalid,
        "AI tokenizer artifact is unavailable or unsafe",
    )
}

fn integrity() -> PlatformError {
    PlatformError::new(
        ErrorCode::ArtifactIntegrityError,
        "AI tokenizer artifact failed digest verification",
    )
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "AI tokenizer artifact contract is invalid",
    )
}

fn contract_mismatch() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "AI tokenizer does not match the frozen model contract",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::{
        AiAuthConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiProviderConfig, AiTokenizer,
        AiTokenizerArtifactConfig,
    };
    use std::path::PathBuf;

    fn fixture_config(path: PathBuf, sha256: String) -> AiConfig {
        let mut config = AiConfig::default();
        config.providers.insert(
            "fixture".to_owned(),
            AiProviderConfig {
                base_url: "http://127.0.0.1:8080/v1".to_owned(),
                auth: AiAuthConfig::None,
            },
        );
        let alias = "@cf/qwen/qwen3-embedding-0.6b";
        config.embedding_models.insert(
            alias.to_owned(),
            AiEmbeddingModelConfig {
                provider: "fixture".to_owned(),
                remote_model: alias.to_owned(),
                model_revision: "fixture-model".to_owned(),
                dimensions: 1024,
                request_dimensions: None,
                metric: AiEmbeddingMetric::Cosine,
                max_input_tokens: 8192,
                tokenizer: AiTokenizer::Qwen3,
                tokenizer_revision: "fixture-tokenizer".to_owned(),
                tokenizer_artifact: AiTokenizerArtifactConfig { path, sha256 },
            },
        );
        config.default_embedding_model = Some(alias.to_owned());
        config
    }

    #[test]
    fn pinned_offline_artifact_counts_tokens_and_rejects_contract_drift() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tokenizer-word-level.json");
        let bytes = std::fs::read(&path).expect("fixture tokenizer");
        let config = fixture_config(path, hex::encode(Sha256::digest(bytes)));
        let registry = AiTokenizerRegistry::load(&config).expect("registry");
        let contract = config.resolve_embedding_model(None).expect("contract");
        let tokenizer = registry.for_contract(&contract).expect("tokenizer");
        assert_eq!(tokenizer.count("alpha beta alpha"), 4);
        assert_ne!(
            tokenizer.count("alpha beta alpha"),
            "alpha beta alpha".chars().count()
        );

        let mut drifted = contract;
        drifted.tokenizer_revision = "different".to_owned();
        assert!(registry.for_contract(&drifted).is_err());

        let keyword = config.resolve_tokenizer(None).expect("keyword contract");
        assert_eq!(
            registry
                .for_tokenizer_contract(&keyword)
                .expect("keyword tokenizer")
                .count("alpha beta"),
            3
        );
    }

    #[test]
    fn missing_digest_and_symlink_fail_closed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("tokenizer.json");
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/tokenizer-word-level.json"),
            &target,
        )
        .expect("copy fixture");
        let wrong = fixture_config(target.clone(), "0".repeat(64));
        assert_eq!(
            AiTokenizerRegistry::load(&wrong)
                .expect_err("digest")
                .code(),
            ErrorCode::ArtifactIntegrityError
        );

        #[cfg(unix)]
        {
            let link = directory.path().join("tokenizer-link.json");
            std::os::unix::fs::symlink(target, &link).expect("symlink");
            let bytes = std::fs::read(&link).expect("read link");
            let linked = fixture_config(link, hex::encode(Sha256::digest(bytes)));
            assert_eq!(
                AiTokenizerRegistry::load(&linked)
                    .expect_err("symlink")
                    .code(),
                ErrorCode::ConfigPathInvalid
            );
        }
    }
}
