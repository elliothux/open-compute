//! Host fixtures, resource setup, version dispatch, and local provider helpers.

use super::*;
use sha2::{Digest as _, Sha256};
use std::os::unix::fs::PermissionsExt;

pub(super) fn create_vectorize(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account: open_compute_core::AccountId,
) -> open_compute_core::ResourceId {
    let controller = ResourceController::new(
        storage,
        pins,
        VectorizeResourceDriver::new(
            storage,
            VectorizeIndexSpec {
                dimensions: 32,
                metric: "cosine".to_owned(),
                quota_vectors: 1_000,
                quota_bytes: 16 * 1024 * 1024,
            },
            5_000,
        ),
    );
    match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::VectorizeIndex,
            name: "p5-vectors".to_owned(),
            idempotency_key: "p5-vectors".to_owned(),
            driver_schema_version: VECTORIZE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 1,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected Vectorize replay"),
    }
}

pub(super) fn create_ai_search_namespace(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account: open_compute_core::AccountId,
) -> open_compute_core::ResourceId {
    let controller =
        ResourceController::new(storage, pins, AiSearchNamespaceResourceDriver::new(storage));
    match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::AiSearchNamespace,
            name: "p5-search-namespace".to_owned(),
            idempotency_key: "p5-search-namespace".to_owned(),
            driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 2,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected AI Search replay"),
    }
}

pub(super) fn create_ai_search_instance(
    storage: &PlatformStorage,
    pins: ResourcePins,
    ai: &AiConfig,
    account: open_compute_core::AccountId,
    namespace: open_compute_core::ResourceId,
) -> open_compute_core::ResourceId {
    let input: AiSearchCreateInput = serde_json::from_value(serde_json::json!({
        "id": "direct",
        "embedding_model": EMBEDDING_ALIAS,
        "ai_search_model": GENERATION_ALIAS,
        "index_method": { "vector": true, "keyword": true },
        "chunk": true,
        "chunk_size": 128,
        "chunk_overlap": 10,
        "rewrite_query": false,
        "reranking": false
    }))
    .unwrap();
    let prepared = input.prepare(ai).unwrap();
    let controller = ResourceController::new(
        storage,
        pins,
        AiSearchInstanceResourceDriver::new(
            storage,
            AiSearchInstanceSpec {
                namespace_resource_id: namespace,
                instance_key: "direct".to_owned(),
                public_config_json: prepared.public_config_json,
                model_contract_json: prepared.model_contract_json,
                model_contract_sha256: prepared.model_contract_sha256,
                dimensions: prepared.dimensions,
                vector_enabled: prepared.vector_enabled,
                keyword_enabled: prepared.keyword_enabled,
            },
            5_000,
        ),
    );
    match controller
        .create(&CreateResourceRequest {
            account_id: account,
            kind: BindingKind::AiSearchInstance,
            name: "p5-direct-search".to_owned(),
            idempotency_key: "p5-direct-search".to_owned(),
            driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 3,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected AI Search instance replay"),
    }
}

pub(super) fn create_metadata_index(
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
    resource: open_compute_core::ResourceId,
) {
    let record = VectorizeIndexRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    let path = VectorizePaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, resource)
        .unwrap();
    VectorizeEngine::open(
        &path,
        &resource.to_string(),
        record.dimensions,
        &record.metric,
        record.quota_vectors,
        record.quota_bytes,
        5_000,
    )
    .unwrap()
    .create_metadata_index("year", "number", 3)
    .unwrap();
}

