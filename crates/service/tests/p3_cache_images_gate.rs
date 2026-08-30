//! Real pinned-workerd P3.3 Cache API, automatic cache, Images, and metadata gate.
//!
//! This matrix intentionally stays cohesive: every assertion shares one stock-workerd process,
//! immutable deployment graph, S3 fixture, and restart boundary so the Gate registry executes the
//! complete lifecycle exactly once rather than rebuilding equivalent state in separate tests.

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, header};
use base64::Engine as _;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use open_compute_artifacts::{
    ArtifactCache, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{
    CacheConfig, ImagesConfig, PlatformConfig, Redactor, RequestId, ResponseCacheConfig,
    RuntimeConfig, StartupId, StorageConfig, SystemClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::cache_backend::CacheBindingService;
use open_compute_service::images_backend::ImageBindingService;
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend_with_assets,
};
use open_compute_storage::{
    BuiltinBindingKind, CacheManager, PlatformStorage, WorkerRepository,
    deployment_runtime_features,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentCacheInput, DeploymentCachePolicyInput, DeploymentContent, DeploymentController,
    DeploymentImagesInput, DeploymentPins, DeploymentRuntimeFeatures, DeploymentServiceInput,
    DeploymentVersionMetadataInput, ModuleInput, ModuleType, ResourcePins, RuntimeSource,
    RuntimeValidator,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TARGET_SOURCE: &str = r##"
import { WorkerEntrypoint } from "cloudflare:workers";
let defaultCount = 0;
let namedCount = 0;
let rpcCount = 0;
const LABEL = "__LABEL__";
const PIXEL = "__PIXEL__";
function imageBytes() {
  const text = atob(PIXEL);
  return Uint8Array.from(text, value => value.charCodeAt(0));
}
export default class Main extends WorkerEntrypoint {
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/auto") {
      defaultCount += 1;
      return new Response(`${LABEL}:${defaultCount}`, {
        headers: { "cache-control": "max-age=120, stale-while-revalidate=30, stale-if-error=30", "cache-tag": "gate" },
      });
    }
    if (path === "/api-put") {
      await caches.default.put("https://cache-key.example/value", new Response(`stored-${LABEL}`, {
        headers: { "cache-control": "max-age=120", "cache-tag": "explicit", "etag": "\"v1\"" },
      }));
      return new Response("put");
    }
    if (path === "/api-match") {
      const value = await caches.default.match("https://cache-key.example/value");
      return value ?? new Response("missing", { status: 404 });
    }
    if (path === "/api-range") {
      const value = await caches.default.match(new Request("https://cache-key.example/value", {
        headers: { range: "bytes=1-3" },
      }));
      return value ?? new Response("missing", { status: 404 });
    }
    if (path === "/api-conditional") {
      const value = await caches.default.match(new Request("https://cache-key.example/value", {
        headers: { "if-none-match": "\"v1\"" },
      }));
      return value ?? new Response("missing", { status: 404 });
    }
    if (path === "/api-delete") {
      return Response.json({ deleted: await caches.default.delete("https://cache-key.example/value") });
    }
    if (path === "/ctx") {
      const value = await this.ctx.exports.Named.fetch(new Request("https://ctx.example/auto"));
      return new Response(await value.text());
    }
    if (path === "/purge") return Response.json(await this.ctx.cache.purge({ tags: ["gate"] }));
    if (path === "/images") {
      const bytes = imageBytes();
      const info = await this.env.IMAGES.info(new Blob([bytes]).stream());
      const output = await this.env.IMAGES.input(new Blob([bytes]).stream())
        .transform({ width: 4, height: 3, fit: "pad", background: "#102030ff" })
        .output({ format: "image/png" });
      const response = output.response();
      return Response.json({ ...info, contentType: output.contentType(), outputBytes: (await response.arrayBuffer()).byteLength });
    }
    if (path === "/version") return Response.json(this.env.VERSION);
    return new Response("target");
  }
  rpcValue() { rpcCount += 1; return rpcCount; }
}
export class Named extends WorkerEntrypoint {
  fetch() {
    namedCount += 1;
    return new Response(`${LABEL}-named:${namedCount}`, { headers: { "cache-control": "max-age=120" } });
  }
}
"##;

