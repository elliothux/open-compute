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
    AiSearchObjectStore, MapEnv, MockS3, ObjectBackend, R2HttpMetadata, R2ObjectStore,
    R2PutOptions, R2UploadSource, UserObjectKey, hash_bytes, resolve_s3_credentials_with,
};
use open_compute_core::config::MetricsConfig;
use open_compute_core::{
    AiAuthConfig, AiBackendConfig, AiBackendProtocol, AiEmbeddingModelConfig,
    AiEmbeddingProfileConfig, AiTokenizer, AiTokenizerArtifactConfig, AiTokenizerConfig,
    DocumentParserConfig, PlatformConfig, R2Config, SecretString,
};
use open_compute_storage::{
    AiSearchObjectReference, R2BucketRepository, R2ObjectRecord, R2ObjectRepository,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRecord, StagedAiSearchChunk,
};
use open_compute_workers::{
    AiSearchNamespaceResourceDriver, CreateResourceOutcome, R2ResourceDriver, ResourcePins,
};
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
mod r2_source_behavior;
struct SearchBehaviorFixture {
    _runtime: RuntimeFeatureFixture,
    service: Arc<AiSearchBindingService>,
    pins: ResourcePins,
    namespace: ResourceRecord,
}

impl SearchBehaviorFixture {
    async fn create() -> Self {
        Self::create_with_parser(PathBuf::from("/usr/bin/false")).await
    }

    async fn create_with_r2() -> Self {
        let executable = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("ocd");
        assert!(executable.is_file(), "missing test ocd at {executable:?}");
        Self::create_with_parser(executable).await
    }

    async fn create_with_parser(parser_executable: PathBuf) -> Self {
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
            driver_schema_version: open_compute_storage::AI_SEARCH_NAMESPACE_SCHEMA_VERSION,
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
        let parser = Arc::new(
            DocumentParserBindingService::with_executable(
                runtime.storage.clone(),
                DocumentParserConfig::default(),
                &AiConfig::default(),
                parser_executable,
            )
            .unwrap(),
        );
        let r2_objects = r2_objects(&runtime._mock);
        let r2_config = R2Config {
            max_object_bytes: 4 * 1024 * 1024,
            operation_timeout_ms: 3_000,
            ..R2Config::default()
        };
        let service = Arc::new(
            AiSearchBindingService::new(
                runtime.storage.clone(),
                pins.clone(),
                ai,
                objects,
                Arc::new(SnapshotPins::empty()),
                parser,
            )
            .unwrap()
            .with_r2_source_backing(r2_objects, r2_config),
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

    async fn create_r2_bucket(&self, name: &str) -> ResourceRecord {
        let resource_id = ResourceId::generate();
        let fingerprint = self.storage().crypto().fingerprint_request(name.as_bytes());
        let reservation = ResourceRepository::new(self.storage().db())
            .reserve_create(
                &ReserveResourceCreate {
                    account_id: self._runtime.account,
                    kind: BindingKind::R2Bucket,
                    name,
                    idempotency_key: name,
                    fingerprint_key_id: self.storage().crypto().fingerprint_key_id(),
                    request_fingerprint: &fingerprint,
                    resource_id,
                    driver_schema_version: open_compute_storage::R2_SCHEMA_VERSION,
                    request_id: RequestId::generate(),
                    now_ms: 20,
                    expires_at_ms: 1_000,
                },
                1_000_000,
            )
            .unwrap();
        let ResourceCreateReservation::Reserved(resource) = reservation else {
            panic!("first R2 bucket creation replayed")
        };
        let objects = r2_objects(&self._runtime._mock);
        R2ResourceDriver::new(self.storage(), objects, R2Config::default())
            .create(&resource)
            .await
            .unwrap();
        ResourceRepository::new(self.storage().db())
            .mark_ready(resource_id, 21)
            .unwrap();
        ResourceRepository::new(self.storage().db())
            .get(self._runtime.account, resource_id)
            .unwrap()
    }

    async fn put_r2_object(
        &self,
        bucket: &ResourceRecord,
        key: &str,
        bytes: &[u8],
        metadata: BTreeMap<String, String>,
    ) {
        let objects = r2_objects(&self._runtime._mock);
        let bucket_state = R2BucketRepository::new(self.storage().db())
            .get(self._runtime.account, bucket.id)
            .unwrap();
        let locator = objects
            .locator(bucket.id, &bucket_state.physical_prefix)
            .unwrap();
        let path = self
            ._runtime
            ._temp
            .path()
            .join(format!("r2-{}", Uuid::now_v7()));
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let version = Uuid::now_v7().to_string();
        R2ObjectRepository::new(self.storage().db())
            .begin_put(
                &R2ObjectRecord {
                    resource_id: bucket.id,
                    account_id: self._runtime.account,
                    object_key: key.to_owned(),
                    object_version: version.clone(),
                    ssec_key_md5: None,
                    ssec_envelope: None,
                },
                22,
            )
            .unwrap();
        let source = R2UploadSource {
            path,
            length: u64::try_from(bytes.len()).unwrap(),
            checksums: hash_bytes(bytes),
            version: version.clone(),
        };
        let uploaded = objects
            .put_file(
                &locator,
                &UserObjectKey::parse(key).unwrap(),
                &source,
                &R2PutOptions {
                    http_metadata: R2HttpMetadata {
                        content_type: Some("text/markdown".to_owned()),
                        ..R2HttpMetadata::default()
                    },
                    custom_metadata: metadata,
                    ..R2PutOptions::default()
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        R2ObjectRepository::new(self.storage().db())
            .finish_put(self._runtime.account, bucket.id, key, &uploaded.version, 23)
            .unwrap();
    }

    fn storage(&self) -> &Arc<PlatformStorage> {
        &self._runtime.storage
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
    config.backends.insert(
        "fixture".to_owned(),
        AiBackendConfig {
            protocol: AiBackendProtocol::OpenAiEmbeddingsV1,
            endpoint: "http://127.0.0.1:8080/v1/embeddings".to_owned(),
            auth: AiAuthConfig::None,
            headers: Default::default(),
        },
    );
    let alias = "@cf/qwen/qwen3-embedding-0.6b";
    let profile = "fixture/qwen3";
    config.embedding_profiles.insert(
        profile.to_owned(),
        AiEmbeddingProfileConfig {
            dimensions: 1_024,
            max_input_tokens: 8_192,
            send_dimensions: false,
            tokenizer: AiTokenizerConfig {
                kind: AiTokenizer::Qwen3,
                revision: "fixture-tokenizer".to_owned(),
                artifact: AiTokenizerArtifactConfig {
                    path,
                    sha256: hex::encode(Sha256::digest(bytes)),
                },
            },
        },
    );
    config.embedding_models.insert(
        alias.to_owned(),
        AiEmbeddingModelConfig {
            backend: "fixture".to_owned(),
            remote_model: alias.to_owned(),
            provider_revision: Some("fixture-model".to_owned()),
            profile: profile.to_owned(),
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

fn r2_objects(mock: &MockS3) -> R2ObjectStore {
    let config = open_compute_core::S3Config {
        endpoint: mock.endpoint.clone(),
        bucket: "open-compute".to_owned(),
        ..open_compute_core::S3Config::default()
    };
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    R2ObjectStore::new(ObjectBackend::connect_s3(&config, &credentials, 4 * 1024 * 1024).unwrap())
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
