//! Focused coordinator recovery tests.

use super::*;
use open_compute_core::{
    AiEmbeddingMetric, AiTokenizer, ResolvedEmbeddingModelContract, ResolvedTokenizerContract,
};
use open_compute_document_parser::{DocumentFormat, DocumentMetadata, ParsedContentKind};
use open_compute_storage::{AiSearchInstanceStorageContract, NewAiSearchItemGeneration};
use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use uuid::Uuid;

fn contract_digest<T: Serialize>(contract: &T) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(contract).expect("serialize unsigned contract"),
    ))
}

fn model_contract(vector_enabled: bool) -> Vec<u8> {
    let digest = "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a";
    if vector_enabled {
        let mut contract = ResolvedEmbeddingModelContract {
            embedding_alias: "fixture/embed".to_owned(),
            backend_name: "fixture".to_owned(),
            backend_contract_sha256: digest.to_owned(),
            protocol: "openai_embeddings_v1".to_owned(),
            endpoint_sha256: digest.to_owned(),
            auth_kind: "none".to_owned(),
            auth_header_name: None,
            headers_sha256: digest.to_owned(),
            remote_model: "fixture".to_owned(),
            provider_revision: None,
            profile: "fixture/profile".to_owned(),
            profile_contract_sha256: digest.to_owned(),
            dimensions: 1,
            send_dimensions: false,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: 512,
            tokenizer: AiTokenizer::Custom,
            tokenizer_revision: "1".to_owned(),
            tokenizer_artifact_sha256: digest.to_owned(),
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = contract_digest(&contract);
        serde_json::to_vec(&contract).expect("serialize model contract")
    } else {
        let mut contract = ResolvedTokenizerContract {
            embedding_alias: "fixture/embed".to_owned(),
            profile: "fixture/profile".to_owned(),
            profile_contract_sha256: digest.to_owned(),
            tokenizer: AiTokenizer::Custom,
            tokenizer_revision: "1".to_owned(),
            tokenizer_artifact_sha256: digest.to_owned(),
            max_input_tokens: 512,
            contract_sha256: String::new(),
        };
        contract.contract_sha256 = contract_digest(&contract);
        serde_json::to_vec(&serde_json::json!({
            "kind": "keyword_only",
            "schemaVersion": 1,
            "tokenizerContract": contract,
        }))
        .expect("serialize tokenizer contract")
    }
}

fn open_store(vector_enabled: bool) -> (tempfile::TempDir, AiSearchStore) {
    open_store_modes(vector_enabled, true)
}

fn open_store_modes(
    vector_enabled: bool,
    keyword_enabled: bool,
) -> (tempfile::TempDir, AiSearchStore) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("instance.sqlite");
    let model = model_contract(vector_enabled);
    let public_config = if vector_enabled && keyword_enabled {
        br#"{"chunk":true,"chunk_overlap":2,"chunk_size":8,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#.as_slice()
    } else if vector_enabled {
        br#"{"chunk":true,"chunk_overlap":2,"chunk_size":8,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":false,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#.as_slice()
    } else {
        br#"{"chunk":true,"chunk_overlap":2,"chunk_size":8,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":false},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#.as_slice()
    };
    let store = AiSearchStore::open(
        &path,
        &AiSearchInstanceStorageContract {
            resource_id: "instance-1",
            model_contract_sha256: Sha256::digest(&model).into(),
            model_contract_json: &model,
            public_config_json: public_config,
            dimensions: u32::from(vector_enabled),
            vector_enabled,
            keyword_enabled,
        },
        1,
    )
    .expect("store");
    (directory, store)
}

fn enqueue_fixture(store: &AiSearchStore, job_id: &str) -> i64 {
    enqueue_fixture_generation(store, job_id, 1)
}

