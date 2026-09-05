//! Focused coordinator recovery tests.

use super::*;
use open_compute_storage::{AiSearchInstanceStorageContract, NewAiSearchItemGeneration};
use uuid::Uuid;

fn open_store(vector_enabled: bool) -> (tempfile::TempDir, AiSearchStore) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("instance.sqlite");
    let model = if vector_enabled {
        br#"{"dimensions":1,"metric":"cosine","tokenizer":"fixture","tokenizerRevision":"1","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a"}"#.as_slice()
    } else {
        br#"{"kind":"keyword_only","schemaVersion":1,"tokenizerContract":{"embeddingAlias":"fixture","tokenizer":"fixture","tokenizerRevision":"1","tokenizerArtifactSha256":"def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a","maxInputTokens":512,"contractSha256":"fixture-contract"}}"#.as_slice()
    };
    let public_config = if vector_enabled {
        br#"{"chunk":true,"chunk_overlap":2,"chunk_size":8,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#.as_slice()
    } else {
        br#"{"chunk":true,"chunk_overlap":2,"chunk_size":8,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":false},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#.as_slice()
    };
    let store = AiSearchStore::open(
        &path,
        &AiSearchInstanceStorageContract {
            resource_id: "instance-1",
            model_contract_sha256: Sha256::digest(model).into(),
            model_contract_json: model,
            public_config_json: public_config,
            dimensions: u32::from(vector_enabled),
            vector_enabled,
            keyword_enabled: true,
        },
        1,
    )
    .expect("store");
    (directory, store)
}

fn enqueue_fixture(store: &AiSearchStore, job_id: &str) -> i64 {
    let now = current_time_ms().expect("clock");
    store
        .enqueue_item_generation(
            job_id,
            &NewAiSearchItemGeneration {
                item_id: "item-1",
                key: "fixture.txt",
                source: "builtin",
                generation: 1,
                index_generation: 1,
                object_key: "ai-search/v1/a/i/objects/sha256/00/0011",
                object_sha256: [7; 32],
                object_size: 14,
                content_type: "text/plain",
                metadata_json: b"{}",
                now_ms: now,
            },
        )
        .expect("enqueue");
    now
}

#[derive(Debug)]
struct FixtureSource;

impl AiSearchSourceReader for FixtureSource {
    fn read<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>> {
        Box::pin(async {
            Ok(AiSearchSourceDocument {
                bytes: b"fixture source".to_vec(),
            })
        })
    }
}

#[derive(Debug)]
struct FixtureParser;

impl AiSearchDocumentParser for FixtureParser {
    fn parse<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
        _: Vec<u8>,
    ) -> TaskFuture<'a, Result<String, PlatformError>> {
        Box::pin(async { Ok("alpha beta gamma delta".to_owned()) })
    }
}

#[derive(Debug)]
struct CharacterTokenizer;

impl AiSearchTokenCounter for CharacterTokenizer {
    fn count(&self, text: &str) -> usize {
        text.chars().count()
    }
}

#[derive(Debug)]
struct FixtureEmbedder;

impl AiSearchEmbedder for FixtureEmbedder {
    fn dimensions(&self) -> usize {
        1
    }

    fn max_batch(&self) -> usize {
        2
    }

    fn embed<'a>(
        &'a self,
        input: &'a [String],
    ) -> TaskFuture<'a, Result<Vec<Vec<f32>>, AiProviderError>> {
        Box::pin(async move { Ok(input.iter().map(|_| vec![1.0]).collect()) })
    }
}

#[derive(Debug)]
struct FailingSource(ErrorCode);

impl AiSearchSourceReader for FailingSource {
    fn read<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>> {
        Box::pin(async move { Err(PlatformError::new(self.0, "fixture source failure")) })
    }
}

#[derive(Debug)]
struct FailingParser(ErrorCode);

impl AiSearchDocumentParser for FailingParser {
    fn parse<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
        _: Vec<u8>,
    ) -> TaskFuture<'a, Result<String, PlatformError>> {
        Box::pin(async move { Err(PlatformError::new(self.0, "fixture parser failure")) })
    }
}

#[derive(Clone, Copy, Debug)]
enum EmbeddingFailure {
    RateLimited,
    Timeout,
    MalformedDimensions,
    NonFinite,
}

#[derive(Debug)]
struct FailingEmbedder(EmbeddingFailure);

impl AiSearchEmbedder for FailingEmbedder {
    fn dimensions(&self) -> usize {
        1
    }

    fn max_batch(&self) -> usize {
        2
    }