const CALLER_SOURCE: &str = r#"
import { WorkerEntrypoint } from "cloudflare:workers";
export default class Caller extends WorkerEntrypoint {
  fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/service") return this.env.TARGET.fetch("https://service.example/auto");
    if (path === "/rpc") return Promise.resolve(this.env.TARGET.rpcValue()).then(value => new Response(String(value)));
    return new Response("caller");
  }
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_cache_images_real_runtime_semantics_and_lifecycle_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
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
    let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default())
                .with_cache(artifact_cache.clone())
                .with_cache_fail_open(true);
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let cache_service = Arc::new(
        CacheBindingService::new(
            storage.clone(),
            artifacts.clone(),
            artifact_cache.clone(),
            ResponseCacheConfig::default(),
        )
        .unwrap(),
    );
    let cache_manager = cache_service.manager();
    let image_service = Arc::new(ImageBindingService::new(
        storage.clone(),
        ImagesConfig::default(),
    ));
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = deployment_pins.clone();
        let cache_service = cache_service.clone();
        let images = image_service.clone();
        let assets = Arc::new(AssetBindingService::new(
            storage.clone(),
            artifacts.clone(),
            artifact_cache.clone(),
            pins.clone(),
        ));
        let services = Arc::new(ServiceInvocationRegistry::new(storage.clone(), pins));
        async move {
            serve_binding_backend_with_assets(
                binding_listener,
                storage.clone(),
                auth,
                ResourcePins::new(),
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
                Some(cache_service),
                Some(images),
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
            version: "p3-cache-images-gate".to_owned(),
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
            lease_path: Some(
                storage
                    .data_dir()
                    .runtime_dir()
                    .join("p3-cache-images.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let target = repo
        .create_worker(account, "cache-target", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0;
    let caller = repo
        .create_worker(account, "cache-caller", RequestId::generate(), 2, 1_000_000)
        .unwrap()
        .0;
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = DeploymentController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let pixel = pixel_base64();
    let a = deploy(
        &controller,
        request(
            account,
            target.id,
            "a",
            &target_source("A", &pixel),
            BTreeMap::new(),
            features("release-A"),
            true,
            10,
        ),
        &supervisor,
    )
    .await;

    let first = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(first.0, 200, "first automatic cache response: {first:?}");
    assert_eq!(first.1, "A:1");
    assert_eq!(
        first.2.as_deref(),
        Some("MISS"),
        "binding generation claim: {:?}",
        binding_auth.claimed_generation_for_test()
    );
    assert!(first.3.is_none(), "Cache-Tag must not reach the client");
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        1,
        Duration::from_secs(5),
    )
    .await;
    let hit = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!((hit.1.as_str(), hit.2.as_deref()), ("A:1", Some("HIT")));
    let range_hit = dispatch_request(
        &transport,
        &repo,
        account,
        target.id,
        &a,
        Request::builder()
            .method(Method::GET)
            .uri("/auto")
            .header(header::HOST, "cache.example.test")
            .header(header::RANGE, "bytes=0-0")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        (range_hit.0, range_hit.1.as_str(), range_hit.2.as_deref()),
        (206, "A", Some("HIT"))
    );

    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &a, "/api-put")
            .await
            .1,
        "put"
    );
    let range = dispatch(&transport, &repo, account, target.id, &a, "/api-range").await;
    assert_eq!((range.0, range.1.as_str()), (206, "tor"));
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            target.id,
            &a,
            "/api-conditional"
        )
        .await
        .0,
        304
    );
    let version: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/version")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(version["id"], a.id.to_string());
    assert_eq!(version["tag"], "release-A");
    let image: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/images")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(
        (
            image["format"].as_str(),
            image["width"].as_u64(),
            image["height"].as_u64()
        ),
        (Some("png"), Some(2), Some(2))
    );
    assert_eq!(image["contentType"], "image/png");
    assert!(image["outputBytes"].as_u64().unwrap() > 0);

    let entries_before_ctx = cache_entries(&cache_manager, account, target.id);
    let ctx_first = dispatch(&transport, &repo, account, target.id, &a, "/ctx").await;
    assert_eq!(ctx_first.1, "A-named:1");
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_ctx + 1,
        Duration::from_secs(5),
    )
    .await;
    let ctx_hit = dispatch(&transport, &repo, account, target.id, &a, "/ctx").await;
    assert_eq!(ctx_hit.1, "A-named:1");

    let b = deploy(
        &controller,
        request(
            account,
            target.id,
            "b",
            &target_source("B", &pixel),
            BTreeMap::new(),
            features("release-B"),
            true,
            20,
        ),
        &supervisor,
    )
    .await;
    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &b, "/api-match")
            .await
            .1,
        "stored-A"
    );
    let b_miss = dispatch(&transport, &repo, account, target.id, &b, "/auto").await;
    assert_eq!(
        (b_miss.1.as_str(), b_miss.2.as_deref()),
        ("B:1", Some("MISS"))
    );
    repo.promote(
        account,
        target.id,
        a.id,
        Some(b.id),
        RequestId::generate(),
        30,
    )
    .unwrap();
    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &a, "/auto")
            .await
            .1,
        "A:1"
    );

    let shared_c = deploy(
        &controller,
        request(
            account,
            target.id,
            "shared-c",
            &target_source("C", &pixel),
            BTreeMap::new(),
            shared_features("release-C"),
            false,
            31,
        ),
        &supervisor,
    )
    .await;
    let entries_before_shared = cache_entries(&cache_manager, account, target.id);
    let shared_miss = dispatch(&transport, &repo, account, target.id, &shared_c, "/auto").await;
    assert_eq!(
        (shared_miss.1.as_str(), shared_miss.2.as_deref()),
        ("C:1", Some("MISS"))
    );
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_shared + 1,
        Duration::from_secs(5),
    )
    .await;
    let shared_d = deploy(
        &controller,
        request(
            account,
            target.id,
            "shared-d",
            &target_source("D", &pixel),
            BTreeMap::new(),
            shared_features("release-D"),
            false,
            32,
        ),
        &supervisor,
    )
    .await;
    let shared_hit = dispatch(&transport, &repo, account, target.id, &shared_d, "/auto").await;
    assert_eq!(
        (shared_hit.1.as_str(), shared_hit.2.as_deref()),
        ("C:1", Some("HIT"))
    );

    let services = BTreeMap::from([(
        "TARGET".to_owned(),
        DeploymentServiceInput {
            target_worker_id: target.id,
            entrypoint: None,
        },
    )]);
    let caller_deployment = deploy(
        &controller,
        request(
            account,
            caller.id,
            "caller",
            CALLER_SOURCE,
            services,
            DeploymentRuntimeFeatures::default(),
            true,
            40,
        ),
        &supervisor,
    )
    .await;
    let entries_before_service = cache_entries(&cache_manager, account, target.id);
    let service_first = dispatch(
        &transport,
        &repo,
        account,
        caller.id,
        &caller_deployment,
        "/service",
    )
    .await;
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_service + 1,
        Duration::from_secs(5),
    )
    .await;
    let service_hit = dispatch(
        &transport,
        &repo,
        account,
        caller.id,
        &caller_deployment,
        "/service",
    )
    .await;
    assert!(service_first.1.starts_with("A:"));
    assert_eq!(service_hit.1, service_first.1);
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            caller.id,
            &caller_deployment,
            "/rpc"
        )
        .await
        .1,
        "1"
    );
    assert_eq!(
        dispatch(
            &transport,
            &repo,
            account,
            caller.id,
            &caller_deployment,
            "/rpc"
        )
        .await
        .1,
        "2"
    );
    let purged: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/purge")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(purged["success"], true);
    assert!(purged["deleted"].as_u64().unwrap() >= 2);
    let entries_before_refill = cache_entries(&cache_manager, account, target.id);
    let after_purge = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(after_purge.2.as_deref(), Some("MISS"));
    wait_cache_entries(
        &cache_manager,
        account,
        target.id,
        entries_before_refill + 1,
        Duration::from_secs(5),
    )
    .await;
    let before_restart = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(
        (before_restart.1.as_str(), before_restart.2.as_deref()),
        (after_purge.1.as_str(), Some("HIT"))
    );
    let deleted: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/api-delete")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert_eq!(
        dispatch(&transport, &repo, account, target.id, &a, "/api-match")
            .await
            .0,
        404
    );

    open_image_session(
        &image_service,
        &storage,
        account,
        target.id,
        &a,
        &binding_auth.claimed_generation_for_test().unwrap(),
        &base64::engine::general_purpose::STANDARD
            .decode(&pixel)
            .unwrap(),
    )
    .await;
    assert_eq!(image_service.capacity().unwrap().active_sessions, 1);

    let old_pid = supervisor.snapshot().pid.unwrap();
    let old_source_fingerprint = source_auth.active_fingerprint().unwrap();
    let old_binding_fingerprint = binding_auth.active_fingerprint().unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, old_pid, Duration::from_secs(30)).await;
    assert_ne!(
        source_auth.active_fingerprint().as_deref(),
        Some(old_source_fingerprint.as_str())
    );
    assert_ne!(
        binding_auth.active_fingerprint().as_deref(),
        Some(old_binding_fingerprint.as_str())
    );
    let after_restart = dispatch(&transport, &repo, account, target.id, &a, "/auto").await;
    assert_eq!(
        (after_restart.1.as_str(), after_restart.2.as_deref()),
        (before_restart.1.as_str(), Some("HIT"))
    );
    let restarted_version: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/version")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(restarted_version, version);
    let restarted_image: serde_json::Value = serde_json::from_str(
        &dispatch(&transport, &repo, account, target.id, &a, "/images")
            .await
            .1,
    )
    .unwrap();
    assert_eq!(restarted_image["format"], "png");
    assert_eq!(restarted_image["contentType"], "image/png");
    assert!(restarted_image["outputBytes"].as_u64().unwrap() > 0);
    assert_eq!(image_service.capacity().unwrap().active_sessions, 0);

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
}

