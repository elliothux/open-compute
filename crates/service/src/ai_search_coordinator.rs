//! Durable per-instance AI Search indexing coordinator.

use crate::ai_provider::{AiProviderError, OpenAiProviderClient};
use crate::document_parser_backend::DocumentParserBindingService;
use crate::metrics::{AiIndexStage, AiProviderCapability, AiProviderOutcome, MetricsRegistry};
use open_compute_artifacts::{AiSearchObjectRef, AiSearchObjectStore};
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use open_compute_search::ai_search::{ChunkConfig, chunk_text};
use open_compute_storage::{AiSearchJobClaim, AiSearchStore, StagedAiSearchChunk};
use sha2::{Digest as _, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt as _;
use tokio::sync::{RwLock, Semaphore};

type TaskFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Exact source bytes returned only after object identity verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchSourceDocument {
    /// Exact source bytes.
    pub bytes: Vec<u8>,
}

/// Source-object boundary used by the coordinator and implemented by the selected object backend.
pub trait AiSearchSourceReader: Send + Sync + std::fmt::Debug {
    /// Download and verify the exact object named by a durable claim.
    fn read<'a>(
        &'a self,
        claim: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>>;
}

/// Parser-child boundary used by the indexing coordinator.
pub trait AiSearchDocumentParser: Send + Sync + std::fmt::Debug {
    /// Parse one exact source into normalized Markdown.
    fn parse<'a>(
        &'a self,
        claim: &'a AiSearchJobClaim,
        bytes: Vec<u8>,
    ) -> TaskFuture<'a, Result<String, PlatformError>>;
}

/// Frozen tokenizer used for chunk-size and overlap enforcement.
pub trait AiSearchTokenCounter: Send + Sync + std::fmt::Debug {
    /// Count model tokens in normalized text using the frozen tokenizer revision.
    /// Return zero when tokenization fails so non-empty input is rejected.
    fn count(&self, text: &str) -> usize;
}

/// Frozen embedding boundary used for document chunks.
pub trait AiSearchEmbedder: Send + Sync + std::fmt::Debug {
    /// Exact vector dimensions.
    fn dimensions(&self) -> usize;
    /// Maximum inputs in one provider request.
    fn max_batch(&self) -> usize;
    /// Embed one ordered batch.
    fn embed<'a>(
        &'a self,
        input: &'a [String],
    ) -> TaskFuture<'a, Result<Vec<Vec<f32>>, AiProviderError>>;
}

/// Production object reader restricted to the exact content-addressed object key.
#[derive(Clone, Debug)]
pub struct ObjectAiSearchSourceReader {
    objects: AiSearchObjectStore,
    account: AccountId,
    instance: ResourceId,
}

impl ObjectAiSearchSourceReader {
    /// Bind one instance to the platform system-object store.
    #[must_use]
    pub const fn new(
        objects: AiSearchObjectStore,
        account: AccountId,
        instance: ResourceId,
    ) -> Self {
        Self {
            objects,
            account,
            instance,
        }
    }
}

impl AiSearchSourceReader for ObjectAiSearchSourceReader {
    fn read<'a>(
        &'a self,
        claim: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>> {
        Box::pin(async move {
            let reference = AiSearchObjectRef::new(
                self.account,
                self.instance,
                claim.item.object_sha256,
                claim.item.object_size,
            )?;
            let download = self
                .objects
                .download(&reference, &claim.item.object_key)
                .await?;
            let expected = usize::try_from(download.size).map_err(|_| limit())?;
            let mut bytes = Vec::with_capacity(expected);
            let mut reader = download.body.into_async_read();
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = reader.read(&mut buffer).await.map_err(|_| unavailable())?;
                if read == 0 {
                    break;
                }
                if bytes
                    .len()
                    .checked_add(read)
                    .is_none_or(|size| size > expected)
                {
                    return Err(integrity());
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            if bytes.len() != usize::try_from(claim.item.object_size).map_err(|_| limit())?
                || digest != claim.item.object_sha256
            {
                return Err(integrity());
            }
            Ok(AiSearchSourceDocument { bytes })
        })
    }
}

/// Production isolated parser-child adapter.
#[derive(Clone)]
pub struct IsolatedAiSearchDocumentParser {
    parser: Arc<DocumentParserBindingService>,
    account: AccountId,
}

impl IsolatedAiSearchDocumentParser {
    /// Bind the account-scoped parser child service.
    #[must_use]
    pub const fn new(parser: Arc<DocumentParserBindingService>, account: AccountId) -> Self {
        Self { parser, account }
    }
}

impl std::fmt::Debug for IsolatedAiSearchDocumentParser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IsolatedAiSearchDocumentParser")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl AiSearchDocumentParser for IsolatedAiSearchDocumentParser {
    fn parse<'a>(
        &'a self,
        claim: &'a AiSearchJobClaim,
        bytes: Vec<u8>,
    ) -> TaskFuture<'a, Result<String, PlatformError>> {
        Box::pin(async move {
            self.parser
                .parse_for_ai_search(
                    self.account,
                    &claim.item.key,
                    &claim.item.content_type,
                    bytes,
                )
                .await
                .map(|success| success.markdown)
        })
    }
}