    fn embed<'a>(
        &'a self,
        input: &'a [String],
    ) -> TaskFuture<'a, Result<Vec<Vec<f32>>, AiProviderError>> {
        Box::pin(async move {
            match self.0 {
                EmbeddingFailure::RateLimited => Err(AiProviderError::RateLimited {
                    retry_after_seconds: Some(2),
                }),
                EmbeddingFailure::Timeout => Err(AiProviderError::Timeout),
                EmbeddingFailure::MalformedDimensions => {
                    Ok(input.iter().map(|_| vec![1.0, 2.0]).collect())
                }
                EmbeddingFailure::NonFinite => Ok(input.iter().map(|_| vec![f32::NAN]).collect()),
            }
        })
    }
}

fn coordinator(
    source: Arc<dyn AiSearchSourceReader>,
    parser: Arc<dyn AiSearchDocumentParser>,
    embedder: Option<Arc<dyn AiSearchEmbedder>>,
) -> AiSearchCoordinator {
    AiSearchCoordinator::new(
        source,
        parser,
        Arc::new(CharacterTokenizer),
        embedder,
        ChunkConfig {
            max_tokens: 8,
            overlap_tokens: 2,
        },
        60_000,
        100,
    )
    .expect("coordinator")
}

#[tokio::test]
async fn startup_reclaims_crashed_job_and_fenced_activation_completes() {
    let (_directory, store) = open_store(true);
    let now = enqueue_fixture(&store, "job-1");
    let crashed = store.claim_due_job(now, 1).expect("claim").expect("due");
    let coordinator = AiSearchCoordinator::new(
        Arc::new(FixtureSource),
        Arc::new(FixtureParser),
        Arc::new(CharacterTokenizer),
        Some(Arc::new(FixtureEmbedder)),
        ChunkConfig {
            max_tokens: 8,
            overlap_tokens: 2,
        },
        60_000,
        100,
    )
    .expect("coordinator");
    let pass = coordinator
        .run_startup(&store, crashed.claim_until_ms, 4)
        .await
        .expect("startup reconciliation");
    assert_eq!(pass.completed, 1);
    assert!(pass.idle);
    assert_eq!(
        store.item_state("item-1").expect("state"),
        Some(("completed".to_owned(), Some(1)))
    );
    let (chunks, count) = store.active_chunks(Some("item-1"), 0, 100).unwrap();
    assert_eq!(usize::try_from(count).unwrap(), chunks.len());
    assert!(chunks.len() > 1);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.embedding.as_deref() == Some(&[1.0]))
    );
}

#[tokio::test]
async fn keyword_only_coordinator_activates_without_embeddings() {
    let (_directory, store) = open_store(false);
    let now = enqueue_fixture(&store, "job-keyword");
    let coordinator = coordinator(Arc::new(FixtureSource), Arc::new(FixtureParser), None);
    let pass = coordinator.run_until_idle(&store, now, 4).await.unwrap();
    assert_eq!(pass.completed, 1);
    assert!(pass.idle);
    let (chunks, _) = store.active_chunks(Some("item-1"), 0, 100).unwrap();
    assert!(chunks.len() > 1);
    assert!(chunks.iter().all(|chunk| chunk.embedding.is_none()));
}

#[tokio::test]
async fn transient_source_and_parser_failures_are_durably_retried() {
    for (source, parser) in [
        (
            Arc::new(FailingSource(ErrorCode::ObjectStorageUnavailable))
                as Arc<dyn AiSearchSourceReader>,
            Arc::new(FixtureParser) as Arc<dyn AiSearchDocumentParser>,
        ),
        (
            Arc::new(FixtureSource) as Arc<dyn AiSearchSourceReader>,
            Arc::new(FailingParser(ErrorCode::DocumentTimeout)) as Arc<dyn AiSearchDocumentParser>,
        ),
    ] {
        let (_directory, store) = open_store(true);
        let now = enqueue_fixture(&store, &Uuid::now_v7().to_string());
        let pass = coordinator(source, parser, Some(Arc::new(FixtureEmbedder)))
            .run_once(&store, now)
            .await
            .unwrap();
        assert_eq!(pass.retried, 1);
        assert_eq!(pass.failed, 0);
        assert_eq!(
            store.item_state("item-1").unwrap(),
            Some(("queued".to_owned(), None))
        );
    }
}