fn features(tag: &str) -> DeploymentRuntimeFeatures {
    DeploymentRuntimeFeatures {
        cache: DeploymentCacheInput {
            default: DeploymentCachePolicyInput {
                enabled: true,
                cross_version_cache: false,
            },
            entrypoints: BTreeMap::from([(
                "Named".to_owned(),
                DeploymentCachePolicyInput {
                    enabled: true,
                    cross_version_cache: false,
                },
            )]),
        },
        images: Some(DeploymentImagesInput {
            binding: "IMAGES".to_owned(),
        }),
        version_metadata: Some(DeploymentVersionMetadataInput {
            binding: "VERSION".to_owned(),
            tag: Some(tag.to_owned()),
        }),
    }
}

fn shared_features(tag: &str) -> DeploymentRuntimeFeatures {
    let mut features = features(tag);
    features.cache.default.cross_version_cache = true;
    for policy in features.cache.entrypoints.values_mut() {
        policy.cross_version_cache = true;
    }
    features
}

fn target_source(label: &str, pixel: &str) -> String {
    TARGET_SOURCE
        .replace("__LABEL__", label)
        .replace("__PIXEL__", pixel)
}

fn pixel_base64() -> String {
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255])))
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[allow(clippy::too_many_arguments)]
fn request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    source: &str,
    services: BTreeMap<String, DeploymentServiceInput>,
    runtime_features: DeploymentRuntimeFeatures,
    promote: bool,
    now_ms: i64,
) -> CreateDeploymentRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        compatibility_date: "2026-08-26".to_owned(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services,
        runtime_features,
        queue_consumers: Vec::new(),
        crons: None,
        limits: serde_json::json!({"profile":"default"}),
        promote,
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
    supervisor: &WorkerdSupervisor,
) -> open_compute_storage::DeploymentRecord {
    let result = controller
        .create_deployment(request)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "deployment failed: {error:?}; diagnostics={:?}",
                supervisor.last_diagnostics()
            )
        });
    match result {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected replay"),
    }
}