pub(super) fn version_request(
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    vectorize: open_compute_core::ResourceId,
    search: open_compute_core::ResourceId,
    direct_search: open_compute_core::ResourceId,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: TENANT_SOURCE.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let bindings = BTreeMap::from([
        (
            "VECTOR".to_owned(),
            VersionBindingInput {
                kind: BindingKind::VectorizeIndex,
                id: vectorize,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        ),
        (
            "SEARCH".to_owned(),
            VersionBindingInput {
                kind: BindingKind::AiSearchNamespace,
                id: search,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        ),
        (
            "DIRECT_SEARCH".to_owned(),
            VersionBindingInput {
                kind: BindingKind::AiSearchInstance,
                id: direct_search,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        ),
    ]);
    CreateVersionRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "p5-version".to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: VersionRuntimeFeatures {
            ai: Some(VersionAiInput {
                binding: "AI".to_owned(),
            }),
            ..VersionRuntimeFeatures::default()
        },
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms: 10,
    }
}

pub(super) async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
    supervisor: &WorkerdSupervisor,
) -> open_compute_storage::VersionRecord {
    match controller
        .create_version(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "version failed: {error:?}; diagnostics={:?}",
                supervisor.last_diagnostics()
            )
        }) {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

pub(super) async fn dispatch(
    transport: &WorkerdTransport,
    workers: &WorkerRepository<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    uri: &str,
) -> (u16, String) {
    let route_generation = i64::try_from(
        workers
            .get_worker(account, worker)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id: account,
                worker_id: worker,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: None,
                route_generation,
                request_id: RequestId::generate(),
            },
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::HOST, "p5.example.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap_or_else(|error| panic!("tenant dispatch {uri} failed: {error:?}"));
    let status = response.status().as_u16();
    let body = String::from_utf8(
        to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

pub(super) fn ai_config(
    chat_base_url: &str,
    embedding_base_url: &str,
    secret_root: &Path,
) -> AiConfig {
    let mut config = AiConfig::default();
    let (tokenizer_path, tokenizer_sha256) = tokenizer_artifact();
    config.providers.insert(
        "fixture".to_owned(),
        AiProviderConfig {
            base_url: embedding_base_url.to_owned(),
            auth: AiAuthConfig::Bearer {
                secret: embedding_secret(secret_root),
            },
        },
    );
    config.providers.insert(
        "chat-fixture".to_owned(),
        AiProviderConfig {
            base_url: chat_base_url.to_owned(),
            auth: AiAuthConfig::None,
        },
    );
    config.embedding_models.insert(
        EMBEDDING_ALIAS.to_owned(),
        AiEmbeddingModelConfig {
            provider: "fixture".to_owned(),
            remote_model: EMBEDDING_ALIAS.to_owned(),
            model_revision: "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3".to_owned(),
            dimensions: 1_024,
            request_dimensions: None,
            metric: AiEmbeddingMetric::Cosine,
            max_input_tokens: 8_192,
            tokenizer: AiTokenizer::Qwen3,
            tokenizer_revision: "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3".to_owned(),
            tokenizer_artifact: AiTokenizerArtifactConfig {
                path: tokenizer_path,
                sha256: tokenizer_sha256,
            },
        },
    );
    config.default_embedding_model = Some(EMBEDDING_ALIAS.to_owned());
    config.generation_models.insert(
        GENERATION_ALIAS.to_owned(),
        AiGenerationModelConfig {
            provider: "chat-fixture".to_owned(),
            remote_model: "fixture-chat".to_owned(),
            model_revision: "fixture-chat-revision".to_owned(),
            max_context_tokens: 4_096,
            capabilities: BTreeSet::from([
                AiGenerationCapability::Chat,
                AiGenerationCapability::Rewrite,
                AiGenerationCapability::Rerank,
            ]),
        },
    );
    config.default_generation_model = Some(GENERATION_ALIAS.to_owned());
    config.validate().unwrap();
    config
}

fn tokenizer_artifact() -> (PathBuf, String) {
    if let Some(path) = std::env::var_os("OPEN_COMPUTE_TEST_TOKENIZER_PATH") {
        let tokenizer_path = PathBuf::from(path)
            .canonicalize()
            .expect("pinned tokenizer artifact");
        let tokenizer_sha256 =
            std::env::var("OPEN_COMPUTE_TEST_TOKENIZER_SHA256").unwrap_or_else(|_| {
                "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a".to_owned()
            });
        return (tokenizer_path, tokenizer_sha256);
    }
    let tokenizer_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tokenizer-word-level.json")
        .canonicalize()
        .expect("in-repo tokenizer fixture");
    let bytes = std::fs::read(&tokenizer_path).expect("tokenizer fixture bytes");
    (tokenizer_path, hex::encode(Sha256::digest(bytes)))
}

fn embedding_secret(root: &Path) -> SecretReference {
    match std::env::var(EMBEDDING_KEY_ENV) {
        Ok(value) if !value.is_empty() => SecretReference {
            env: Some(EMBEDDING_KEY_ENV.to_owned()),
            file: None,
        },
        _ => {
            let path = root.join("embedding-api-key");
            std::fs::write(&path, EMBEDDING_FIXTURE_SECRET).unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&path, permissions).unwrap();
            SecretReference {
                env: None,
                file: Some(path),
            }
        }
    }
}

pub(super) async fn spawn_embedding_fixture() -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/embeddings", post(embedding_fixture)),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .unwrap();
    });
    (format!("http://{address}/v1"), shutdown_tx, task)
}