#[tokio::test]
async fn permanent_parser_and_malformed_embedding_failures_set_error() {
    let cases: Vec<(Arc<dyn AiSearchDocumentParser>, Arc<dyn AiSearchEmbedder>)> = vec![
        (
            Arc::new(FailingParser(ErrorCode::DocumentProtocolError)),
            Arc::new(FixtureEmbedder),
        ),
        (
            Arc::new(FixtureParser),
            Arc::new(FailingEmbedder(EmbeddingFailure::MalformedDimensions)),
        ),
        (
            Arc::new(FixtureParser),
            Arc::new(FailingEmbedder(EmbeddingFailure::NonFinite)),
        ),
    ];
    for (parser, embedder) in cases {
        let (_directory, store) = open_store(true);
        let now = enqueue_fixture(&store, &Uuid::now_v7().to_string());
        let pass = coordinator(Arc::new(FixtureSource), parser, Some(embedder))
            .run_once(&store, now)
            .await
            .unwrap();
        assert_eq!(pass.failed, 1);
        assert_eq!(pass.retried, 0);
        assert_eq!(
            store.item_state("item-1").unwrap(),
            Some(("error".to_owned(), None))
        );
    }
}

#[tokio::test]
async fn provider_backpressure_failures_retry_and_idle_frontier_is_reported() {
    for failure in [EmbeddingFailure::RateLimited, EmbeddingFailure::Timeout] {
        let (_directory, store) = open_store(true);
        let now = enqueue_fixture(&store, &Uuid::now_v7().to_string());
        let pass = coordinator(
            Arc::new(FixtureSource),
            Arc::new(FixtureParser),
            Some(Arc::new(FailingEmbedder(failure))),
        )
        .with_provider_permits(Arc::new(Semaphore::new(1)))
        .run_once(&store, now)
        .await
        .unwrap();
        assert_eq!(pass.retried, 1);
        let idle = coordinator(
            Arc::new(FixtureSource),
            Arc::new(FixtureParser),
            Some(Arc::new(FixtureEmbedder)),
        )
        .run_once(&store, now)
        .await
        .unwrap();
        assert!(idle.idle);
    }
}

#[tokio::test]
async fn constructor_frontier_and_store_contract_limits_fail_closed() {
    assert!(
        AiSearchCoordinator::new(
            Arc::new(FixtureSource),
            Arc::new(FixtureParser),
            Arc::new(CharacterTokenizer),
            Some(Arc::new(FixtureEmbedder)),
            ChunkConfig {
                max_tokens: 8,
                overlap_tokens: 2,
            },
            0,
            100,
        )
        .is_err()
    );

    let (_directory, store) = open_store(true);
    let keyword = coordinator(Arc::new(FixtureSource), Arc::new(FixtureParser), None);
    assert_eq!(
        keyword.run_once(&store, 1).await.unwrap_err().code(),
        ErrorCode::ArtifactIntegrityError
    );
    let vector = coordinator(
        Arc::new(FixtureSource),
        Arc::new(FixtureParser),
        Some(Arc::new(FixtureEmbedder)),
    );
    assert_eq!(
        vector
            .run_until_idle(&store, 1, 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    let stop = AtomicBool::new(false);
    assert_eq!(
        vector
            .run_periodic(&store, &stop, Duration::ZERO, || 1)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
}

#[test]
fn failure_classification_retry_delay_and_provider_metrics_are_exhaustive() {
    assert_eq!(retry_delay(100, 1).unwrap(), 100);
    assert_eq!(retry_delay(100, 12).unwrap(), 102_400);
    assert_eq!(
        classify_provider(AiProviderError::RateLimited {
            retry_after_seconds: Some(3)
        }),
        Failure::Transient(Some(3))
    );
    assert_eq!(
        classify_provider(AiProviderError::MalformedResponse),
        Failure::Permanent
    );
    for (error, expected) in [
        (AiProviderError::InvalidRequest, AiProviderOutcome::Invalid),
        (
            AiProviderError::ContractMismatch,
            AiProviderOutcome::Invalid,
        ),
        (
            AiProviderError::Unauthorized,
            AiProviderOutcome::Unauthorized,
        ),
        (
            AiProviderError::RateLimited {
                retry_after_seconds: None,
            },
            AiProviderOutcome::RateLimited,
        ),
        (AiProviderError::Transient, AiProviderOutcome::Transient),
        (AiProviderError::Permanent, AiProviderOutcome::Permanent),
        (AiProviderError::Timeout, AiProviderOutcome::Timeout),
        (
            AiProviderError::MalformedResponse,
            AiProviderOutcome::Malformed,
        ),
    ] {
        assert_eq!(provider_outcome(error), expected);
    }
    assert_eq!(
        classify_platform(&PlatformError::new(
            ErrorCode::DocumentUnavailable,
            "fixture"
        )),
        Failure::Transient(None)
    );
    assert_eq!(
        classify_platform(&PlatformError::new(
            ErrorCode::DocumentProtocolError,
            "fixture"
        )),
        Failure::Permanent
    );
}
