use super::chat::chunks_sse_event;
use super::ingest::materialize_upload_metadata;
use super::*;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::cloudflare_v4::{router as v4_router, storage_router};
use crate::health::HealthCoordinator;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::metrics::MetricsRegistry;
use crate::p3_3_test_support::RuntimeFeatureFixture;
use crate::search_api::SearchApiState;
use crate::snapshot_pins::SnapshotPins;
use axum::body::to_bytes;
use axum::http::{Request as HttpRequest, StatusCode};
use futures::stream;
use open_compute_artifacts::{
    AiSearchObjectStore, MapEnv, MockS3, ObjectBackend, resolve_s3_credentials_with,
};
use open_compute_core::config::MetricsConfig;
use open_compute_core::{
    AiAuthConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiProviderConfig, AiTokenizer,
    AiTokenizerArtifactConfig, DocumentParserConfig, PlatformConfig, SecretString,
};
use open_compute_storage::AiSearchObjectReference;
use open_compute_storage::{ResourceRecord, StagedAiSearchChunk};
use open_compute_workers::{AiSearchNamespaceResourceDriver, CreateResourceOutcome, ResourcePins};
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::time::Duration;
use tower::ServiceExt as _;

#[tokio::test]
async fn upload_frame_is_streamed_to_private_exact_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("upload");
    let metadata = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "instance": "docs",
        "name": "guide.txt",
        "contentType": "text/plain",
        "options": {"metadata": {"language": "en"}},
    }))
    .unwrap();
    let mut frame = u32::try_from(metadata.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    frame.extend_from_slice(&metadata);
    frame.extend_from_slice(b"hello streamed world");
    let pieces = frame
        .chunks(3)
        .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let body = Body::from_stream(stream::iter(pieces));
    let staged = stage_upload(body, path.clone()).await.unwrap();
    assert_eq!(staged.header.instance.as_deref(), Some("docs"));
    assert_eq!(staged.header.name, "guide.txt");
    assert_eq!(staged.size, 20);
    let expected_digest: [u8; 32] = Sha256::digest(b"hello streamed world").into();
    assert_eq!(staged.digest, expected_digest);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello streamed world");
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn malformed_upload_removes_partial_staging() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("upload");
    let body = Body::from(Bytes::from_static(&[0, 1, 0, 0]));
    assert_eq!(
        stage_upload(body, path.clone()).await.unwrap_err().code(),
        ErrorCode::BindingProtocolError
    );
    assert!(!path.exists());
}

#[test]
fn response_shapes_are_bounded_and_cloudflare_facing() {
    let item = AiSearchItemRecord {
        id: "item".to_owned(),
        key: "guide.txt".to_owned(),
        status: "completed".to_owned(),
        active_generation: Some(1),
        desired_generation: 1,
        metadata_json: br#"{"language":"en"}"#.to_vec(),
        created_at_ms: 10,
        updated_at_ms: 20,
        object: AiSearchObjectReference {
            object_key: "system/ai-search/object".to_owned(),
            object_sha256: [7; 32],
            object_size: 5,
        },
        content_type: "text/plain".to_owned(),
        chunks_count: 2,
    };
    let value = item_info_value(&item).unwrap();
    assert_eq!(value["status"], "completed");
    assert_eq!(value["metadata"]["language"], "en");
    assert_eq!(page_bounds(Some(2), Some(10), 15).unwrap(), (2, 10, 10, 15));
    assert_eq!(
        metric_operation("namespace.search"),
        AiSearchOperation::Search
    );
}