impl AiSearchEmbedder for OpenAiProviderClient {
    fn dimensions(&self) -> usize {
        self.dimensions()
    }

    fn max_batch(&self) -> usize {
        self.max_inputs_per_batch()
    }

    fn embed<'a>(
        &'a self,
        input: &'a [String],
    ) -> TaskFuture<'a, Result<Vec<Vec<f32>>, AiProviderError>> {
        Box::pin(async move { self.embeddings(input).await.map(|batch| batch.embeddings) })
    }
}

/// Result of one bounded coordinator pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AiSearchCoordinatorPass {
    /// Jobs successfully activated.
    pub completed: u64,
    /// Jobs moved to retry wait.
    pub retried: u64,
    /// Jobs settled as permanent errors.
    pub failed: u64,
    /// Whether no due job remained.
    pub idle: bool,
}

/// One instance coordinator with no in-memory authority.
#[derive(Debug)]
pub struct AiSearchCoordinator {
    source: Arc<dyn AiSearchSourceReader>,
    parser: Arc<dyn AiSearchDocumentParser>,
    tokenizer: Arc<dyn AiSearchTokenCounter>,
    embedder: Option<Arc<dyn AiSearchEmbedder>>,
    chunk: ChunkConfig,
    lease_ms: u64,
    retry_base_ms: u64,
    metrics: Option<Arc<MetricsRegistry>>,
    provider_permits: Option<Arc<Semaphore>>,
    activation_lock: Option<Arc<RwLock<()>>>,
}

impl AiSearchCoordinator {
    /// Compose one coordinator from frozen instance dependencies.
    pub fn new(
        source: Arc<dyn AiSearchSourceReader>,
        parser: Arc<dyn AiSearchDocumentParser>,
        tokenizer: Arc<dyn AiSearchTokenCounter>,
        embedder: Option<Arc<dyn AiSearchEmbedder>>,
        chunk: ChunkConfig,
        lease_ms: u64,
        retry_base_ms: u64,
    ) -> Result<Self, PlatformError> {
        chunk.validate().map_err(|_| limit())?;
        if lease_ms == 0
            || retry_base_ms == 0
            || embedder
                .as_ref()
                .is_some_and(|value| value.dimensions() == 0 || value.max_batch() == 0)
        {
            return Err(limit());
        }
        Ok(Self {
            source,
            parser,
            tokenizer,
            embedder,
            chunk,
            lease_ms,
            retry_base_ms,
            metrics: None,
            provider_permits: None,
            activation_lock: None,
        })
    }

    /// Attach the platform fixed-cardinality metrics owner.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Share the process-wide provider admission semaphore.
    #[must_use]
    pub fn with_provider_permits(mut self, permits: Arc<Semaphore>) -> Self {
        self.provider_permits = Some(permits);
        self
    }

    /// Serialize active generation changes against queries for this instance.
    #[must_use]
    pub fn with_activation_lock(mut self, lock: Arc<RwLock<()>>) -> Self {
        self.activation_lock = Some(lock);
        self
    }

