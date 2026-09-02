//! Real pinned-workerd P3.2 Service Binding authority, routing, and lifecycle gate.

mod p3_services_support;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use bytes::Bytes;
use futures::{StreamExt, stream};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::RequestId;
use open_compute_service::runtime_bridge::{DispatchTarget, WorkerdTransport};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_storage::WorkerRepository;
use open_compute_workers::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, BundleLimits, CanonicalBundle,
    CreateVersionOutcome, CreateVersionRequest, HtmlHandling, ModuleInput, ModuleType,
    NotFoundHandling, RunWorkerFirst, RuntimeValidator, VersionAssets, VersionContent,
    VersionController, VersionPins, VersionServiceInput,
};
use p3_services_support::Harness;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
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
    if (path === "/connect" || path === "/connect-ipv6") {
      const ipv6 = path === "/connect-ipv6";
      const socket = this.env.TARGET.connect(
        ipv6 ? { hostname: "2606:4700:4700::1111", port: 7000 } : "service.invalid:7000",
        { allowHalfOpen: true },
      );
      await socket.opened;
      const writer = socket.writable.getWriter();
      await writer.write(new Uint8Array(ipv6 ? [10, 11, 12] : [7, 8, 9]));
      await writer.close();
      writer.releaseLock();
      const bytes = new Uint8Array(await new Response(socket.readable).arrayBuffer());
      await socket.close();
      return new Response(Array.from(bytes).join(","));
    }
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
    let harness = Harness::start("p3-services-product").await;
    let storage = harness.storage.clone();
    let artifacts = harness.artifacts.clone();
    let transport = harness.transport.clone();
    let supervisor = harness.supervisor.clone();
    let version_pins = harness.version_pins.clone();
    let service_invocations = harness.service_invocations.clone();

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
    let controller = VersionController::new(
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
    let asset_version = deploy(
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
    let object_version = deploy(
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
            VersionServiceInput {
                target_worker_id: target.id,
                entrypoint: None,
            },
        ),
        (
            "NAMED".to_owned(),
            VersionServiceInput {
                target_worker_id: target.id,
                entrypoint: Some("NamedApi".to_owned()),
            },
        ),
        (
            "ASSET_ONLY".to_owned(),
            VersionServiceInput {
                target_worker_id: asset_only.id,
                entrypoint: None,
            },
        ),
        (
            "OBJECT".to_owned(),
            VersionServiceInput {
                target_worker_id: object_target.id,
                entrypoint: None,
            },
        ),
        (
            "SELF".to_owned(),
            VersionServiceInput {
                target_worker_id: caller.id,
                entrypoint: None,
            },
        ),
    ]);
    let caller_version = deploy(
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

    let first_asset = dispatch(&transport, account, caller.id, &caller_version, "/asset").await;
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
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let connect = dispatch(&transport, account, caller.id, &caller_version, "/connect").await;
    let connect_status = connect.status();
    let connect_body = body(connect).await;
    assert_eq!(
        connect_status,
        StatusCode::OK,
        "Service connect failed: {}; diagnostics={:?}",
        String::from_utf8_lossy(&connect_body),
        supervisor.last_diagnostics(),
    );
    assert_eq!(connect_body.as_ref(), b"7,8,9");
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let connect_ipv6 = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/connect-ipv6",
    )
    .await;
    let connect_ipv6_status = connect_ipv6.status();
    let connect_ipv6_body = body(connect_ipv6).await;
    assert_eq!(
        connect_ipv6_status,
        StatusCode::OK,
        "IPv6 Service connect failed: {}; diagnostics={:?}",
        String::from_utf8_lossy(&connect_ipv6_body),
        supervisor.last_diagnostics(),
    );
    assert_eq!(connect_ipv6_body.as_ref(), b"10,11,12");
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/object-fetch",
        "object:object-v1:object.example",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, object_version.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/asset-only",
        "only-asset",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/target-fetch",
        "fetch-v1:preserved.example:/worker",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/named-fetch",
        "named-fetch-v1:named.example",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    let identity = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/default-rpc",
    )
    .await;
    assert_eq!(identity.status(), StatusCode::OK);
    let identity: serde_json::Value = serde_json::from_slice(&body(identity).await).unwrap();
    assert_eq!(
        identity,
        serde_json::json!({"version":"v1","owner":"target-v1"})
    );
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/named-rpc",
        "42",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/asset-only-rpc",
        "SERVICE_ENTRYPOINT_NOT_FOUND",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/background",
        "background-v1",
    )
    .await;
    assert_eq!(version_pins.count(target_v1.id), 1);
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/failure",
        "business-failure-v1",
    )
    .await;
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(&version_pins, &service_invocations, caller_version.id, 1).await;
    let capability = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
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
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(&version_pins, &service_invocations, caller_version.id, 1).await;

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
    let held = dispatch(&transport, account, caller.id, &caller_version, "/hold").await;
    assert_eq!(held.status(), StatusCode::OK);
    let mut held_body = held.into_body().into_data_stream();
    let ready = held_body.next().await.unwrap().unwrap();
    assert_eq!(ready.as_ref(), b"ready\n");
    let held_counts = service_invocations.counts();
    assert_eq!((held_counts.0, held_counts.2), (1, 1));
    assert!(held_counts.1 <= 1);
    assert_eq!(version_pins.count(target_v1.id), 1);
    assert_eq!(version_pins.count(caller_version.id), 2);
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
    wait_pin_count(&version_pins, &service_invocations, target_v1.id, 0).await;
    wait_pin_count(&version_pins, &service_invocations, caller_version.id, 1).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/asset",
        "asset-v2",
    )
    .await;
    let identity = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/default-rpc",
    )
    .await;
    let identity: serde_json::Value = serde_json::from_slice(&body(identity).await).unwrap();
    assert_eq!(
        identity,
        serde_json::json!({"version":"v2","owner":"target-v2"})
    );
    assert_eq!(version_pins.count(target_v1.id), 0);
    wait_pin_count(&version_pins, &service_invocations, target_v2.id, 0).await;
    assert_body(
        &transport,
        account,
        caller.id,
        &caller_version,
        "/limit",
        "SERVICE_LIMIT_EXCEEDED",
    )
    .await;
    wait_service_counts(&service_invocations, (0, 0, 0)).await;
    assert!(version_pins.count(asset_version.id) <= 1);

    harness.stop().await;
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
  async connect(socket) {{
    const reader = socket.readable.getReader();
    const writer = socket.writable.getWriter();
    const part = await reader.read();
    if (!part.done) await writer.write(part.value);
    await writer.close();
    writer.releaseLock();
    await reader.cancel();
    reader.releaseLock();
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
) -> CreateVersionRequest {
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
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: options.assets,
        },
        vars: options.vars,
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: options.services,
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: options.promote,
        request_id: RequestId::generate(),
        now_ms: options.now_ms,
    }
}