#[test]
fn upload_metadata_is_declared_string_input_and_materialized_by_schema() {
    let config: ResolvedAiSearchConfig = serde_json::from_value(json!({
        "id": "docs",
        "rewrite_query": false,
        "reranking": false,
        "embedding_model": "@cf/qwen/qwen3-embedding-0.6b",
        "index_method": {"vector": true, "keyword": true},
        "fusion_method": "rrf",
        "indexing_options": {"keyword_tokenizer": "porter"},
        "retrieval_options": {"keyword_match_mode": "and"},
        "chunk": true,
        "chunk_size": 64,
        "chunk_overlap": 0,
        "score_threshold": 0.4,
        "max_num_results": 10,
        "custom_metadata": [
            {"field_name": "rank", "data_type": "number"},
            {"field_name": "published", "data_type": "boolean"},
            {"field_name": "at", "data_type": "datetime"}
        ],
        "metadata": {}
    }))
    .unwrap();
    let input = json!({
        "rank": "2.5",
        "published": "true",
        "at": "2026-09-02T03:04:05Z"
    })
    .as_object()
    .unwrap()
    .clone();
    let value: Value =
        serde_json::from_slice(&materialize_upload_metadata(&config, &input).unwrap()).unwrap();
    assert_eq!(value["rank"], 2.5);
    assert_eq!(value["published"], true);
    assert_eq!(value["at"], "2026-09-02T03:04:05Z");

    let typed = json!({"rank": 2}).as_object().unwrap().clone();
    assert!(materialize_upload_metadata(&config, &typed).is_err());
    let undeclared = json!({"language": "en"}).as_object().unwrap().clone();
    assert!(materialize_upload_metadata(&config, &undeclared).is_err());
}

#[test]
fn streaming_chat_starts_with_retrieved_chunks_event() {
    let event = chunks_sse_event(&json!([{"id": "chunk-1", "score": 0.9}])).unwrap();
    assert_eq!(
        event,
        Bytes::from_static(b"event: chunks\ndata: [{\"id\":\"chunk-1\",\"score\":0.9}]\n\n")
    );
}

fn upload_frame(header: &Value, body: &[u8]) -> Vec<u8> {
    let metadata = serde_json::to_vec(header).unwrap();
    let mut frame = u32::try_from(metadata.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    frame.extend_from_slice(&metadata);
    frame.extend_from_slice(body);
    frame
}

#[tokio::test]
async fn upload_framing_rejects_invalid_lengths_headers_empty_files_and_unwritable_paths() {
    let valid = json!({
        "schemaVersion": 1,
        "name": "guide.txt",
        "contentType": "text/plain",
        "options": {},
    });
    for (name, bytes, code) in [
        (
            "zero-header",
            0_u32.to_be_bytes().to_vec(),
            ErrorCode::BindingLimitExceeded,
        ),
        (
            "oversized-header",
            u32::try_from(MAX_FRAME_METADATA_BYTES + 1)
                .unwrap()
                .to_be_bytes()
                .to_vec(),
            ErrorCode::BindingLimitExceeded,
        ),
        (
            "invalid-json",
            upload_frame(&Value::String("not an object".to_owned()), b"body"),
            ErrorCode::BindingProtocolError,
        ),
        (
            "wrong-version",
            upload_frame(
                &json!({
                    "schemaVersion": 2,
                    "name": "guide.txt",
                    "contentType": "text/plain",
                    "options": {},
                }),
                b"body",
            ),
            ErrorCode::BindingProtocolError,
        ),
        (
            "empty-body",
            upload_frame(&valid, b""),
            ErrorCode::BindingLimitExceeded,
        ),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join(name);
        let error = stage_upload(Body::from(bytes), path.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code(), code);
        assert!(!path.exists());
    }

    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("missing-parent/upload");
    let error = stage_upload(Body::from(upload_frame(&valid, b"body")), path.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceUnavailable);
    assert!(!path.exists());
}

#[test]
fn protocol_validators_cover_boundaries_headers_pagination_and_status_mapping() {
    assert!(validate_source("a.txt", "text/plain", 1).is_ok());
    for (name, content_type, size) in [
        ("", "text/plain", 1),
        ("line\nbreak", "text/plain", 1),
        ("a.txt", "", 1),
        ("a.txt", "text/plain\nforged", 1),
        ("a.txt", "text/plain", 0),
        ("a.txt", "text/plain", MAX_UPLOAD_BYTES as u64 + 1),
    ] {
        assert_eq!(
            validate_source(name, content_type, size)
                .unwrap_err()
                .code(),
            ErrorCode::BindingLimitExceeded
        );
    }

    assert_eq!(page_bounds(None, None, 3).unwrap(), (1, 50, 0, 3));
    assert_eq!(page_bounds(Some(9), Some(10), 3).unwrap(), (9, 10, 3, 3));
    assert_eq!(page_bounds(Some(1), Some(0), 3).unwrap(), (1, 0, 0, 0));
    for (page, per_page) in [(Some(0), Some(1)), (None, Some(101))] {
        assert_eq!(
            page_bounds(page, per_page, 3).unwrap_err().code(),
            ErrorCode::BindingProtocolError
        );
    }
    assert_eq!(
        pagination(2, 3, 10, 22),
        json!({
            "count": 2,
            "page": 3,
            "per_page": 10,
            "total_count": 22,
        })
    );

    let mut headers = HeaderMap::new();
    headers.insert("x-number", "42".parse().unwrap());
    headers.insert("x-digest", hex::encode([3_u8; 32]).parse().unwrap());
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert_eq!(parse_header::<u64>(&headers, "x-number").unwrap(), 42);
    assert_eq!(parse_digest(&headers, "x-digest").unwrap(), [3; 32]);
    assert!(content_type_is(&headers, "application/json"));
    assert!(!content_type_is(
        &headers,
        "application/json; charset=utf-8"
    ));
    assert!(header_text(&headers, "missing").is_err());
    headers.insert("x-number", "bad".parse().unwrap());
    assert!(parse_header::<u64>(&headers, "x-number").is_err());
    headers.insert("x-digest", "00".parse().unwrap());
    assert!(parse_digest(&headers, "x-digest").is_err());

    for (code, status) in [
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (
            ErrorCode::BindingLimitExceeded,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            ErrorCode::ResourceUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST),
    ] {
        let error = PlatformError::new(code, "fixture");
        let response = error_response(&error);
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()["x-open-compute-error-code"],
            code.as_str()
        );
    }
}

