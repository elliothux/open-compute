//! Durable derived-document cache integration for the indexing coordinator.

use super::{
    AiSearchCoordinator, AiSearchJobClaim, Failure, INDEX_FAILURE_CODE, PlatformError,
    classify_platform, current_time_ms, integrity,
};
use open_compute_document_parser::{
    DocumentFormat, DocumentMetadata, ParsedContentKind, canonical_content_type,
};
use open_compute_storage::{
    AiSearchParseCacheKey, AiSearchParseCacheLookup, AiSearchParseCacheStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Mutex as AsyncMutex;

type CacheLockKey = (open_compute_core::ResourceId, [u8; 32]);
type CacheLock = Weak<AsyncMutex<()>>;

/// Parsed content plus the complete parser/OCR/VLM semantic identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AiSearchParsedDocument {
    /// Normalized text submitted to chunking.
    pub content: String,
    /// SHA-256 of the normalized Markdown bytes.
    pub markdown_sha256: String,
    /// Format selected by the canonical admission registry.
    pub format: DocumentFormat,
    /// Canonical content type selected by admission.
    pub detected_content_type: String,
    /// Whether the normalized content preserves plain text or is Markdown.
    pub content_kind: ParsedContentKind,
    /// PDF page count when available.
    pub page_count: Option<u32>,
    /// Spreadsheet sheet count when available.
    pub sheet_count: Option<u32>,
    /// Spreadsheet sheet names in stable workbook order.
    pub sheet_names: Option<Vec<String>>,
    /// Bounded document metadata returned by the parser.
    pub metadata: DocumentMetadata,
    /// Stable parser warning codes.
    pub warnings: Vec<String>,
    /// Digest of parser, OCR, VLM, preprocessing, output-kind, and language semantics.
    pub semantic_contract_sha256: String,
}

/// Process-local single-flight locks; durable cache bytes remain the only reusable value.
#[derive(Debug, Default)]
pub(crate) struct AiSearchParseCacheLocks {
    locks: Mutex<HashMap<CacheLockKey, CacheLock>>,
}

impl AiSearchParseCacheLocks {
    fn for_key(
        &self,
        scope: open_compute_core::ResourceId,
        key: [u8; 32],
    ) -> Result<Arc<AsyncMutex<()>>, PlatformError> {
        let mut locks = self.locks.lock().map_err(|_| integrity())?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&(scope, key)).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert((scope, key), Arc::downgrade(&lock));
        Ok(lock)
    }
}

impl AiSearchCoordinator {
    pub(super) async fn parsed_document(
        &self,
        claim: &AiSearchJobClaim,
    ) -> Result<AiSearchParsedDocument, Failure> {
        let Some(cache) = &self.parse_cache else {
            return self.read_and_parse(claim).await;
        };
        let content_type = canonical_content_type(&claim.item.content_type)
            .map_err(|_| Failure::Permanent(INDEX_FAILURE_CODE))?;
        let key = AiSearchParseCacheKey::new(
            claim.item.source.identity_sha256(),
            claim.item.source.object_size(),
            &claim.item.key,
            &content_type,
            self.parser.cache_contract_sha256(),
        )
        .map_err(|error| classify_platform(&error))?;
        let Some(locks) = &self.parse_cache_locks else {
            return Err(Failure::Permanent(INDEX_FAILURE_CODE));
        };
        let Some(scope) = self.parse_cache_scope else {
            return Err(Failure::Permanent(INDEX_FAILURE_CODE));
        };
        let lock = locks
            .for_key(scope, key.digest())
            .map_err(|error| classify_platform(&error))?;
        let _guard = lock.lock().await;
        match cache.get(&key, current_time_ms()) {
            Ok(AiSearchParseCacheLookup::Hit(payload)) => {
                if let Ok(parsed) = serde_json::from_slice::<AiSearchParsedDocument>(&payload)
                    && valid_cached_document(&parsed)
                {
                    self.observe_parse_cache(0);
                    return Ok(parsed);
                }
                let _ = cache.discard(&key);
                self.observe_parse_cache(3);
            }
            Ok(AiSearchParseCacheLookup::Miss) => self.observe_parse_cache(1),
            Ok(AiSearchParseCacheLookup::Corrupt) | Err(_) => self.observe_parse_cache(3),
        }
        let parsed = self.read_and_parse(claim).await?;
        if let Ok(payload) = serde_json::to_vec(&parsed) {
            match cache.put(&key, &payload, current_time_ms()) {
                Ok(AiSearchParseCacheStore::Stored { evicted }) => {
                    self.observe_parse_cache(2);
                    for _ in 0..evicted {
                        self.observe_parse_cache(4);
                    }
                }
                Ok(AiSearchParseCacheStore::TooLarge) | Err(_) => self.observe_parse_cache(3),
            }
        } else {
            self.observe_parse_cache(3);
        }
        Ok(parsed)
    }

    async fn read_and_parse(
        &self,
        claim: &AiSearchJobClaim,
    ) -> Result<AiSearchParsedDocument, Failure> {
        let source = self.source.read(claim).await;
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_search_object(1, source.is_ok());
        }
        let source = source.map_err(|error| classify_platform(&error))?;
        self.parser
            .parse(claim, source.bytes)
            .await
            .map_err(|error| classify_platform(&error))
    }

    fn observe_parse_cache(&self, outcome: usize) {
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_search_parse_cache(outcome);
        }
    }
}

fn valid_cached_document(parsed: &AiSearchParsedDocument) -> bool {
    !parsed.content.trim().is_empty()
        && parsed.content.len() <= 16 * 1024 * 1024
        && parsed.markdown_sha256 == hex::encode(Sha256::digest(parsed.content.as_bytes()))
        && parsed.semantic_contract_sha256.len() == 64
        && hex::decode(&parsed.semantic_contract_sha256).is_ok_and(|digest| digest.len() == 32)
        && !parsed.detected_content_type.is_empty()
        && parsed.detected_content_type.len() <= 128
        && parsed.warnings.len() <= 128
        && parsed
            .warnings
            .iter()
            .all(|warning| !warning.is_empty() && warning.len() <= 128)
}