fn enqueue_fixture_generation(store: &AiSearchStore, job_id: &str, generation: u64) -> i64 {
    let now = current_time_ms();
    let index_generation = store.inspect().expect("inspection").active_index_generation;
    store
        .enqueue_item_generation(
            job_id,
            &NewAiSearchItemGeneration {
                item_id: "item-1",
                key: "fixture.txt",
                source: "builtin",
                generation,
                index_generation,
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
struct CountingSource(Arc<AtomicUsize>);

impl AiSearchSourceReader for CountingSource {
    fn read<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>> {
        Box::pin(async move {
            self.0.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(AiSearchSourceDocument {
                bytes: b"fixture source".to_vec(),
            })
        })
    }
}

#[derive(Debug)]
struct FixtureParser;

fn parsed_document(content: &str) -> AiSearchParsedDocument {
    AiSearchParsedDocument {
        content: content.to_owned(),
        markdown_sha256: hex::encode(Sha256::digest(content.as_bytes())),
        format: DocumentFormat::Text,
        detected_content_type: "text/plain".to_owned(),
        content_kind: ParsedContentKind::PlainText,
        page_count: None,
        sheet_count: None,
        sheet_names: None,
        metadata: DocumentMetadata::default(),
        warnings: Vec::new(),
        semantic_contract_sha256: hex::encode([6; 32]),
    }
}

impl AiSearchDocumentParser for FixtureParser {
    fn cache_contract_sha256(&self) -> [u8; 32] {
        [5; 32]
    }

    fn parse<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
        _: Vec<u8>,
    ) -> TaskFuture<'a, Result<AiSearchParsedDocument, PlatformError>> {
        Box::pin(async { Ok(parsed_document("alpha beta gamma delta")) })
    }
}

#[derive(Debug)]
struct CountingParser {
    calls: Arc<AtomicUsize>,
    contract: [u8; 32],
}

impl AiSearchDocumentParser for CountingParser {
    fn cache_contract_sha256(&self) -> [u8; 32] {
        self.contract
    }

    fn parse<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
        _: Vec<u8>,
    ) -> TaskFuture<'a, Result<AiSearchParsedDocument, PlatformError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            tokio::task::yield_now().await;
            Ok(parsed_document("alpha beta gamma delta"))
        })
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
    fn cache_contract_sha256(&self) -> [u8; 32] {
        [5; 32]
    }

    fn parse<'a>(
        &'a self,
        _: &'a AiSearchJobClaim,
        _: Vec<u8>,
    ) -> TaskFuture<'a, Result<AiSearchParsedDocument, PlatformError>> {
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
        AiSearchChunking {
            enabled: true,
            recursive: ChunkConfig {
                max_tokens: 8,
                overlap_tokens: 2,
            },
            max_input_tokens: 512,
        },
        60_000,
        100,
    )
    .expect("coordinator")
}

