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

mod upload_frame_is_streamed_to_private_exact_staging;

mod malformed_upload_removes_partial_staging;

mod response_shapes_are_bounded_and_cloudflare_facing;

mod upload_metadata_is_declared_string_input_and_materialized_by_schema;

mod streaming_chat_starts_with_retrieved_chunks_event;

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

mod upload_framing_rejects_invalid_lengths_headers_empty_files_and_unwritable_paths;

mod protocol_validators_cover_boundaries_headers_pagination_and_status_mapping;

mod strict_search_payloads_extract_only_unambiguous_user_queries;

mod metadata_materialization_enforces_declared_types_and_limits;

mod operation_metrics_and_empty_payload_validation_cover_all_categories;

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
        let now = unix_ms();
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

mod keyword_search_covers_filters_context_metadata_and_rejection_paths;

mod namespace_behavior_covers_list_federation_updates_stats_and_empty_delete;
