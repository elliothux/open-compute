//! Real pinned-workerd P3.2 Service Binding authority, routing, and lifecycle gate.

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use bytes::Bytes;
use futures::{StreamExt, stream};
use open_compute_artifacts::{
    ArtifactCache, ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{
    CacheConfig, PlatformConfig, Redactor, RequestId, RuntimeConfig, StartupId, StorageConfig,
    SystemClock,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, SupervisorState, WorkerdSupervisor,
    WorkerdSupervisorOptions, verify_runtime_binary,
};
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkerdTransport, bind_runtime_source, serve_runtime_source,
};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, serve_binding_backend_with_assets,
};
use open_compute_storage::{PlatformStorage, WorkerRepository};
use open_compute_workers::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, BundleLimits, CanonicalBundle,
    CreateDeploymentOutcome, CreateDeploymentRequest, DeploymentAssets, DeploymentContent,
    DeploymentController, DeploymentPins, DeploymentServiceInput, HtmlHandling, ModuleInput,
    ModuleType, NotFoundHandling, ResourcePins, RunWorkerFirst, RuntimeSource, RuntimeValidator,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CALLER_SOURCE: &str = r#"
import { WorkerEntrypoint } from "cloudflare:workers";

export default class Caller extends WorkerEntrypoint {
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/asset") return this.env.TARGET.fetch("https://not-a-route.example/asset.txt");
    if (path === "/asset-only") return this.env.ASSET_ONLY.fetch("https://private.example/only.txt");
    if (path === "/target-fetch") return this.env.TARGET.fetch("https://preserved.example/worker");
    if (path === "/named-fetch") return this.env.NAMED.fetch("https://named.example/path");
    if (path === "/object-fetch") return this.env.OBJECT.fetch("https://object.example/path");
    if (path === "/default-rpc") return Response.json(await this.env.TARGET.identify());
    if (path === "/named-rpc") return new Response(String(await this.env.NAMED.multiply(6, 7)));
    if (path === "/asset-only-rpc") {
      try { await this.env.ASSET_ONLY.identify(); return new Response("unexpected"); }
      catch (error) { return new Response(String(error?.message)); }
    }
    if (path === "/background") {
      return new Response(String(await this.env.TARGET.background()));
    }
    if (path === "/failure") {
      try { await this.env.TARGET.failure(); return new Response("unexpected"); }
      catch (error) { return new Response(String(error?.message)); }
    }
    if (path === "/capability") {
      const target = await this.env.TARGET.capability("cap");
      const duplicate = target.dup();
      target[Symbol.dispose]();
      const first = await duplicate.ping("one");
      const callback = await duplicate.callback(value => `callback:${value}`, "ok");
      const nested = await duplicate.nested();
      const second = await nested.label;
      nested[Symbol.dispose]();
      duplicate[Symbol.dispose]();
      return Response.json({ first, callback, second });
    }
    if (path === "/hold") {
      const target = this.env.TARGET;
      return new Response(new ReadableStream({
        async start(controller) {
          const held = await target.capability("held");
          controller.enqueue(new TextEncoder().encode("ready\n"));
          await scheduler.wait(500);
          controller.enqueue(new TextEncoder().encode(await held.ping("later")));
          held[Symbol.dispose]();
          controller.close();
        },
      }));
    }
    if (path === "/limit") {
      try { await this.env.SELF.recurse(16); return new Response("unexpected"); }
      catch (error) { return new Response(String(error?.message)); }
    }
    return new Response("caller");
  }

  recurse(remaining) {
    return remaining === 0 ? "done" : this.env.SELF.recurse(remaining - 1);
  }
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_services_real_runtime_authority_routing_budget_and_lifecycle_matrix() {
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
    let cache = Arc::new(
        ArtifactCache::open(
            storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let runtime = verify_runtime_binary(&lock, &workerd, Duration::from_secs(10), &Redactor::new())
        .await
        .expect("formal pinned runtime");
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let deployment_pins = DeploymentPins::new();
    let service_invocations = Arc::new(ServiceInvocationRegistry::new(
        storage.clone(),
        deployment_pins.clone(),
    ));
    let (shutdown, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = deployment_pins.clone();
        let service_invocations = service_invocations.clone();
        let asset_service = Arc::new(AssetBindingService::new(
            storage.clone(),
            artifacts.clone(),
            cache,
            pins.clone(),
        ));
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
                asset_service,
                service_invocations,
                None,
                None,
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
            version: "p3-services-product".to_owned(),
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
                    .join("p3-services-product.lease"),
            ),
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
    let repository = WorkerRepository::new(storage.db());
    let (target, _) = repository
        .create_worker(
            account,
            "service-target",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let (asset_only, _) = repository
        .create_worker(account, "asset-target", RequestId::generate(), 2, 1_000_000)
        .unwrap();
    let (caller, _) = repository
        .create_worker(
            account,
            "service-caller",
            RequestId::generate(),
            3,
            1_000_000,
        )
        .unwrap();
    let (object_target, _) = repository
        .create_worker(
            account,
            "object-service-target",
            RequestId::generate(),
            4,
            1_000_000,
        )
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = DeploymentController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );

    let target_v1 = deploy(
        &controller,
        worker_request(
            account,
            target.id,
            "target-v1",
            &target_source("v1"),
            WorkerRequestOptions {
                assets: Some(single_asset(&artifacts, "/asset.txt", b"asset-v1").await),
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("target-v1"))]),
                services: BTreeMap::new(),
                promote: true,
                now_ms: 10,
            },
        ),
    )
    .await;
    let asset_deployment = deploy(
        &controller,
        assets_request(
            account,
            asset_only.id,
            "asset-only",
            single_asset(&artifacts, "/only.txt", b"only-asset").await,
            11,
        ),
    )
    .await;
    let object_deployment = deploy(
        &controller,
        worker_request(
            account,
            object_target.id,
            "object-target-v1",
            r#"export default {
  fetch(request, env) {
    return new Response(`object:${env.OWNER}:${new URL(request.url).hostname}`);
  }
};"#,
            WorkerRequestOptions {
                assets: None,
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("object-v1"))]),
                services: BTreeMap::new(),
                promote: true,
                now_ms: 12,
            },
        ),
    )
    .await;
    let services = BTreeMap::from([
        (
            "TARGET".to_owned(),
            DeploymentServiceInput {
                target_worker_id: target.id,
                entrypoint: None,
            },
        ),
        (
            "NAMED".to_owned(),
            DeploymentServiceInput {
                target_worker_id: target.id,
                entrypoint: Some("NamedApi".to_owned()),
            },
        ),
        (
            "ASSET_ONLY".to_owned(),
            DeploymentServiceInput {
                target_worker_id: asset_only.id,
                entrypoint: None,
            },
        ),
        (
            "OBJECT".to_owned(),
            DeploymentServiceInput {
                target_worker_id: object_target.id,
                entrypoint: None,
            },
        ),
        (
            "SELF".to_owned(),
            DeploymentServiceInput {
                target_worker_id: caller.id,
                entrypoint: None,
            },
        ),
    ]);
    let caller_deployment = deploy(
        &controller,
        worker_request(
            account,
            caller.id,
            "caller-v1",
            CALLER_SOURCE,
            WorkerRequestOptions {
                assets: None,
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("caller"))]),
                services,
                promote: true,
                now_ms: 13,
            },
        ),
    )
    .await;

    let first_asset = dispatch(&transport, account, caller.id, &caller_deployment, "/asset").await;
    let first_asset_status = first_asset.status();
    let first_asset_body = body(first_asset).await;
    assert_eq!(
        first_asset_status,
        StatusCode::OK,
        "first Service call failed: {}; diagnostics={:?}",
        String::from_utf8_lossy(&first_asset_body),
        supervisor.last_diagnostics(),
    );
    assert_eq!(first_asset_body.as_ref(), b"asset-v1");
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/object-fetch",
        "object:object-v1:object.example",
    )
    .await;
    wait_pin_count(
        &deployment_pins,
        &service_invocations,
        object_deployment.id,
        0,
    )
    .await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/asset-only",
        "only-asset",
    )
    .await;
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/target-fetch",
        "fetch-v1:preserved.example:/worker",
    )
    .await;
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/named-fetch",
        "named-fetch-v1:named.example",
    )
    .await;
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    let identity = dispatch(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/default-rpc",
    )
    .await;
    assert_eq!(identity.status(), StatusCode::OK);
    let identity: serde_json::Value = serde_json::from_slice(&body(identity).await).unwrap();
    assert_eq!(
        identity,
        serde_json::json!({"version":"v1","owner":"target-v1"})
    );
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/named-rpc",
        "42",
    )
    .await;
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/asset-only-rpc",
        "SERVICE_ENTRYPOINT_NOT_FOUND",
    )
    .await;
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/background",
        "background-v1",
    )
    .await;
    assert_eq!(deployment_pins.count(target_v1.id), 1);
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/failure",
        "business-failure-v1",
    )
    .await;
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(
        &deployment_pins,
        &service_invocations,
        caller_deployment.id,
        1,
    )
    .await;
    let capability = dispatch(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/capability",
    )
    .await;
    let capability: serde_json::Value = serde_json::from_slice(&body(capability).await).unwrap();
    assert_eq!(
        capability,
        serde_json::json!({
            "first":"v1:cap:one",
            "callback":"callback:ok",
            "second":"label:v1:cap:nested",
        }),
    );
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(
        &deployment_pins,
        &service_invocations,
        caller_deployment.id,
        1,
    )
    .await;

    let target_v2 = deploy(
        &controller,
        worker_request(
            account,
            target.id,
            "target-v2",
            &target_source("v2"),
            WorkerRequestOptions {
                assets: Some(single_asset(&artifacts, "/asset.txt", b"asset-v2").await),
                vars: BTreeMap::from([("OWNER".to_owned(), serde_json::json!("target-v2"))]),
                services: BTreeMap::new(),
                promote: false,
                now_ms: 14,
            },
        ),
    )
    .await;
    let held = dispatch(&transport, account, caller.id, &caller_deployment, "/hold").await;
    assert_eq!(held.status(), StatusCode::OK);
    let mut held_body = held.into_body().into_data_stream();
    let ready = held_body.next().await.unwrap().unwrap();
    assert_eq!(ready.as_ref(), b"ready\n");
    let held_counts = service_invocations.counts();
    assert_eq!((held_counts.0, held_counts.2), (1, 1));
    assert!(held_counts.1 <= 1);
    assert_eq!(deployment_pins.count(target_v1.id), 1);
    assert_eq!(deployment_pins.count(caller_deployment.id), 2);
    repository
        .promote(
            account,
            target.id,
            target_v2.id,
            Some(target_v1.id),
            RequestId::generate(),
            15,
        )
        .unwrap();
    let mut held_tail = Vec::new();
    while let Some(chunk) = held_body.next().await {
        held_tail.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(held_tail, b"v1:held:later");
    wait_pin_count(&deployment_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(
        &deployment_pins,
        &service_invocations,
        caller_deployment.id,
        1,
    )
    .await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/asset",
        "asset-v2",
    )
    .await;
    let identity = dispatch(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/default-rpc",
    )
    .await;
    let identity: serde_json::Value = serde_json::from_slice(&body(identity).await).unwrap();
    assert_eq!(
        identity,
        serde_json::json!({"version":"v2","owner":"target-v2"})
    );
    assert_eq!(deployment_pins.count(target_v1.id), 0);
    wait_pin_count(&deployment_pins, &service_invocations, target_v2.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_deployment,
        "/limit",
        "SERVICE_LIMIT_EXCEEDED",
    )
    .await;
    wait_service_counts(&service_invocations, (0, 0, 0)).await;
    assert!(deployment_pins.count(asset_deployment.id) <= 1);

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
}