struct WorkerRequestOptions {
    assets: Option<VersionAssets>,
    vars: BTreeMap<String, serde_json::Value>,
    services: BTreeMap<String, VersionServiceInput>,
    promote: bool,
    now_ms: i64,
}

fn assets_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    assets: VersionAssets,
    now_ms: i64,
) -> CreateVersionRequest {
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::AssetsOnly { assets },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        promote: true,
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn single_asset(artifacts: &ArtifactStore, path: &str, content: &[u8]) -> VersionAssets {
    let digest = hex::encode(Sha256::digest(content));
    artifacts
        .put_verified(
            stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(content)) }),
            &digest,
            content.len() as u64,
        )
        .await
        .unwrap();
    VersionAssets {
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
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
) -> open_compute_storage::VersionRecord {
    match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

async fn dispatch(
    transport: &WorkerdTransport,
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    path: &str,
) -> axum::response::Response {
    transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
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
    version: &open_compute_storage::VersionRecord,
    path: &str,
    expected: &str,
) {
    let response = dispatch(transport, account_id, worker_id, version, path).await;
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
    pins: &VersionPins,
    registry: &ServiceInvocationRegistry,
    version: open_compute_core::VersionId,
    count: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while pins.count(version) != count {
        assert!(
            Instant::now() < deadline,
            "version {version} pin did not drain: actual={}; registry={:?}; pins={pins:?}",
            pins.count(version),
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