async fn dispatch(
    transport: &WorkerdTransport,
    repo: &WorkerRepository<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    uri: &str,
) -> (u16, String, Option<String>, Option<String>) {
    dispatch_request(
        transport,
        repo,
        account,
        worker,
        deployment,
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::HOST, "cache.example.test")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn dispatch_request(
    transport: &WorkerdTransport,
    repo: &WorkerRepository<'_>,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    request: Request<Body>,
) -> (u16, String, Option<String>, Option<String>) {
    let route_generation =
        i64::try_from(repo.get_worker(account, worker).unwrap().route_generation).unwrap();
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
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let cache_status = response
        .headers()
        .get("cf-cache-status")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cache_tag = response
        .headers()
        .get("cache-tag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body, cache_status, cache_tag)
}

async fn open_image_session(
    service: &ImageBindingService,
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    generation: &str,
    bytes: &[u8],
) {
    let (_, bindings) = deployment_runtime_features(storage.db(), deployment.id).unwrap();
    let descriptor = bindings
        .iter()
        .find(|binding| binding.kind == BuiltinBindingKind::Images)
        .unwrap()
        .descriptor_sha256;
    let response = service
        .handle(
            Request::builder()
                .method(Method::POST)
                .uri("/internal/images/v1/input")
                .header("x-open-compute-account-id", account.to_string())
                .header("x-open-compute-worker-id", worker.to_string())
                .header("x-open-compute-deployment-id", deployment.id.to_string())
                .header("x-open-compute-descriptor-sha256", hex::encode(descriptor))
                .header("x-open-compute-startup-generation", generation)
                .body(Body::from(bytes.to_vec()))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), 200);
}

async fn wait_running(supervisor: &WorkerdSupervisor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed,
            "supervisor failed: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        assert!(Instant::now() < deadline, "supervisor did not become ready");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
    }
}

async fn wait_pid_change(supervisor: &WorkerdSupervisor, old_pid: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut rx = supervisor.subscribe();
    loop {
        let snapshot = rx.borrow().clone();
        if snapshot.state == SupervisorState::Running && snapshot.pid != Some(old_pid) {
            return;
        }
        assert!(
            snapshot.state != SupervisorState::Failed,
            "supervisor failed during restart: {snapshot:?}; diagnostics={:?}",
            supervisor.last_diagnostics()
        );
        assert!(Instant::now() < deadline, "runtime did not restart");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
    }
}

async fn wait_cache_entries(
    manager: &CacheManager,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    minimum: u64,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let stats = manager
            .worker_stats(account, worker, wall_now_ms())
            .unwrap();
        if stats.entries >= minimum {
            return;
        }
        assert!(Instant::now() < deadline, "cache store did not commit");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn cache_entries(
    manager: &CacheManager,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
) -> u64 {
    manager
        .worker_stats(account, worker, wall_now_ms())
        .unwrap()
        .entries
}

fn wall_now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

fn runtime_config() -> RuntimeConfig {
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

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
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
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 32 * 1024 * 1024).unwrap())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