#[test]
fn strict_search_payloads_extract_only_unambiguous_user_queries() {
    let direct: SearchPayload = serde_json::from_value(json!({"query": "needle"})).unwrap();
    assert_eq!(direct.query_text().unwrap(), "needle");

    let messages: SearchPayload = serde_json::from_value(json!({
        "messages": [
            {"role": "user", "content": "old"},
            {"role": "assistant", "content": "answer"},
            {"role": "user", "content": "new"}
        ]
    }))
    .unwrap();
    assert_eq!(messages.query_text().unwrap(), "new");
    for payload in [
        json!({}),
        json!({"query": ""}),
        json!({"query": "x", "messages": []}),
        json!({"messages": [{"role": "assistant", "content": "none"}]}),
        json!({"messages": [{"role": "user", "content": ""}]}),
    ] {
        let payload: SearchPayload = serde_json::from_value(payload).unwrap();
        assert_eq!(
            payload.query_text().unwrap_err().code(),
            ErrorCode::BindingProtocolError
        );
    }
    assert!(
        serde_json::from_value::<SearchPayload>(json!({
            "query": "x",
            "unexpected": true
        }))
        .is_err()
    );

    let chat: ChatPayload = serde_json::from_value(json!({
        "messages": [{"role": "user", "content": "chat query"}],
        "stream": true
    }))
    .unwrap();
    assert_eq!(chat.as_search().query_text().unwrap(), "chat query");
}

