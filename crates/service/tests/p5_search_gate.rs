//! Stock-workerd P5 Vectorize, AI Search, and Markdown Conversion tenant gate.

#[path = "p5_search_gate/support.rs"]
mod support;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::routing::post;
use open_compute_artifacts::{
    AiSearchObjectStore, ArtifactCache, ArtifactStore, MapEnv, MockS3, S3ArtifactClient,
    resolve_s3_credentials_with,
};
use open_compute_core::{
    AiAuthConfig, AiConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiGenerationCapability,
    AiGenerationModelConfig, AiProviderConfig, AiTokenizer, AiTokenizerArtifactConfig, BindingKind,
    CacheConfig, CanonicalBindingConfig, CanonicalPermissions, DocumentParserConfig,
    PlatformConfig, Redactor, RequestId, RuntimeConfig, SecretReference, StartupId, StorageConfig,
    SystemClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::ai_search_config::AiSearchCreateInput;
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::document_parser_backend::DocumentParserBindingService;
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend_with_ai_search,
};
use open_compute_storage::{
    AI_SEARCH_SCHEMA_VERSION, PlatformStorage, VECTORIZE_SCHEMA_VERSION, VectorizeEngine,
    VectorizeIndexRepository, VectorizePaths, WorkerRepository,
};
use open_compute_workers::{
    AiSearchInstanceResourceDriver, AiSearchInstanceSpec, AiSearchNamespaceResourceDriver,
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    CreateResourceOutcome, CreateResourceRequest, DeploymentAiInput, DeploymentBindingInput,
    DeploymentContent, DeploymentController, DeploymentPins, DeploymentRuntimeFeatures,
    ModuleInput, ModuleType, ResourceController, ResourcePins, RuntimeSource, RuntimeValidator,
    VectorizeIndexSpec, VectorizeResourceDriver,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::*;

const EMBEDDING_ALIAS: &str = "@cf/qwen/qwen3-embedding-0.6b";
const GENERATION_ALIAS: &str = "@cf/meta/llama-3.3-70b-instruct-fp8-fast";
const EMBEDDING_KEY_ENV: &str = "OPEN_COMPUTE_TEST_EMBEDDING_API_KEY";
const EMBEDDING_BASE_URL_ENV: &str = "OPEN_COMPUTE_TEST_EMBEDDING_BASE_URL";
const EMBEDDING_FIXTURE_SECRET: &str = "fixture-secret";

const TENANT_SOURCE: &str = r##"
import { WorkerEntrypoint } from "cloudflare:workers";
const vector = (axis) => Array.from({ length: 32 }, (_, index) => index === axis ? 1 : 0);
export default class Main extends WorkerEntrypoint {
  async fetch(request) {
    let stage = "route";
    try {
    const phase = new URL(request.url).searchParams.get("phase");
    if (phase === "vector") {
    stage = "vector-insert";
    const mutation = await this.env.VECTOR.insert([
      { id: "recent", values: vector(0), metadata: { year: 2026, topic: "search" } },
      { id: "old", values: vector(1), metadata: { year: 2010, topic: "archive" } },
    ]);
    stage = "vector-describe";
    let description;
    for (let attempt = 0; attempt < 600; attempt += 1) {
      description = await this.env.VECTOR.describe();
      if (description.vectorCount === 2) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    stage = "vector-query";
    const matches = await this.env.VECTOR.query(vector(0), {
      topK: 2,
      returnValues: true,
      returnMetadata: "all",
      filter: { year: { $gte: 2020, $lt: 2030 } },
    });
    stage = "vector-invalid-dimension";
    let rejectedDimension = false;
    try { await this.env.VECTOR.query([1, 2], { topK: 1 }); }
    catch { rejectedDimension = true; }
    stage = "vector-get";
    const fetched = await this.env.VECTOR.getByIds(["recent", "missing"]);
    stage = "vector-query-by-id";
    const byId = await this.env.VECTOR.queryById("recent", { topK: 1 });
    stage = "vector-upsert";
    const upsert = await this.env.VECTOR.upsert([
      { id: "recent", values: vector(2), metadata: { year: 2026, topic: "updated" } },
    ]);
    stage = "vector-upsert-poll";
    let updated = [];
    for (let attempt = 0; attempt < 600; attempt += 1) {
      updated = await this.env.VECTOR.getByIds(["recent"]);
      if (updated[0]?.values?.[2] === 1) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    stage = "vector-delete";
    const deletion = await this.env.VECTOR.deleteByIds(["old"]);
    stage = "vector-delete-poll";
    let deleted = [true];
    for (let attempt = 0; attempt < 600; attempt += 1) {
      deleted = await this.env.VECTOR.getByIds(["old"]);
      if (deleted.length === 0) break;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    return Response.json({
      mutation, description, matches, rejectedDimension, fetched, byId, upsert, updated, deletion, deleted,
    });
    }

    if (phase === "namespace-upload") {
    stage = "search-create";
    const instance = await this.env.SEARCH.create({
      id: "docs",
      embedding_model: "@cf/qwen/qwen3-embedding-0.6b",
      ai_search_model: "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
      index_method: { vector: true, keyword: true },
      chunk: true,
      chunk_size: 128,
      chunk_overlap: 10,
      rewrite_query: false,
      reranking: false,
      custom_metadata: [{ field_name: "category", data_type: "text" }],
    });
    stage = "search-upload";
    const queuedUpload = await instance.items.upload(
      "guide.md",
      new Blob(["# Search guide\n\nThe cobalt retrieval marker belongs to the current document."], { type: "text/markdown" }),
      { metadata: { category: "guide" } },
    );
    return Response.json({ queuedUpload });
    }

    if (phase === "namespace-status") {
    stage = "search-status";
    const selected = this.env.SEARCH.get("docs");
    const items = await selected.items.list({ page: 1, per_page: 10, key: "guide.md" });
    const item = items.result.find(entry => entry.key === "guide.md");
    if (!item) throw new Error("uploaded guide.md item was not listed");
    const selectedItem = selected.items.get(item.id);
    return Response.json({
      item: await selectedItem.info(),
      indexingLogs: await selectedItem.logs({ limit: 10 }),
    });
    }

    if (phase === "namespace-retrieval") {
    stage = "search-retrieval";
    const instance = this.env.SEARCH.get("docs");
    const retrieval = await instance.search({
      query: "cobalt retrieval marker",
      ai_search_options: { retrieval: { retrieval_type: "hybrid", max_num_results: 5 } },
    });
    return Response.json({ retrieval });
    }

    if (phase === "namespace-management") {
    stage = "search-management";
    const listedInstances = await this.env.SEARCH.list({
      page: 1, per_page: 10, search: "doc", order_by: "created_at", order_by_direction: "desc",
    });
    const selected = this.env.SEARCH.get("docs");
    const instanceInfo = await selected.info();
    const instanceStats = await selected.stats();
    const updatedInstance = await selected.update({ metadata: { gate: "updated" } });
    const items = await selected.items.list({ page: 1, per_page: 10, key: "guide.md" });
    const item = items.result.find(entry => entry.key === "guide.md");
    if (!item) throw new Error("uploaded guide.md item was not listed");
    const selectedItem = selected.items.get(item.id);
    const selectedItemInfo = await selectedItem.info();
    const itemDownload = await selectedItem.download();
    const downloadedText = await new Response(itemDownload.body).text();
    const itemLogs = await selectedItem.logs({ limit: 10 });
    const itemChunks = await selectedItem.chunks({ limit: 10, offset: 0 });
    const syncStarted = await selectedItem.sync();
    return Response.json({
      listedInstances, instanceInfo, instanceStats, updatedInstance, items,
      selectedItemInfo, itemDownload: { contentType: itemDownload.contentType, filename: itemDownload.filename, size: itemDownload.size },
      downloadedText, itemLogs, itemChunks, syncStarted,
    });
    }

    if (phase === "namespace-sync-status") {
    stage = "search-sync-status";
    const selected = this.env.SEARCH.get("docs");
    const items = await selected.items.list({ page: 1, per_page: 10, key: "guide.md" });
    const item = items.result.find(entry => entry.key === "guide.md");
    if (!item) throw new Error("uploaded guide.md item was not listed");
    return Response.json({ syncedTerminal: await selected.items.get(item.id).info() });
    }

    if (phase === "namespace-jobs") {
    stage = "search-jobs-upload";
    const selected = this.env.SEARCH.get("docs");
    const queuedItem = await selected.items.upload(
      "delete-me.txt",
      "temporary AI Search item",
      { metadata: { category: "guide" } },
    );
    stage = "search-jobs-delete";
    await selected.items.delete(queuedItem.id);

    stage = "search-jobs-create";
    const jobCreated = await selected.jobs.create({ description: "stock workerd reindex" });
    const selectedJob = selected.jobs.get(jobCreated.id);
    stage = "search-jobs-info";
    const jobInfo = await selectedJob.info();
    stage = "search-jobs-logs";
    const jobLogs = await selectedJob.logs({ page: 1, per_page: 10 });
    stage = "search-jobs-cancel";
    const jobCancelled = await selectedJob.cancel();
    stage = "search-jobs-list";
    const jobs = await selected.jobs.list({ page: 1, per_page: 10 });
    return Response.json({
      queuedItem, jobCreated, jobInfo, jobLogs, jobCancelled, jobs,
    });
    }

    if (phase === "direct-upload") {
    stage = "direct-upload";
    const directQueuedUpload = await this.env.DIRECT_SEARCH.items.upload(
      "direct.md",
      new Blob(["# Direct binding\n\nThe amber direct binding marker is searchable."], { type: "text/markdown" }),
    );
    return Response.json({ directQueuedUpload });
    }

    if (phase === "direct-status") {
    stage = "direct-status";
    const items = await this.env.DIRECT_SEARCH.items.list({ page: 1, per_page: 10, key: "direct.md" });
    const item = items.result.find(entry => entry.key === "direct.md");
    if (!item) throw new Error("uploaded direct.md item was not listed");
    return Response.json({ directItem: await this.env.DIRECT_SEARCH.items.get(item.id).info() });
    }

    if (phase === "direct-search") {
    stage = "direct-search";
    const directSearch = await this.env.DIRECT_SEARCH.search({ query: "amber direct binding marker" });
    const multiSearch = await this.env.SEARCH.search({
      query: "binding marker",
      ai_search_options: { instance_ids: ["docs", "direct"], retrieval: { max_num_results: 5 } },
    });
    return Response.json({ directSearch, multiSearch });
    }

    if (phase === "chat") {
    stage = "chat";
    const directChat = await this.env.DIRECT_SEARCH.chatCompletions({
      messages: [{ role: "user", content: "Summarize the direct binding marker." }],
    });
    const directChatStream = await this.env.DIRECT_SEARCH.chatCompletions({
      messages: [{ role: "user", content: "Stream a summary." }],
      stream: true,
    });
    const directChatSse = await new Response(directChatStream).text();
    const multiChat = await this.env.SEARCH.chatCompletions({
      messages: [{ role: "user", content: "Summarize binding markers." }],
      ai_search_options: { instance_ids: ["docs", "direct"] },
    });
    const multiChatStream = await this.env.SEARCH.chatCompletions({
      messages: [{ role: "user", content: "Stream binding markers." }],
      stream: true,
      ai_search_options: { instance_ids: ["docs", "direct"] },
    });
    const multiChatSse = await new Response(multiChatStream).text();
    const trash = await this.env.SEARCH.create({ id: "trash" });
    const trashInfo = await trash.info();
    await this.env.SEARCH.delete("trash");
    return Response.json({
      directChat, directChatSse, multiChat, multiChatSse, trashInfo,
    });
    }

    if (phase === "markdown") {
    stage = "markdown-single";
    const markdown = await this.env.AI.toMarkdown({
      name: "tenant.md",
      blob: new Blob(["# Markdown gate\n\nstock workerd"], { type: "text/markdown" }),
    });
    stage = "markdown-handle";
    const markdownService = this.env.AI.toMarkdown();
    const transformed = await markdownService.transform({
      name: "tenant.txt",
      blob: new Blob(["handle transform gate"], { type: "text/plain" }),
    });
    stage = "markdown-batch";
    const markdownBatch = await this.env.AI.toMarkdown([
      { name: "one.md", blob: new Blob(["# one"], { type: "text/markdown" }) },
      { name: "bad.png", blob: new Blob(["not an image"], { type: "image/png" }) },
    ]);
    stage = "markdown-handle-batch";
    const transformedBatch = await markdownService.transform([
      { name: "two.txt", blob: new Blob(["two"], { type: "text/plain" }) },
      { name: "three.md", blob: new Blob(["# three"], { type: "text/markdown" }) },
    ]);
    stage = "markdown-supported";
    const supported = await markdownService.supported();
    return Response.json({
      markdown, transformed, markdownBatch, transformedBatch, supported,
      aiGatewayLogId: this.env.AI.aiGatewayLogId,
    });
    }
    return Response.json({ error: "unknown phase" }, { status: 404 });
    } catch (error) {
      return Response.json({ stage, error: String(error) }, { status: 599 });
    }
  }
}
"##;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p5_real_vectorize_ai_search_and_markdown_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temporary = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &storage_config(&temporary.path().join("data")),
            &SystemClock,
        )
        .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let (chat_base_url, chat_shutdown, chat_task) = spawn_chat_fixture().await;
    let embedding_fixture = match std::env::var(EMBEDDING_BASE_URL_ENV) {
        Ok(url) if !url.is_empty() => None,
        _ => Some(spawn_embedding_fixture().await),
    };
    let embedding_base_url = embedding_fixture.as_ref().map_or_else(
        || std::env::var(EMBEDDING_BASE_URL_ENV).unwrap(),
        |(url, _, _)| url.clone(),
    );
    let ai = ai_config(&chat_base_url, &embedding_base_url, temporary.path());
    let (artifacts, s3_client) = artifact_store(&mock);
    let artifact_cache = Arc::new(
        ArtifactCache::open(
            storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .unwrap();
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let deployment_pins = DeploymentPins::new();
    let resource_pins = ResourcePins::new();
    let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default())
                .with_cache(artifact_cache.clone());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let document_parser = Arc::new(DocumentParserBindingService::with_executable(
        storage.clone(),
        DocumentParserConfig::default(),
        PathBuf::from(env!("CARGO_BIN_EXE_ocd")),
    ));
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = deployment_pins.clone();
        let resources = resource_pins.clone();
        let assets = Arc::new(AssetBindingService::new(
            storage.clone(),
            artifacts.clone(),
            artifact_cache,
            pins.clone(),
        ));
        let services = Arc::new(ServiceInvocationRegistry::new(storage.clone(), pins));
        let ai = ai.clone();
        let objects = AiSearchObjectStore::new(s3_client);
        let parser = document_parser.clone();
        async move {
            serve_binding_backend_with_ai_search(
                binding_listener,
                storage.clone(),
                auth,
                resources,
                Arc::new(SqliteKvBindingExecutor::new(
                    storage.clone(),
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
                open_compute_core::DurableObjectsConfig::default(),
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                None,
                assets,
                services,
                None,
                None,
                parser,
                ai,
                objects,
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });

    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock,
        root.join("packages/runtime"),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p5-search-gate".to_owned(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone())
        .with_deployment_pins(deployment_pins.clone());
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            runtime.version_output(),
        )
        .unwrap();
    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: Some(storage.data_dir().runtime_dir().join("p5-search.lease")),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth, binding_auth],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let vectorize_id = create_vectorize(&storage, resource_pins.clone(), account);
    let search_id = create_ai_search_namespace(&storage, resource_pins.clone(), account);
    let direct_search_id =
        create_ai_search_instance(&storage, resource_pins, &ai, account, search_id);
    create_metadata_index(&storage, account, vectorize_id);
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(account, "p5-search", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0;
    let controller = DeploymentController::new(
        &storage,
        artifacts,
        Arc::new(transport.clone()) as Arc<dyn RuntimeValidator>,
        BundleLimits::default(),
    );
    let deployment = deploy(
        &controller,
        deployment_request(
            account,
            worker.id,
            vectorize_id,
            search_id,
            direct_search_id,
        ),
        &supervisor,
    )
    .await;
    macro_rules! request_phase {
        ($phase:expr) => {{
            let phase = $phase;
            let uri = format!("/gate?phase={phase}");
            let response =
                dispatch(&transport, &workers, account, worker.id, &deployment, &uri).await;
            if response.0 != 200 {
                supervisor.shutdown().await;
                panic!(
                    "tenant phase={phase} response status={}: {}; diagnostics={:?}",
                    response.0,
                    response.1,
                    supervisor.last_diagnostics()
                );
            }
            match serde_json::from_str::<serde_json::Value>(&response.1).unwrap() {
                serde_json::Value::Object(fields) => fields,
                value => panic!("tenant phase={phase} returned a non-object: {value}"),
            }
        }};
    }
    fn merge_phase_fields(
        target: &mut serde_json::Map<String, serde_json::Value>,
        phase: &str,
        fields: serde_json::Map<String, serde_json::Value>,
    ) {
        for (key, value) in fields {
            assert!(
                target.insert(key.clone(), value).is_none(),
                "tenant phase {phase} returned duplicate field {key}"
            );
        }
    }

    let mut body_fields = serde_json::Map::new();
    for phase in ["vector", "namespace-upload"] {
        merge_phase_fields(&mut body_fields, phase, request_phase!(phase));
    }
    let namespace_started = Instant::now();
    let namespace_item = loop {
        let fields = request_phase!("namespace-status");
        let status = fields["item"]["status"].as_str();
        if matches!(status, Some("completed" | "error" | "skipped" | "outdated")) {
            break fields;
        }
        if namespace_started.elapsed() >= Duration::from_secs(60) {
            panic!("namespace item did not reach a terminal state: {fields:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    merge_phase_fields(&mut body_fields, "namespace-status", namespace_item);
    for phase in ["namespace-retrieval", "namespace-management"] {
        merge_phase_fields(&mut body_fields, phase, request_phase!(phase));
    }
    let sync_started = Instant::now();
    let synced_item = loop {
        let fields = request_phase!("namespace-sync-status");
        let status = fields["syncedTerminal"]["status"].as_str();
        if matches!(status, Some("completed" | "error" | "skipped" | "outdated")) {
            break fields;
        }
        assert!(
            sync_started.elapsed() < Duration::from_secs(60),
            "namespace sync did not reach a terminal state"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    merge_phase_fields(&mut body_fields, "namespace-sync-status", synced_item);
    for phase in ["namespace-jobs", "direct-upload"] {
        merge_phase_fields(&mut body_fields, phase, request_phase!(phase));
    }
    let direct_started = Instant::now();
    let direct_item = loop {
        let fields = request_phase!("direct-status");
        let status = fields["directItem"]["status"].as_str();
        if matches!(status, Some("completed" | "error" | "skipped" | "outdated")) {
            break fields;
        }
        assert!(
            direct_started.elapsed() < Duration::from_secs(60),
            "direct item did not reach a terminal state"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    merge_phase_fields(&mut body_fields, "direct-status", direct_item);
    for phase in ["direct-search", "chat", "markdown"] {
        merge_phase_fields(&mut body_fields, phase, request_phase!(phase));
    }
    let body = serde_json::Value::Object(body_fields);
    assert!(body["mutation"]["mutationId"].as_str().is_some());
    assert_eq!(body["description"]["dimensions"], 32);
    assert_eq!(body["description"]["vectorCount"], 2);
    assert_eq!(body["matches"]["count"], 1);
    assert_eq!(body["matches"]["matches"][0]["id"], "recent");
    assert_eq!(body["matches"]["matches"][0]["metadata"]["year"], 2026);
    assert_eq!(body["rejectedDimension"], true);
    assert_eq!(body["fetched"].as_array().unwrap().len(), 1);
    assert_eq!(body["byId"]["matches"][0]["id"], "recent");
    assert!(body["upsert"]["mutationId"].as_str().is_some());
    assert_eq!(body["updated"][0]["values"][2], 1.0);
    assert!(body["deletion"]["mutationId"].as_str().is_some());
    assert!(body["deleted"].as_array().unwrap().is_empty());
    assert_eq!(body["queuedUpload"]["status"], "queued");
    assert_eq!(body["item"]["status"], "completed");
    assert!(
        body["retrieval"]["chunks"]
            .as_array()
            .is_some_and(|chunks| chunks.iter().any(|chunk| chunk["text"]
                .as_str()
                .is_some_and(|text| text.contains("cobalt retrieval marker"))))
    );
    assert!(
        body["listedInstances"]["result"]
            .as_array()
            .is_some_and(|instances| instances.iter().any(|instance| instance["id"] == "docs"))
    );
    assert_eq!(body["instanceInfo"]["id"], "docs");
    assert_eq!(body["instanceInfo"]["status"], "ready");
    assert!(
        body["instanceStats"]["completed"]
            .as_u64()
            .unwrap_or_default()
            >= 1
    );
    assert_eq!(body["updatedInstance"]["metadata"]["gate"], "updated");
    assert!(
        body["items"]["result"]
            .as_array()
            .is_some_and(|items| items.iter().any(|entry| entry["id"] == body["item"]["id"]))
    );
    assert_eq!(body["selectedItemInfo"]["id"], body["item"]["id"]);
    assert_eq!(body["itemDownload"]["filename"], "guide.md");
    assert_eq!(
        body["itemDownload"]["size"].as_u64(),
        Some(body["downloadedText"].as_str().unwrap().len() as u64)
    );
    assert!(
        body["downloadedText"]
            .as_str()
            .is_some_and(|text| text.contains("cobalt retrieval marker"))
    );
    assert!(body["itemLogs"]["result"].is_array());
    assert!(
        body["itemChunks"]["result"]
            .as_array()
            .is_some_and(|chunks| chunks.iter().any(|chunk| {
                chunk["item"]["key"] == "guide.md"
                    && chunk["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("cobalt retrieval marker"))
            }))
    );
    assert!(matches!(
        body["syncedTerminal"]["status"].as_str(),
        Some("completed" | "skipped")
    ));
    assert!(body["queuedItem"]["id"].as_str().is_some());
    assert_eq!(body["jobCreated"]["id"], body["jobInfo"]["id"]);
    assert_eq!(body["jobCreated"]["id"], body["jobCancelled"]["id"]);
    assert!(body["jobLogs"]["result"].is_array());
    assert!(
        body["jobs"]["result"]
            .as_array()
            .is_some_and(|jobs| jobs.iter().any(|job| job["id"] == body["jobCreated"]["id"]))
    );
    assert_eq!(body["directQueuedUpload"]["status"], "queued");
    assert_eq!(body["directItem"]["status"], "completed");
    assert!(
        body["directSearch"]["chunks"]
            .as_array()
            .is_some_and(|chunks| chunks.iter().any(|chunk| chunk["text"]
                .as_str()
                .is_some_and(|text| text.contains("amber direct binding marker"))))
    );
    assert!(
        body["multiSearch"]["chunks"]
            .as_array()
            .is_some_and(|chunks| chunks.iter().all(|chunk| chunk["instance_id"]
                .as_str()
                .is_some_and(|id| id == "docs" || id == "direct")))
    );
    assert_eq!(
        body["directChat"]["choices"][0]["message"]["content"],
        "answer"
    );
    assert_eq!(
        body["multiChat"]["choices"][0]["message"]["content"],
        "answer"
    );
    for stream in ["directChatSse", "multiChatSse"] {
        let sse = body[stream].as_str().unwrap();
        assert!(sse.starts_with("event: chunks\ndata: "));
        assert!(sse.contains("hel"));
        assert!(sse.contains("lo"));
        assert!(sse.ends_with("data: [DONE]\n\n"));
    }
    assert_eq!(body["trashInfo"]["id"], "trash");
    assert!(body["aiGatewayLogId"].is_null());
    assert_eq!(body["markdown"]["format"], "markdown");
    assert!(
        body["markdown"]["data"]
            .as_str()
            .is_some_and(|text| text.contains("Markdown gate"))
    );
    assert_eq!(body["transformed"]["format"], "markdown");
    assert!(
        body["transformed"]["data"]
            .as_str()
            .is_some_and(|text| text.contains("handle transform gate"))
    );
    assert_eq!(body["markdownBatch"].as_array().unwrap().len(), 2);
    assert_eq!(body["markdownBatch"][0]["format"], "markdown");
    assert_eq!(body["markdownBatch"][1]["format"], "error");
    assert_eq!(body["transformedBatch"].as_array().unwrap().len(), 2);
    assert!(
        body["transformedBatch"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["format"] == "markdown")
    );
    assert!(
        body["supported"]
            .as_array()
            .is_some_and(|formats| formats.iter().any(|format| format["extension"] == ".pdf"))
    );

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
    let _ = chat_shutdown.send(());
    chat_task.await.unwrap();
    if let Some((_, embedding_shutdown, embedding_task)) = embedding_fixture {
        let _ = embedding_shutdown.send(());
        embedding_task.await.unwrap();
    }
}