fn coordinator_with_chunking(vector: bool, chunking: AiSearchChunking) -> AiSearchCoordinator {
    AiSearchCoordinator::new(
        Arc::new(FixtureSource),
        Arc::new(FixtureParser),
        Arc::new(CharacterTokenizer),
        vector.then(|| Arc::new(FixtureEmbedder) as Arc<dyn AiSearchEmbedder>),
        chunking,
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
        AiSearchChunking {
            enabled: true,
            recursive: ChunkConfig {
                max_tokens: 8,
                overlap_tokens: 2,
            },
            max_input_tokens: 512,
        },
        60_000,
        100,
    )
    .expect("coordinator");
    let pass = coordinator
        .run_until_idle(&store, crashed.claim_until_ms, 4)
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
async fn chunk_false_preserves_one_exact_document_and_fails_closed_for_vector_limits() {
    let no_chunk = |max_input_tokens| AiSearchChunking {
        enabled: false,
        recursive: ChunkConfig {
            max_tokens: 8,
            overlap_tokens: 2,
        },
        max_input_tokens,
    };

    let (_directory, hybrid) = open_store(true);
    let now = enqueue_fixture(&hybrid, "job-hybrid-no-chunk");
    let pass = coordinator_with_chunking(true, no_chunk(512))
        .run_once(&hybrid, now)
        .await
        .unwrap();
    assert_eq!(pass.completed, 1);
    let (chunks, total) = hybrid.active_chunks(Some("item-1"), 0, 100).unwrap();
    assert_eq!(total, 1);
    assert_eq!(chunks[0].ordinal, 0);
    assert_eq!(chunks[0].start_byte, 0);
    assert_eq!(chunks[0].end_byte, 22);
    assert_eq!(chunks[0].text, "alpha beta gamma delta");

    let (_directory, restarted) = open_store(true);
    let now = enqueue_fixture(&restarted, "job-hybrid-no-chunk-restart");
    let crashed = restarted.claim_due_job(now, 1).unwrap().unwrap();
    let expected_id = stable_chunk_id(&crashed, &hex::encode([6; 32]), 0).unwrap();
    let pass = coordinator_with_chunking(true, no_chunk(512))
        .run_once(&restarted, crashed.claim_until_ms)
        .await
        .unwrap();
    assert_eq!(pass.completed, 1);
    let (chunks, total) = restarted.active_chunks(Some("item-1"), 0, 100).unwrap();
    assert_eq!(total, 1);
    assert_eq!(chunks[0].id, expected_id);
    assert_eq!(chunks[0].text, "alpha beta gamma delta");
    assert_eq!((chunks[0].start_byte, chunks[0].end_byte), (0, 22));

    let (_directory, keyword) = open_store(false);
    let now = enqueue_fixture(&keyword, "job-keyword-no-chunk");
    assert_eq!(
        coordinator_with_chunking(false, no_chunk(3))
            .run_once(&keyword, now)
            .await
            .unwrap()
            .completed,
        1
    );

    let (_directory, vector) = open_store_modes(true, false);
    let now = enqueue_fixture(&vector, "job-vector-no-chunk-limit");
    let pass = coordinator_with_chunking(true, no_chunk(3))
        .run_once(&vector, now)
        .await
        .unwrap();
    assert_eq!(pass.failed, 1);
    assert!(
        vector
            .active_chunks(Some("item-1"), 0, 100)
            .unwrap()
            .0
            .is_empty()
    );
    assert!(
        vector
            .item_logs("item-1", 0, 100)
            .unwrap()
            .iter()
            .any(|log| log.message_code == "EMBEDDING_INPUT_TOO_LARGE")
    );
}

#[tokio::test]
async fn durable_parse_cache_reuses_retry_and_reindex_and_contract_change_misses() {
    let (directory, store) = open_store(true);
    let cache_path = directory.path().join("parse-cache.sqlite");
    let cache = Arc::new(AiSearchParseCache::open(&cache_path, 1_000).expect("parse cache"));
    let locks = Arc::new(AiSearchParseCacheLocks::default());
    let cache_scope = ResourceId::generate();
    let reads = Arc::new(AtomicUsize::new(0));
    let parses = Arc::new(AtomicUsize::new(0));
    let build = |cache: Arc<AiSearchParseCache>, contract, embedder: Arc<dyn AiSearchEmbedder>| {
        AiSearchCoordinator::new(
            Arc::new(CountingSource(reads.clone())),
            Arc::new(CountingParser {
                calls: parses.clone(),
                contract,
            }),
            Arc::new(CharacterTokenizer),
            Some(embedder),
            AiSearchChunking {
                enabled: true,
                recursive: ChunkConfig {
                    max_tokens: 8,
                    overlap_tokens: 2,
                },
                max_input_tokens: 512,
            },
            60_000,
            100,
        )
        .expect("coordinator")
        .with_parse_cache(cache_scope, cache.clone(), locks.clone())
    };

    let now = enqueue_fixture_generation(&store, "cache-job-1", 1);
    assert_eq!(
        build(
            cache.clone(),
            [5; 32],
            Arc::new(FailingEmbedder(EmbeddingFailure::RateLimited)),
        )
        .run_once(&store, now)
        .await
        .unwrap()
        .retried,
        1
    );
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(parses.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(
        build(cache.clone(), [5; 32], Arc::new(FixtureEmbedder))
            .run_once(&store, current_time_ms().saturating_add(3_000))
            .await
            .unwrap()
            .completed,
        1
    );
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(parses.load(AtomicOrdering::Relaxed), 1);
    drop(cache);
    let reopened = Arc::new(AiSearchParseCache::open(&cache_path, 1_000).expect("reopen cache"));
    let model = model_contract(true);
    let public = br#"{"chunk":true,"chunk_overlap":2,"chunk_size":8,"custom_metadata":[],"fusion_method":"rrf","index_method":{"keyword":true,"vector":true},"max_num_results":10,"metadata":{},"score_threshold":0.4}"#;
    let now = current_time_ms();
    assert!(
        store
            .begin_full_reindex(
                1,
                &AiSearchInstanceStorageContract {
                    resource_id: "instance-1",
                    model_contract_sha256: Sha256::digest(&model).into(),
                    model_contract_json: &model,
                    public_config_json: public,
                    dimensions: 1,
                    vector_enabled: true,
                    keyword_enabled: true,
                },
                "cache-reindex",
                now,
            )
            .unwrap()
    );
    assert_eq!(
        build(reopened.clone(), [5; 32], Arc::new(FixtureEmbedder))
            .run_once(&store, now)
            .await
            .unwrap()
            .completed,
        1
    );
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(parses.load(AtomicOrdering::Relaxed), 1);

    let key = open_compute_storage::AiSearchParseCacheKey::new(
        [7; 32],
        14,
        "fixture.txt",
        "text/plain",
        [5; 32],
    )
    .unwrap();
    rusqlite::Connection::open(&cache_path)
        .unwrap()
        .execute(
            "UPDATE parse_cache SET payload=X'00' WHERE cache_key=?1",
            [key.digest().as_slice()],
        )
        .unwrap();
    let now = enqueue_fixture_generation(&store, "cache-job-3", 3);
    assert_eq!(
        build(reopened.clone(), [5; 32], Arc::new(FixtureEmbedder))
            .run_once(&store, now)
            .await
            .unwrap()
            .completed,
        1
    );
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 2);
    assert_eq!(parses.load(AtomicOrdering::Relaxed), 2);

    let now = enqueue_fixture_generation(&store, "cache-job-4", 4);
    assert_eq!(
        build(reopened, [9; 32], Arc::new(FixtureEmbedder))
            .run_once(&store, now)
            .await
            .unwrap()
            .completed,
        1
    );
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 3);
    assert_eq!(parses.load(AtomicOrdering::Relaxed), 3);
}