#[test]
fn metadata_materialization_enforces_declared_types_and_limits() {
    let config: ResolvedAiSearchConfig = serde_json::from_value(json!({
        "id": "docs",
        "rewrite_query": false,
        "reranking": false,
        "embedding_model": "@cf/qwen/qwen3-embedding-0.6b",
        "index_method": {"vector": true, "keyword": true},
        "fusion_method": "rrf",
        "indexing_options": {"keyword_tokenizer": "porter"},
        "retrieval_options": {"keyword_match_mode": "and"},
        "chunk": true,
        "chunk_size": 64,
        "chunk_overlap": 0,
        "score_threshold": 0.4,
        "max_num_results": 10,
        "custom_metadata": [
            {"field_name": "text", "data_type": "text"},
            {"field_name": "number", "data_type": "number"},
            {"field_name": "boolean", "data_type": "boolean"},
            {"field_name": "datetime", "data_type": "datetime"}
        ],
        "metadata": {}
    }))
    .unwrap();
    let valid = json!({
        "text": "hello",
        "number": "-1.25",
        "boolean": "false",
        "datetime": "2026-09-02T03:04:05Z"
    });
    let canonical = materialize_upload_metadata(&config, valid.as_object().unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&canonical).unwrap(),
        json!({
            "boolean": false,
            "datetime": "2026-09-02T03:04:05Z",
            "number": -1.25,
            "text": "hello"
        })
    );
    for invalid in [
        json!({"number": "nan"}),
        json!({"boolean": "TRUE"}),
        json!({"datetime": "tomorrow"}),
        json!({"unknown": "value"}),
        json!({"text": 1}),
    ] {
        assert_eq!(
            materialize_upload_metadata(&config, invalid.as_object().unwrap())
                .unwrap_err()
                .code(),
            ErrorCode::BindingProtocolError
        );
    }
    let too_many = json!({"a":"1","b":"2","c":"3","d":"4","e":"5","f":"6"});
    assert_eq!(
        materialize_upload_metadata(&config, too_many.as_object().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
}

#[test]
fn operation_metrics_and_empty_payload_validation_cover_all_categories() {
    for (operation, expected) in [
        ("namespace.search", AiSearchOperation::Search),
        ("instance.search", AiSearchOperation::Search),
        ("namespace.chatCompletions", AiSearchOperation::Chat),
        ("instance.chatCompletions", AiSearchOperation::Chat),
        ("namespace.list", AiSearchOperation::Namespace),
        ("instance.info", AiSearchOperation::Instance),
        ("item.info", AiSearchOperation::Item),
        ("jobs.list", AiSearchOperation::Job),
    ] {
        assert_eq!(metric_operation(operation), expected);
    }
    assert!(require_empty_object(&json!({})).is_ok());
    assert!(require_empty_object(&Value::Null).is_err());
    assert!(require_empty_object(&json!([])).is_err());
    assert!(unix_ms().unwrap() > 0);
}

#[path = "ai_search_backend/official_v4_tests.rs"]
mod official_v4_tests;
struct SearchBehaviorFixture {
    _runtime: RuntimeFeatureFixture,
    service: Arc<AiSearchBindingService>,
    pins: ResourcePins,
    namespace: ResourceRecord,
}

impl SearchBehaviorFixture {
    async fn create() -> Self {
        let runtime =
            RuntimeFeatureFixture::create(open_compute_workers::VersionRuntimeFeatures::default())
                .await;
        let pins = ResourcePins::new();
        let namespace_id = match ResourceController::new(
            &runtime.storage,
            pins.clone(),
            AiSearchNamespaceResourceDriver::new(&runtime.storage),
        )
        .create(&CreateResourceRequest {
            account_id: runtime.account,
            kind: BindingKind::AiSearchNamespace,
            name: "search-behavior".to_owned(),
            idempotency_key: "search-behavior-namespace".to_owned(),
            driver_schema_version: open_compute_storage::AI_SEARCH_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 10,
        })
        .unwrap()
        {
            CreateResourceOutcome::Applied(result) => result.resource_id,
            CreateResourceOutcome::Replay(_) => panic!("first namespace create replayed"),
        };
        let namespace = ResourceRepository::new(runtime.storage.db())
            .get(runtime.account, namespace_id)
            .unwrap();
        let ai = keyword_ai_config();
        let objects = ai_search_objects(&runtime._mock);
        let parser = Arc::new(DocumentParserBindingService::with_executable(
            runtime.storage.clone(),
            DocumentParserConfig::default(),
            PathBuf::from("/usr/bin/false"),
        ));
        let service = Arc::new(
            AiSearchBindingService::new(
                runtime.storage.clone(),
                pins.clone(),
                ai,
                objects,
                Arc::new(SnapshotPins::empty()),
                parser,
            )
            .unwrap(),
        );
        Self {
            _runtime: runtime,
            service,
            pins,
            namespace,
        }
    }

    fn authority(&self, resource: ResourceRecord, kind: BindingKind) -> Authority {
        let resource_id = resource.id;
        Authority {
            account_id: self._runtime.account,
            kind,
            resource,
            read: true,
            write: true,
            request_id: RequestId::generate(),
            _bound_pin: self.pins.try_pin(resource_id).unwrap(),
        }
    }

    fn namespace_authority(&self) -> Authority {
        self.authority(self.namespace.clone(), BindingKind::AiSearchNamespace)
    }

    fn create_instance(&self, id: &str) -> AiSearchInstanceRecord {
        self.create_instance_with_vector(id, false)
    }

    fn create_instance_with_vector(
        &self,
        id: &str,
        vector_enabled: bool,
    ) -> AiSearchInstanceRecord {
        self.service
            .namespace_create(
                &self.namespace_authority(),
                JsonCall {
                    operation: "namespace.create".to_owned(),
                    instance: None,
                    payload: json!({
                        "id": id,
                        "embedding_model": "@cf/qwen/qwen3-embedding-0.6b",
                        "index_method": {"vector": vector_enabled, "keyword": true},
                        "indexing_options": {"keyword_tokenizer": "porter"},
                        "retrieval_options": {"keyword_match_mode": "and"},
                        "chunk_size": 32,
                        "chunk_overlap": 0,
                        "score_threshold": 0.0,
                        "max_num_results": 10,
                        "custom_metadata": [
                            {"field_name": "category", "data_type": "text"},
                            {"field_name": "rank", "data_type": "number"}
                        ]
                    }),
                },
            )
            .unwrap();
        AiSearchCatalog::new(self._runtime.storage.db())
            .get_instance_by_key(self._runtime.account, self.namespace.id, id)
            .unwrap()
    }

    fn seed_item(
        &self,
        record: &AiSearchInstanceRecord,
        item_id: &str,
        key: &str,
        metadata: &[u8],
        chunks: &[(&str, &str)],
    ) {
        let (store, inspection) = self.service.open_store(record).unwrap();
        let now = unix_ms().unwrap();
        let object_digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        store
            .enqueue_item_generation(
                &format!("job-{item_id}"),
                &NewAiSearchItemGeneration {
                    item_id,
                    key,
                    source: "builtin",
                    generation: 1,
                    index_generation: inspection.active_index_generation,
                    object_key: &format!("system/test/{item_id}"),
                    object_sha256: object_digest,
                    object_size: 1,
                    content_type: "text/plain",
                    metadata_json: metadata,
                    now_ms: now,
                },
            )
            .unwrap();
        let claim = store.claim_due_job(now, 60_000).unwrap().unwrap();
        let staged = chunks
            .iter()
            .enumerate()
            .map(|(ordinal, (id, text))| StagedAiSearchChunk {
                chunk_id: id,
                ordinal: u32::try_from(ordinal).unwrap(),
                start_byte: 0,
                end_byte: u64::try_from(text.len()).unwrap(),
                text,
                embedding_f32le: None,
                vector_norm: None,
                metadata_json: metadata,
            })
            .collect::<Vec<_>>();
        assert!(
            store
                .activate_item_generation(&claim, item_id, 1, &staged, now + 1)
                .unwrap()
        );
    }
}

fn keyword_ai_config() -> AiConfig {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tokenizer-word-level.json");
    let bytes = std::fs::read(&path).unwrap();
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
            dimensions: 1_024,
            request_dimensions: None,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: 8_192,
            tokenizer: AiTokenizer::Qwen3,
            tokenizer_revision: "fixture-tokenizer".to_owned(),
            tokenizer_artifact: AiTokenizerArtifactConfig {
                path,
                sha256: hex::encode(Sha256::digest(bytes)),
            },
        },
    );
    config.default_embedding_model = Some(alias.to_owned());
    config
}

