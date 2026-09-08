//! Real pinned-workerd P3.2 Service Binding authority, routing, and lifecycle gate.

#[path = "../p3_services_support/mod.rs"]
mod p3_services_support;
mod websocket_handoff;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use bytes::Bytes;
use futures::{StreamExt, stream};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{BindingKind, CanonicalBindingConfig, CanonicalPermissions, RequestId};
use open_compute_service::runtime_bridge::{DispatchTarget, WorkerdTransport};
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_storage::WorkerRepository;
use open_compute_workers::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, BundleLimits, CanonicalBundle,
    CreateVersionOutcome, CreateVersionRequest, HtmlHandling, ModuleInput, ModuleType,
    NotFoundHandling, RunWorkerFirst, RuntimeValidator, VersionAssets, VersionBindingInput,
    VersionContent, VersionController, VersionPins, VersionServiceInput,
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
    if (path === "/socket") return this.env.TARGET.fetch(request);
    if (path === "/named-socket") return this.env.NAMED.fetch(request);
    if (path === "/request-body") return this.env.TARGET.fetch("https://target.example/body", {
      method: "POST", body: new ReadableStream({ start(controller) {
        controller.enqueue(new TextEncoder().encode("streamed request"));
        controller.close();
      } }),
    });
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
    if (path === "/props") return Response.json(await this.env.TARGET.bindingProps());
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

mod lifecycle_matrix;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_services_real_runtime_authority_routing_budget_and_lifecycle_matrix() {
    lifecycle_matrix::run().await;
}

fn target_source(version: &str) -> String {
    format!(
        r#"
import {{ DurableObject, RpcTarget, WorkerEntrypoint }} from "cloudflare:workers";
const VERSION = {version:?};
function echoSocket(env, request) {{
  return env.SOCKETS.getByName("service-websocket").fetch(request);
}}
export class SocketRoom extends DurableObject {{
  fetch() {{
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["service"]);
    return new Response(null, {{ status: 101, webSocket: pair[0] }});
  }}
  webSocketMessage(socket, message) {{ socket.send(message); }}
  webSocketClose(socket, code, reason) {{ socket.close(code, reason); }}
}}
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
    if (request.headers.get("upgrade") === "websocket") return echoSocket(this.env, request);
    if (url.pathname === "/body") return request.text().then(body => new Response(body));
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
  bindingProps() {{ return this.ctx.props; }}
  background() {{
    this.ctx.waitUntil(scheduler.wait(750));
    return `background-${{VERSION}}`;
  }}
  failure() {{ throw new Error(`business-failure-${{VERSION}}`); }}
  capability(name) {{ return new Capability(`${{VERSION}}:${{name}}`); }}
}}
export class NamedApi extends WorkerEntrypoint {{
  fetch(request) {{ if (request.headers.get("upgrade") === "websocket") return echoSocket(this.env, request); return new Response(`named-fetch-${{VERSION}}:${{new URL(request.url).hostname}}`); }}
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
        bindings: options.bindings,
        services: options.services,
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: options
            .promote
            .then_some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms: options.now_ms,
    }
}

struct WorkerRequestOptions {
    assets: Option<VersionAssets>,
    vars: BTreeMap<String, serde_json::Value>,
    bindings: BTreeMap<String, VersionBindingInput>,
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
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
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