#[tokio::test]
async fn parse_cache_singleflight_deduplicates_concurrent_identical_generations() {
    let (directory, store) = open_store(true);
    let cache = Arc::new(
        AiSearchParseCache::open(&directory.path().join("parse-cache.sqlite"), 1_000)
            .expect("parse cache"),
    );
    let locks = Arc::new(AiSearchParseCacheLocks::default());
    let cache_scope = ResourceId::generate();
    let reads = Arc::new(AtomicUsize::new(0));
    let parses = Arc::new(AtomicUsize::new(0));
    let coordinator = Arc::new(
        AiSearchCoordinator::new(
            Arc::new(CountingSource(reads.clone())),
            Arc::new(CountingParser {
                calls: parses.clone(),
                contract: [5; 32],
            }),
            Arc::new(CharacterTokenizer),
            Some(Arc::new(FixtureEmbedder)),
            AiSearchChunking {
                enabled: true,
                recursive: ChunkConfig {
                    max_tokens: 8,
                    overlap_tokens: 2,
                },
                max_input_tokens: 512,
            },
            60_000,
            100,
        )
        .expect("coordinator")
        .with_parse_cache(cache_scope, cache, locks),
    );
    let now = enqueue_fixture_generation(&store, "concurrent-cache-1", 1);
    enqueue_fixture_generation(&store, "concurrent-cache-2", 2);
    let (first, second) = tokio::join!(
        coordinator.run_once(&store, now),
        coordinator.run_once(&store, now)
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 1);
    assert_eq!(parses.load(AtomicOrdering::Relaxed), 1);
}

#[tokio::test]
async fn transient_parser_failures_are_not_cached_as_success() {
    let (directory, store) = open_store(true);
    let cache = Arc::new(
        AiSearchParseCache::open(&directory.path().join("parse-cache.sqlite"), 1_000)
            .expect("parse cache"),
    );
    let locks = Arc::new(AiSearchParseCacheLocks::default());
    let cache_scope = ResourceId::generate();
    let reads = Arc::new(AtomicUsize::new(0));
    let make = |parser: Arc<dyn AiSearchDocumentParser>| {
        AiSearchCoordinator::new(
            Arc::new(CountingSource(reads.clone())),
            parser,
            Arc::new(CharacterTokenizer),
            Some(Arc::new(FixtureEmbedder)),
            AiSearchChunking {
                enabled: true,
                recursive: ChunkConfig {
                    max_tokens: 8,
                    overlap_tokens: 2,
                },
                max_input_tokens: 512,
            },
            60_000,
            100,
        )
        .expect("coordinator")
        .with_parse_cache(cache_scope, cache.clone(), locks.clone())
    };
    let now = enqueue_fixture(&store, "failed-parse-cache");
    assert_eq!(
        make(Arc::new(FailingParser(ErrorCode::DocumentTimeout)))
            .run_once(&store, now)
            .await
            .unwrap()
            .retried,
        1
    );
    assert_eq!(
        make(Arc::new(FixtureParser))
            .run_once(&store, current_time_ms().saturating_add(1_000))
            .await
            .unwrap()
            .completed,
        1
    );
    assert_eq!(reads.load(AtomicOrdering::Relaxed), 2);
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
            AiSearchChunking {
                enabled: true,
                recursive: ChunkConfig {
                    max_tokens: 8,
                    overlap_tokens: 2,
                },
                max_input_tokens: 512,
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