fn ai_search_objects(mock: &MockS3) -> AiSearchObjectStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 3000
"#,
        mock.endpoint
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    AiSearchObjectStore::new(
        ObjectBackend::connect_s3(&config, &credentials, 32 * 1024 * 1024).unwrap(),
    )
}

fn search_call(instance: Option<&str>, payload: Value) -> JsonCall {
    JsonCall {
        operation: "instance.search".to_owned(),
        instance: instance.map(str::to_owned),
        payload,
    }
}

#[tokio::test]
async fn keyword_search_covers_filters_context_metadata_and_rejection_paths() {
    let fixture = SearchBehaviorFixture::create().await;
    let record = fixture.create_instance("docs");
    fixture.seed_item(
        &record,
        "guide",
        "guide.txt",
        br#"{"category":"guide","rank":2}"#,
        &[
            ("guide-0", "alpha beta first"),
            ("guide-1", "neighbor context"),
            ("guide-2", "alpha second"),
        ],
    );
    fixture.seed_item(
        &record,
        "note",
        "note.txt",
        br#"{"category":"note","rank":1}"#,
        &[("note-0", "alpha beta note")],
    );
    let authority = fixture.authority(record.resource.clone(), BindingKind::AiSearchInstance);

    let result = fixture
        .service
        .instance_search(
            &authority,
            search_call(
                None,
                json!({
                    "query": "alpha beta",
                    "ai_search_options": {"retrieval": {
                        "retrieval_type": "keyword",
                        "keyword_match_mode": "or",
                        "filters": {"category": "guide"},
                        "context_expansion": 1,
                        "max_num_results": 5,
                        "match_threshold": 0.0
                    }}
                }),
            ),
        )
        .await
        .unwrap();
    let chunks = result["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 2);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk["item"]["metadata"]["category"] == "guide")
    );
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk["text"].as_str().unwrap().contains("neighbor context"))
    );

    let metadata_only = fixture
        .service
        .instance_search(
            &authority,
            search_call(
                None,
                json!({
                    "messages": [{"role":"user","content":"alpha"}],
                    "ai_search_options": {"retrieval": {
                        "retrieval_type": "keyword",
                        "metadata_only": true,
                        "filters": {"rank": {"$gte": 2}}
                    }}
                }),
            ),
        )
        .await
        .unwrap();
    assert!(
        metadata_only["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|chunk| chunk["text"] == "")
    );

    for payload in [
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"vector"}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"unknown"}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","keyword_match_mode":"xor"}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","boost_by":{}}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","context_expansion":4}}}),
        json!({"query":"alpha","ai_search_options":{"retrieval":{"retrieval_type":"keyword","filters":{"undeclared":"x"}}}}),
        json!({"query":"","ai_search_options":{}}),
    ] {
        assert!(
            fixture
                .service
                .instance_search(&authority, search_call(None, payload))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn namespace_behavior_covers_list_federation_updates_stats_and_empty_delete() {
    let fixture = SearchBehaviorFixture::create().await;
    let docs = fixture.create_instance("docs");
    let archive = fixture.create_instance("archive");
    let _disposable = fixture.create_instance("disposable");
    fixture.seed_item(
        &docs,
        "docs-item",
        "docs.txt",
        br#"{"category":"guide","rank":2}"#,
        &[("docs-0", "alpha docs")],
    );
    fixture.seed_item(
        &archive,
        "archive-item",
        "archive.txt",
        br#"{"category":"note","rank":1}"#,
        &[("archive-0", "alpha archive")],
    );
    let namespace = fixture.namespace_authority();

    let listed = fixture
        .service
        .namespace_list(
            &namespace,
            JsonCall {
                operation: "namespace.list".to_owned(),
                instance: None,
                payload: json!({
                    "page": 1,
                    "per_page": 2,
                    "search": "a",
                    "order_by": "created_at",
                    "order_by_direction": "desc"
                }),
            },
        )
        .unwrap();
    assert_eq!(listed["result"].as_array().unwrap().len(), 2);
    assert_eq!(listed["result_info"]["total_count"], 2);

    for payload in [
        json!({"order_by":"name"}),
        json!({"order_by_direction":"sideways"}),
        json!({"page":0}),
    ] {
        assert!(
            fixture
                .service
                .namespace_list(
                    &namespace,
                    JsonCall {
                        operation: "namespace.list".to_owned(),
                        instance: None,
                        payload,
                    },
                )
                .is_err()
        );
    }

    let federated = fixture
        .service
        .namespace_search(
            &namespace,
            JsonCall {
                operation: "namespace.search".to_owned(),
                instance: None,
                payload: json!({
                    "query":"alpha",
                    "ai_search_options": {
                        "instance_ids":["docs","archive","missing"],
                        "retrieval":{"retrieval_type":"keyword","return_on_failure":true}
                    }
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(federated["chunks"].as_array().unwrap().len(), 2);
    assert_eq!(federated["errors"][0]["instance_id"], "missing");
    assert!(
        federated["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|chunk| chunk.get("instance_id").is_some())
    );

    for payload in [
        json!({"query":"alpha","ai_search_options":{"instance_ids":[]}}),
        json!({"query":"alpha","ai_search_options":{"instance_ids":["docs","docs"]}}),
        json!({"query":"alpha","ai_search_options":{"instance_ids":["missing"],"retrieval":{"return_on_failure":false}}}),
    ] {
        assert!(
            fixture
                .service
                .namespace_search(
                    &namespace,
                    JsonCall {
                        operation: "namespace.search".to_owned(),
                        instance: None,
                        payload,
                    },
                )
                .await
                .is_err()
        );
    }

    let docs_authority = fixture.authority(docs.resource.clone(), BindingKind::AiSearchInstance);
    let info = fixture
        .service
        .instance_info_call(
            &docs_authority,
            &JsonCall {
                operation: "instance.info".to_owned(),
                instance: None,
                payload: json!({}),
            },
        )
        .unwrap();
    assert_eq!(info["status"], "ready");
    let stats = fixture
        .service
        .instance_stats(
            &docs_authority,
            &JsonCall {
                operation: "instance.stats".to_owned(),
                instance: None,
                payload: json!({}),
            },
        )
        .unwrap();
    assert_eq!(stats["completed"], 1);
    assert_eq!(stats["engine"]["chunks"], 1);
    let updatable = fixture.create_instance_with_vector("updatable", true);
    let updatable_authority =
        fixture.authority(updatable.resource.clone(), BindingKind::AiSearchInstance);
    let updated = fixture
        .service
        .instance_update(
            &updatable_authority,
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: None,
                payload: json!({"metadata":{"updated":true}}),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated["metadata"]["updated"], true);

    let reindexed = fixture
        .service
        .instance_update(
            &updatable_authority,
            JsonCall {
                operation: "instance.update".to_owned(),
                instance: None,
                payload: json!({"chunk_size":16}),
            },
        )
        .await
        .unwrap();
    assert_eq!(reindexed["chunk_size"], 16);
    drop(updatable_authority);
    let deleted = fixture
        .service
        .namespace_delete(
            &namespace,
            JsonCall {
                operation: "namespace.delete".to_owned(),
                instance: None,
                payload: json!({"instance":"disposable"}),
            },
        )
        .await
        .unwrap();
    assert_eq!(deleted, Value::Null);
}