    /// Claim and execute at most one durable job.
    pub async fn run_once(
        &self,
        store: &AiSearchStore,
        now_ms: i64,
    ) -> Result<AiSearchCoordinatorPass, PlatformError> {
        if store.vector_enabled() != self.embedder.is_some()
            || self
                .embedder
                .as_ref()
                .is_some_and(|value| value.dimensions() != store.dimensions())
        {
            return Err(integrity());
        }
        let Some(claim) = store.claim_due_job(now_ms, self.lease_ms)? else {
            return Ok(AiSearchCoordinatorPass {
                idle: true,
                ..AiSearchCoordinatorPass::default()
            });
        };
        match self.process_claim(store, &claim).await {
            Ok(true) => Ok(AiSearchCoordinatorPass {
                completed: 1,
                ..AiSearchCoordinatorPass::default()
            }),
            Ok(false) => Ok(AiSearchCoordinatorPass::default()),
            Err(Failure::Transient(retry_after_seconds)) => {
                let exponential = retry_delay(self.retry_base_ms, claim.attempt)?;
                let provider_delay = retry_after_seconds
                    .unwrap_or(0)
                    .checked_mul(1_000)
                    .ok_or_else(limit)?;
                let delay = exponential.max(provider_delay);
                let settled_at = match current_time_ms() {
                    Ok(value) => value,
                    Err(_) => now_ms,
                };
                let next = settled_at
                    .checked_add(i64::try_from(delay).map_err(|_| limit())?)
                    .ok_or_else(limit)?;
                let settled = store.fail_claim(&claim, true, next, settled_at)?;
                if !settled {
                    let _ = store.acknowledge_cancel(&claim, settled_at);
                }
                Ok(AiSearchCoordinatorPass {
                    retried: u64::from(settled),
                    ..AiSearchCoordinatorPass::default()
                })
            }
            Err(Failure::Permanent) => {
                let settled_at = match current_time_ms() {
                    Ok(value) => value,
                    Err(_) => now_ms,
                };
                let settled = store.fail_claim(&claim, false, settled_at, settled_at)?;
                if !settled {
                    let _ = store.acknowledge_cancel(&claim, settled_at);
                }
                Ok(AiSearchCoordinatorPass {
                    failed: u64::from(settled),
                    ..AiSearchCoordinatorPass::default()
                })
            }
        }
    }

    /// Startup reconciliation: recover expired claims and drain a bounded due frontier.
    pub async fn run_until_idle(
        &self,
        store: &AiSearchStore,
        mut now_ms: i64,
        maximum_jobs: usize,
    ) -> Result<AiSearchCoordinatorPass, PlatformError> {
        if maximum_jobs == 0 {
            return Err(limit());
        }
        let mut total = AiSearchCoordinatorPass::default();
        for _ in 0..maximum_jobs {
            let pass = self.run_once(store, now_ms).await?;
            total.completed += pass.completed;
            total.retried += pass.retried;
            total.failed += pass.failed;
            if pass.idle {
                total.idle = true;
                break;
            }
            now_ms = now_ms.saturating_add(1);
        }
        Ok(total)
    }

    /// Reconcile expired work and drain a bounded due frontier during startup.
    pub async fn run_startup(
        &self,
        store: &AiSearchStore,
        now_ms: i64,
        maximum_jobs: usize,
    ) -> Result<AiSearchCoordinatorPass, PlatformError> {
        self.run_until_idle(store, now_ms, maximum_jobs).await
    }

    /// Periodically reconcile due work until the owner sets `stop`.
    pub async fn run_periodic(
        &self,
        store: &AiSearchStore,
        stop: &AtomicBool,
        interval: Duration,
        clock_ms: impl Fn() -> i64,
    ) -> Result<(), PlatformError> {
        if interval.is_zero() {
            return Err(limit());
        }
        while !stop.load(Ordering::Acquire) {
            let _ = self.run_until_idle(store, clock_ms(), 32).await?;
            tokio::time::sleep(interval).await;
        }
        Ok(())
    }