fn target_source(version: &str) -> String {
    format!(
        r#"
import {{ RpcTarget, WorkerEntrypoint }} from "cloudflare:workers";
const VERSION = {version:?};
class Capability extends RpcTarget {{
  constructor(value) {{ super(); this.value = value; }}
  get label() {{ return `label:${{this.value}}`; }}
  ping(suffix) {{ return `${{this.value}}:${{suffix}}`; }}
  callback(callback, value) {{ return callback(value); }}
  nested() {{ return new Capability(`${{this.value}}:nested`); }}
}}
export default class Target extends WorkerEntrypoint {{
  fetch(request) {{
    const url = new URL(request.url);
    return new Response(`fetch-${{VERSION}}:${{url.hostname}}:${{url.pathname}}`);
  }}
  identify() {{ return {{ version: VERSION, owner: this.env.OWNER }}; }}
  background() {{
    this.ctx.waitUntil(scheduler.wait(750));
    return `background-${{VERSION}}`;
  }}
  failure() {{ throw new Error(`business-failure-${{VERSION}}`); }}
  capability(name) {{ return new Capability(`${{VERSION}}:${{name}}`); }}
}}
export class NamedApi extends WorkerEntrypoint {{
  fetch(request) {{ return new Response(`named-fetch-${{VERSION}}:${{new URL(request.url).hostname}}`); }}
  multiply(left, right) {{ return left * right; }}
}}
"#,
    )
}