pub(super) async fn embedding_fixture(
    headers: HeaderMap,
    body: Bytes,
) -> axum::http::Response<String> {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer fixture-secret");
    if !authorized {
        return axum::http::Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(String::new())
            .unwrap();
    }
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return axum::http::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(String::new())
                .unwrap();
        }
    };
    if request.get("model") != Some(&serde_json::json!(EMBEDDING_ALIAS)) {
        return axum::http::Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(String::new())
            .unwrap();
    }
    let inputs = match request.get("input") {
        Some(serde_json::Value::String(value)) => vec![value.clone()],
        Some(serde_json::Value::Array(values)) => {
            let mut inputs = Vec::with_capacity(values.len());
            for value in values {
                let Some(input) = value.as_str() else {
                    return axum::http::Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(String::new())
                        .unwrap();
                };
                inputs.push(input.to_owned());
            }
            inputs
        }
        _ => {
            return axum::http::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(String::new())
                .unwrap();
        }
    };
    let data: Vec<serde_json::Value> = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| {
            serde_json::json!({
                "object": "embedding",
                "index": index,
                "embedding": fixture_embedding(input),
            })
        })
        .collect();
    axum::http::Response::builder()
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "object": "list",
                "model": EMBEDDING_ALIAS,
                "data": data,
                "usage": {
                    "prompt_tokens": inputs.len() as u64,
                    "total_tokens": inputs.len() as u64
                }
            })
            .to_string(),
        )
        .unwrap()
}

fn fixture_embedding(text: &str) -> Vec<f32> {
    let mut values = vec![0.0_f32; 1_024];
    for token in text.split_whitespace() {
        let digest = Sha256::digest(token.as_bytes());
        let index = u32::from_le_bytes(digest[..4].try_into().unwrap()) as usize % 1_024;
        values[index] += 1.0;
    }
    if values.iter().all(|value| *value == 0.0) {
        values[0] = 1.0;
    }
    values
}

pub(super) async fn spawn_chat_fixture() -> (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(chat_fixture)),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .unwrap();
    });
    (format!("http://{address}/v1"), shutdown_tx, task)
}

pub(super) async fn chat_fixture(body: Bytes) -> axum::http::Response<String> {
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return axum::http::Response::builder()
                .status(400)
                .body(String::new())
                .unwrap();
        }
    };
    if request.get("model") != Some(&serde_json::json!("fixture-chat")) {
        return axum::http::Response::builder()
            .status(400)
            .body(String::new())
            .unwrap();
    }
    if request.get("stream") == Some(&serde_json::json!(true)) {
        return axum::http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hel\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_owned(),
            )
            .unwrap();
    }
    axum::http::Response::builder()
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "fixture-chat",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "answer" },
                    "finish_reason": "stop"
                }]
            })
            .to_string(),
        )
        .unwrap()
}

pub(super) async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut snapshots = supervisor.subscribe();
    loop {
        let snapshot = snapshots.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert_ne!(snapshot.state, SupervisorState::Failed, "{snapshot:?}");
        assert!(Instant::now() < deadline, "workerd did not become ready");
        tokio::time::timeout(Duration::from_millis(250), snapshots.changed())
            .await
            .ok();
    }
}

pub(super) fn runtime_config() -> RuntimeConfig {
    RuntimeConfig {
        startup_timeout_ms: 20_000,
        shutdown_grace_ms: 500,
        drain_timeout_ms: 500,
        kill_timeout_ms: 500,
        restart_budget: 3,
        restart_window_ms: 60_000,
        restart_backoff_initial_ms: 10,
        restart_backoff_max_ms: 100,
    }
}

pub(super) fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

pub(super) fn artifact_store(mock: &MockS3) -> (ArtifactStore, ObjectBackend) {
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
    let client = ObjectBackend::connect_s3(&config, &credentials, 32 * 1024 * 1024).unwrap();
    (ArtifactStore::new(client.clone()), client)
}

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