    async fn process_claim(
        &self,
        store: &AiSearchStore,
        claim: &AiSearchJobClaim,
    ) -> Result<bool, Failure> {
        let source = self.source.read(claim).await;
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_search_object(1, source.is_ok());
        }
        let source = source.map_err(|error| classify_platform(&error))?;
        let started = Instant::now();
        let markdown = self
            .parser
            .parse(claim, source.bytes)
            .await
            .map_err(|error| classify_platform(&error))?;
        self.observe_stage(AiIndexStage::Parse, started);
        let started = Instant::now();
        let tokenizer = self.tokenizer.clone();
        let chunk = self.chunk;
        let chunks = tokio::task::spawn_blocking(move || {
            chunk_text(&markdown, chunk, |text| tokenizer.count(text))
        })
        .await
        .map_err(|_| Failure::Permanent)?
        .map_err(|_| Failure::Permanent)?;
        self.observe_stage(AiIndexStage::Chunk, started);
        if chunks.is_empty() {
            return Err(Failure::Permanent);
        }
        let mut next = usize::try_from(claim.next_batch_ordinal).map_err(|_| Failure::Permanent)?;
        if next > chunks.len() {
            return Err(Failure::Permanent);
        }
        let batch_size = self
            .embedder
            .as_ref()
            .map_or(64, |embedder| embedder.max_batch());
        while next < chunks.len() {
            let end = next.saturating_add(batch_size).min(chunks.len());
            let batch = &chunks[next..end];
            let lease_now = current_time_ms().map_err(|error| classify_platform(&error))?;
            if !store
                .renew_claim(claim, lease_now, self.lease_ms)
                .map_err(|error| classify_platform(&error))?
            {
                let _ = store.acknowledge_cancel(claim, lease_now);
                return Ok(false);
            }
            let vectors = if let Some(embedder) = &self.embedder {
                let input = batch
                    .iter()
                    .map(|chunk| chunk.text.clone())
                    .collect::<Vec<_>>();
                let started = Instant::now();
                let _permit = match &self.provider_permits {
                    Some(permits) => Some(
                        permits
                            .clone()
                            .acquire_owned()
                            .await
                            .map_err(|_| Failure::Transient(None))?,
                    ),
                    None => None,
                };
                let embedded = embedder.embed(&input).await;
                self.observe_provider(&embedded, input.len());
                self.observe_stage(AiIndexStage::Embed, started);
                let embedded = embedded.map_err(classify_provider)?;
                if embedded.len() != batch.len()
                    || embedded.iter().any(|vector| {
                        vector.len() != embedder.dimensions()
                            || vector.iter().any(|value| !value.is_finite())
                    })
                {
                    return Err(Failure::Permanent);
                }
                embedded.into_iter().map(Some).collect::<Vec<_>>()
            } else {
                vec![None; batch.len()]
            };
            let encoded = vectors
                .iter()
                .map(|vector| {
                    vector.as_ref().map(|vector| {
                        let bytes = vector
                            .iter()
                            .flat_map(|value| value.to_le_bytes())
                            .collect::<Vec<_>>();
                        let norm = vector
                            .iter()
                            .map(|value| f64::from(*value).powi(2))
                            .sum::<f64>()
                            .sqrt();
                        (bytes, norm)
                    })
                })
                .collect::<Vec<_>>();
            let ids = batch
                .iter()
                .map(|chunk| stable_chunk_id(claim, chunk.ordinal))
                .collect::<Result<Vec<_>, _>>()?;
            let staged = batch
                .iter()
                .enumerate()
                .map(|(index, chunk)| {
                    Ok(StagedAiSearchChunk {
                        chunk_id: &ids[index],
                        ordinal: u32::try_from(chunk.ordinal).map_err(|_| Failure::Permanent)?,
                        start_byte: u64::try_from(chunk.start_byte)
                            .map_err(|_| Failure::Permanent)?,
                        end_byte: u64::try_from(chunk.end_byte).map_err(|_| Failure::Permanent)?,
                        text: &chunk.text,
                        embedding_f32le: encoded[index].as_ref().map(|value| value.0.as_slice()),
                        vector_norm: encoded[index].as_ref().map(|value| value.1),
                        metadata_json: &claim.item.metadata_json,
                    })
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            let staged_at = current_time_ms().map_err(|error| classify_platform(&error))?;
            if !store
                .stage_item_generation_batch(
                    claim,
                    u32::try_from(next).map_err(|_| Failure::Permanent)?,
                    &staged,
                    staged_at,
                )
                .map_err(|error| classify_platform(&error))?
            {
                let _ = store.acknowledge_cancel(claim, staged_at);
                return Ok(false);
            }
            next = end;
        }
        let activated_at = current_time_ms().map_err(|error| classify_platform(&error))?;
        let started = Instant::now();
        let _activation = match &self.activation_lock {
            Some(lock) => Some(lock.write().await),
            None => None,
        };
        let activated = store
            .complete_staged_item_generation(
                claim,
                u32::try_from(chunks.len()).map_err(|_| Failure::Permanent)?,
                activated_at,
            )
            .map_err(|error| classify_platform(&error))?;
        self.observe_stage(AiIndexStage::Activate, started);
        if !activated {
            let _ = store.acknowledge_cancel(claim, activated_at);
        }
        Ok(activated)
    }

    fn observe_stage(&self, stage: AiIndexStage, started: Instant) {
        if let Some(metrics) = &self.metrics {
            metrics.observe_ai_index_stage(stage, started.elapsed());
        }
    }

    fn observe_provider(&self, result: &Result<Vec<Vec<f32>>, AiProviderError>, inputs: usize) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let outcome = match result {
            Ok(_) => AiProviderOutcome::Success,
            Err(error) => provider_outcome(*error),
        };
        let response_bytes = result.as_ref().ok().and_then(|vectors| {
            vectors.iter().try_fold(0_u64, |total, vector| {
                u64::try_from(vector.len())
                    .ok()
                    .and_then(|length| length.checked_mul(4))
                    .and_then(|length| total.checked_add(length))
            })
        });
        let response_bytes = response_bytes.unwrap_or_default();
        let inputs = u64::try_from(inputs).unwrap_or_default();
        metrics.observe_ai_provider(
            AiProviderCapability::Embedding,
            outcome,
            inputs,
            response_bytes,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Failure {
    Transient(Option<u64>),
    Permanent,
}

fn stable_chunk_id(claim: &AiSearchJobClaim, ordinal: usize) -> Result<String, Failure> {
    let mut digest = Sha256::new();
    digest.update(b"open-compute-ai-search-chunk-v1\0");
    digest.update(claim.item.item_id.as_bytes());
    digest.update(claim.item.object_sha256);
    digest.update(claim.config_generation.to_be_bytes());
    digest.update(claim.index_generation.to_be_bytes());
    digest.update(claim.item.generation.to_be_bytes());
    digest.update(
        u64::try_from(ordinal)
            .map_err(|_| Failure::Permanent)?
            .to_be_bytes(),
    );
    Ok(hex::encode(digest.finalize()))
}

fn retry_delay(base_ms: u64, attempt: u32) -> Result<u64, PlatformError> {
    let exponent = attempt.saturating_sub(1).min(10);
    base_ms.checked_mul(1_u64 << exponent).ok_or_else(limit)
}

fn classify_provider(error: AiProviderError) -> Failure {
    match error {
        AiProviderError::RateLimited {
            retry_after_seconds,
        } => Failure::Transient(retry_after_seconds),
        AiProviderError::Transient | AiProviderError::Timeout => Failure::Transient(None),
        _ => Failure::Permanent,
    }
}

const fn provider_outcome(error: AiProviderError) -> AiProviderOutcome {
    match error {
        AiProviderError::InvalidRequest | AiProviderError::ContractMismatch => {
            AiProviderOutcome::Invalid
        }
        AiProviderError::Unauthorized => AiProviderOutcome::Unauthorized,
        AiProviderError::RateLimited { .. } => AiProviderOutcome::RateLimited,
        AiProviderError::Transient => AiProviderOutcome::Transient,
        AiProviderError::Permanent => AiProviderOutcome::Permanent,
        AiProviderError::Timeout => AiProviderOutcome::Timeout,
        AiProviderError::MalformedResponse => AiProviderOutcome::Malformed,
    }
}

fn classify_platform(error: &PlatformError) -> Failure {
    match error.code() {
        ErrorCode::ObjectStorageUnavailable
        | ErrorCode::PlatformUnavailable
        | ErrorCode::DocumentUnavailable
        | ErrorCode::DocumentTimeout => Failure::Transient(None),
        _ => Failure::Permanent,
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageUnavailable,
        "AI Search source object is unavailable",
    )
}

fn integrity() -> PlatformError {
    PlatformError::new(
        ErrorCode::ArtifactIntegrityError,
        "AI Search source object failed integrity verification",
    )
}

fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::LimitInvalid,
        "AI Search coordinator limit is invalid",
    )
}

fn current_time_ms() -> Result<i64, PlatformError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| unavailable())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| limit())
}

#[cfg(test)]
#[path = "ai_search_coordinator_tests.rs"]
mod tests;