fn worker_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    source: &str,
    options: WorkerRequestOptions,
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
            assets: options.assets,
        },
        compatibility_date: "2026-08-26".to_owned(),
        compatibility_flags: Vec::new(),
        vars: options.vars,
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: options.services,
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: None,
        limits: serde_json::json!({"profile":"default"}),
        promote: options.promote,
        request_id: RequestId::generate(),
        now_ms: options.now_ms,
    }
}

struct WorkerRequestOptions {
    assets: Option<DeploymentAssets>,
    vars: BTreeMap<String, serde_json::Value>,
    services: BTreeMap<String, DeploymentServiceInput>,
    promote: bool,
    now_ms: i64,
}

fn assets_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    assets: DeploymentAssets,
    now_ms: i64,
) -> CreateDeploymentRequest {
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: DeploymentContent::AssetsOnly { assets },
        compatibility_date: "2026-08-26".to_owned(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: None,
        limits: serde_json::json!({"profile":"default"}),
        promote: true,
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn single_asset(artifacts: &ArtifactStore, path: &str, content: &[u8]) -> DeploymentAssets {
    let digest = hex::encode(Sha256::digest(content));
    artifacts
        .put_verified(
            stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(content)) }),
            &digest,
            content.len() as u64,
        )
        .await
        .unwrap();
    DeploymentAssets {
        manifest: AssetManifestV1 {
            schema_version: 1,
            entries: vec![AssetEntryV1 {
                path: path.to_owned(),
                sha256: digest,
                size: content.len() as u64,
                content_type: "text/plain; charset=utf-8".to_owned(),
            }],
        },
        routing: AssetRoutingConfigV1 {
            schema_version: 1,
            binding: None,
            run_worker_first: RunWorkerFirst::All(false),
            html_handling: HtmlHandling::None,
            not_found_handling: NotFoundHandling::None,
            headers: Vec::new(),
            redirects: Vec::new(),
        },
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
) -> open_compute_storage::DeploymentRecord {
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    }
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    path: &str,
) -> axum::response::Response {
    transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                deployment_id: deployment.id,
                worker_code_sha256: hex::encode(deployment.worker_code_sha256),
                entrypoint: None,
                route_generation: 1,
                request_id: RequestId::generate(),
            },
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::HOST, "caller.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn assert_body(
    transport: &WorkerdTransport,
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    path: &str,
    expected: &str,
) {
    let response = dispatch(transport, account_id, worker_id, deployment, path).await;
    let status = response.status();
    let headers = response.headers().clone();
    let actual = body(response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{path}: headers={headers:?}; body={}",
        String::from_utf8_lossy(&actual),
    );
    assert_eq!(actual.as_ref(), expected.as_bytes(), "{path}");
}

async fn body(response: axum::response::Response) -> Bytes {
    to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap()
}

async fn wait_pin_count(
    pins: &DeploymentPins,
    registry: &ServiceInvocationRegistry,
    deployment: open_compute_core::DeploymentId,
    count: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while pins.count(deployment) != count {
        assert!(
            Instant::now() < deadline,
            "deployment {deployment} pin did not drain: actual={}; registry={:?}; pins={pins:?}",
            pins.count(deployment),
            registry.counts(),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_service_counts(
    registry: &ServiceInvocationRegistry,
    expected: (usize, usize, usize),
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while registry.counts() != expected {
        assert!(
            Instant::now() < deadline,
            "Service invocation registry did not drain: actual={:?}",
            registry.counts(),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
            "supervisor failed: {snapshot:?}"
        );
        assert!(Instant::now() < deadline, "supervisor did not become ready");
        tokio::time::timeout(Duration::from_millis(250), rx.changed())
            .await
            .ok();
    }
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
        mock.endpoint,
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

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
}
