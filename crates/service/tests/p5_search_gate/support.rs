//! Host fixtures, resource setup, deployment dispatch, and local provider helpers.

use super::*;

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

pub(super) fn deployment_request(
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    vectorize: open_compute_core::ResourceId,
    search: open_compute_core::ResourceId,
    direct_search: open_compute_core::ResourceId,
) -> CreateDeploymentRequest {
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
            DeploymentBindingInput {
                kind: BindingKind::VectorizeIndex,
                id: vectorize,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        ),
        (
            "SEARCH".to_owned(),
            DeploymentBindingInput {
                kind: BindingKind::AiSearchNamespace,
                id: search,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        ),
        (
            "DIRECT_SEARCH".to_owned(),
            DeploymentBindingInput {
                kind: BindingKind::AiSearchInstance,
                id: direct_search,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        ),
    ]);
    CreateDeploymentRequest {
        account_id: account,
        worker_id: worker,
        idempotency_key: "p5-deployment".to_owned(),
        content: DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: DeploymentRuntimeFeatures {
            ai: Some(DeploymentAiInput {
                binding: "AI".to_owned(),
            }),
            ..DeploymentRuntimeFeatures::default()
        },
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: true,
        request_id: RequestId::generate(),
        now_ms: 10,
    }
}

pub(super) async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
    supervisor: &WorkerdSupervisor,
) -> open_compute_storage::DeploymentRecord {
    match controller
        .create_deployment(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "deployment failed: {error:?}; diagnostics={:?}",
                supervisor.last_diagnostics()
            )
        }) {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    }
}

pub(super) async fn dispatch(
    transport: &WorkerdTransport,
    workers: &WorkerRepository<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
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
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
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

pub(super) fn ai_config(chat_base_url: &str) -> AiConfig {
    let base_url = std::env::var("OPEN_COMPUTE_TEST_EMBEDDING_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080/v1".to_owned());
    let mut config = AiConfig::default();
    let tokenizer_path = std::env::var_os("OPEN_COMPUTE_TEST_TOKENIZER_PATH").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.temp/tei-hf/hub/models--Qwen--Qwen3-Embedding-0.6B/blobs/def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a")
        },
        PathBuf::from,
    )
        .canonicalize()
        .expect("pinned Qwen3 tokenizer artifact");
    let tokenizer_sha256 =
        std::env::var("OPEN_COMPUTE_TEST_TOKENIZER_SHA256").unwrap_or_else(|_| {
            "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a".to_owned()
        });
    config.providers.insert(
        "fixture".to_owned(),
        AiProviderConfig {
            base_url,
            auth: AiAuthConfig::Bearer {
                secret: SecretReference {
                    env: Some(EMBEDDING_KEY_ENV.to_owned()),
                    file: None,
                },
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

pub(super) fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

pub(super) fn artifact_store(mock: &MockS3) -> (ArtifactStore, S3ArtifactClient) {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
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
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    let client = S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap();
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
