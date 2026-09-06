//! Native public Loader through fixed Wrangler, v4 authority, and the supervised runtime.

use super::*;
use serde_json::json;

const SCRIPT: &str = "p6-wrangler-resource-gate";
const SOURCE: &str = r#"
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
let parentCount = 0;
let tailFailure;
const label = "__LABEL__";
function code(value) {
  return {
    compatibilityDate: "2026-08-30",
    mainModule: "child.js",
    globalOutbound: null,
    modules: { "child.js": `
      let count = 0;
      export default { fetch() {
        console.log("loader-child:" + ${JSON.stringify(value)});
        return Response.json({ label: ${JSON.stringify(value)}, count: ++count });
      } };
    ` },
  };
}
function facetCode() {
  return {
    compatibilityDate: "2026-08-30",
    mainModule: "facet.js",
    globalOutbound: null,
    modules: { "facet.js": `
      import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
      export class Child extends DurableObject {
        async increment() {
          const count = (await this.ctx.storage.get("count") ?? 0) + 1;
          await this.ctx.storage.put("count", count);
          console.log("loader-facet:" + count);
          return count;
        }
      }
      export class Named extends WorkerEntrypoint {
        echo(value) { return { value, marker: this.ctx.props.marker }; }
      }
    ` },
  };
}
export class LoaderParent extends DurableObject {
  async recordTail() {
    await this.ctx.storage.put("tail-count", (await this.ctx.storage.get("tail-count") ?? 0) + 1);
  }
  async tailCount() { return await this.ctx.storage.get("tail-count") ?? 0; }
  async increment() {
    const stub = this.env.LOADER.get("facet-worker", facetCode);
    const child = this.ctx.facets.get("child", () => ({
      class: stub.getDurableObjectClass("Child"), id: "child-v1",
    }));
    return child.increment();
  }
}
export class TailReceiver extends WorkerEntrypoint {
  async tail(events) {
    try {
      if (events.some(event => event.logs.some(log => String(log.message).includes("loader-child:tail")))) {
        await this.env.OBJECTS.getByName("tail-receiver").recordTail();
      }
    }
    catch (error) { tailFailure = String(error); }
  }
}
export default {
  async fetch(request, env, ctx) {
    const path = new URL(request.url).pathname;
    if (path.endsWith("/facet")) {
      return Response.json({ count: await env.OBJECTS.getByName("parent").increment() });
    }
    if (path.endsWith("/tail")) {
      const stub = env.LOADER.load({ ...code("tail"), tails: [ctx.exports.TailReceiver(Object.freeze({ props: Object.freeze({}) }))] });
      return stub.getEntrypoint().fetch("https://tail.invalid");
    }
    if (path.endsWith("/tail-status")) {
      if (tailFailure) return Response.json({ tailFailure });
      return Response.json({ count: await env.OBJECTS.getByName("tail-receiver").tailCount() });
    }
    if (path.endsWith("/rpc")) {
      const stub = env.LOADER.load(facetCode());
      const entrypoint = stub.getEntrypoint("Named", { props: { marker: "scoped" } });
      return Response.json(await entrypoint.echo({ nested: [1, true, null] }));
    }
    if (path.endsWith("/egress")) {
      const stub = env.LOADER.load({
        compatibilityDate: "2026-08-30", mainModule: "main.js",
        modules: { "main.js": `
          export default { async fetch(request) {
            try { await fetch(request.url); return Response.json({ rejected: false }); }
            catch { return Response.json({ rejected: true }); }
          } };
        ` },
      });
      return stub.getEntrypoint().fetch(new URL(request.url).searchParams.get("url"));
    }
    if (path.endsWith("/wasm")) {
      const stub = env.LOADER.load({
        compatibilityDate: "2026-08-30", mainModule: "main.js", globalOutbound: null,
        modules: {
          "main.js": `
            import module from "./add.wasm";
            export default { async fetch() {
              const instance = await WebAssembly.instantiate(module);
              return Response.json({ sum: instance.exports.add(5, 7) });
            } };
          `,
          "add.wasm": { wasm: new Uint8Array([
            0,97,115,109,1,0,0,0,1,7,1,96,2,127,127,1,127,3,2,1,0,
            7,7,1,3,97,100,100,0,0,10,9,1,7,0,32,0,32,1,106,11,
          ]) },
        },
      });
      return stub.getEntrypoint().fetch("https://wasm.invalid");
    }
    if (path.endsWith("/python")) {
      const stub = env.LOADER.load({
        compatibilityDate: "2026-08-30", compatibilityFlags: ["python_workers"],
        mainModule: "main.py", globalOutbound: null,
        modules: { "main.py": { py: `
from workers import WorkerEntrypoint
class Default(WorkerEntrypoint):
  async def echo(self, value):
    return "python:" + value
` } },
      });
      return Response.json({ value: await stub.getEntrypoint().echo("module") });
    }
    if (path.endsWith("/invalid")) {
      let limitsRejected = false, delegationRejected = false;
      try {
        await env.LOADER.load({ ...code("limited"), limits: {} })
          .getEntrypoint().fetch("https://limited.invalid");
      }
      catch { limitsRejected = true; }
      try { env.LOADER.load({ ...code("delegated"), env: { LOADER: env.LOADER } }); }
      catch { delegationRejected = true; }
      return Response.json({ limitsRejected, delegationRejected, keys: Object.keys(env).sort() });
    }
    const child = env.LOADER.get("shared-child", () => code("shared"));
    const other = env.OTHER.get("shared-child", () => code("other"));
    const value = await (await child.getEntrypoint().fetch("https://child.invalid")).json();
    const independent = await (await other.getEntrypoint().fetch("https://child.invalid")).json();
    return Response.json({ label, parentCount: ++parentCount, child: value, other: independent });
  },
};
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_loader_native_binding_versions_delete_and_restart() {
    let mut fixture = Fixture::new().await;
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };
    let portable = repo_root().join("test/conformance/fixtures/workers/dynamic-loader");
    let contract: Value =
        serde_json::from_slice(&fs::read(portable.join("contract.json")).unwrap()).unwrap();
    let mut config: Value =
        serde_json::from_slice(&fs::read(fixture.project.join("wrangler.jsonc")).unwrap()).unwrap();
    config["worker_loaders"] = json!([{ "binding": "LOADER" }]);
    fs::write(
        fixture.project.join("wrangler.jsonc"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    fs::copy(
        portable.join("src/index.ts"),
        fixture.project.join("index.ts"),
    )
    .unwrap();
    assert_success(&command.run(&["deploy", "--config", "wrangler.jsonc"]).await);
    let (status, actual) = invoke(&client, &fixture, SCRIPT, "").await;
    assert_eq!(status, 200, "{actual}");
    assert_eq!(actual, contract["observations"][0]["expect"]["json"]);
    let (_, settings) = api(&client, &fixture, SCRIPT, "/settings", "GET", None).await;
    assert!(json_contains(
        &settings,
        "bindings",
        &json!([
            { "name": "LOADER", "type": "worker_loader" }
        ])
    ));

    config["worker_loaders"] = json!([{ "binding": "LOADER" }, { "binding": "OTHER" }]);
    config["durable_objects"] = json!({
        "bindings": [{ "name": "OBJECTS", "class_name": "LoaderParent" }]
    });
    config["migrations"] =
        json!([{ "tag": "loader-parent", "new_sqlite_classes": ["LoaderParent"] }]);
    config["observability"] = json!({ "enabled": true, "head_sampling_rate": 1 });
    fs::write(
        fixture.project.join("wrangler.jsonc"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    deploy_source(&command, "one").await;
    let first = active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    assert_state(&client, &fixture, "one", 1, "shared", 1).await;
    let (status, invalid) = invoke(&client, &fixture, SCRIPT, "/invalid").await;
    assert_eq!(status, 200);
    assert_eq!(
        invalid,
        json!({
            "limitsRejected": true, "delegationRejected": true, "keys": ["LOADER", "OBJECTS", "OTHER"]
        })
    );
    let (status, rpc) = invoke(&client, &fixture, SCRIPT, "/rpc").await;
    assert_eq!(status, 200, "{rpc}");
    assert_eq!(
        rpc,
        json!({ "value": { "nested": [1, true, null] }, "marker": "scoped" })
    );
    for address in [fixture.admin_addr, fixture.public_addr] {
        let (status, egress) = invoke(
            &client,
            &fixture,
            SCRIPT,
            &format!("/egress?url=http://{address}/health/live"),
        )
        .await;
        assert_eq!(status, 200, "{egress}");
        assert_eq!(egress, json!({ "rejected": true }));
    }
    let (status, wasm) = invoke(&client, &fixture, SCRIPT, "/wasm").await;
    assert_eq!(status, 200, "{wasm}");
    assert_eq!(wasm, json!({ "sum": 12 }));
    let (status, python) = invoke(&client, &fixture, SCRIPT, "/python").await;
    assert_eq!(status, 200, "{python}");
    assert_eq!(python, json!({ "value": "python:module" }));
    let (status, tail) = invoke(&client, &fixture, SCRIPT, "/tail").await;
    assert_eq!(status, 200, "{tail}");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (status, tail) = invoke(&client, &fixture, SCRIPT, "/tail-status").await;
        assert_eq!(status, 200, "{tail}");
        if tail == json!({ "count": 1 }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "user tail did not receive child logs: {tail}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_facet(&client, &fixture, 1).await;
    deploy_source(&command, "two").await;
    assert_state(&client, &fixture, "two", 1, "shared", 2).await;
    assert_facet(&client, &fixture, 2).await;
    promote(&client, &fixture, &first).await;
    assert_state(&client, &fixture, "one", 2, "shared", 3).await;
    promote(&client, &fixture, &first).await;
    assert_state(&client, &fixture, "one", 3, "shared", 4).await;
    assert_facet(&client, &fixture, 3).await;
    wait_child_log(&client, &fixture).await;

    // A second Script uses the same public binding names and cache ID in a disjoint namespace.
    config["name"] = json!("dynamic-independent");
    fs::write(
        fixture.project.join("independent.jsonc"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    assert_success(
        &command
            .run(&["deploy", "--config", "independent.jsonc"])
            .await,
    );
    let (status, independent) = invoke(&client, &fixture, "dynamic-independent", "").await;
    assert_eq!(status, 200);
    assert_eq!(
        independent["child"],
        json!({ "label": "shared", "count": 1 })
    );
    drop(command);

    fixture.process.stop().await;
    fixture.process.restart(&fixture.config, &fixture.log);
    wait_ready(
        &client,
        fixture.admin_addr,
        &mut fixture.process,
        &fixture.log,
    )
    .await;
    assert_state(&client, &fixture, "one", 1, "shared", 1).await;
    assert_facet(&client, &fixture, 4).await;
    // Existing admission retains executed Versions until the supervised generation exits:
    // an HTTP response alone cannot prove that background tenant work has drained.
    assert_eq!(
        api(&client, &fixture, SCRIPT, "", "DELETE", None).await.0,
        409
    );
    assert_state(&client, &fixture, "one", 2, "shared", 2).await;
    restart(&client, &mut fixture).await;
    let (status, deleted) = api(&client, &fixture, SCRIPT, "", "DELETE", None).await;
    assert_eq!(status, 200, "{deleted}");
    assert_eq!(invoke(&client, &fixture, SCRIPT, "").await.0, 404);
    let (status, _) = api(&client, &fixture, SCRIPT, "", "DELETE", None).await;
    assert_eq!(status, 404);
    let (_, independent) = invoke(&client, &fixture, "dynamic-independent", "").await;
    assert_eq!(
        independent["child"],
        json!({ "label": "shared", "count": 1 })
    );

    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };
    deploy_source(&command, "replacement").await;
    assert_state(&client, &fixture, "replacement", 1, "shared", 1).await;
    drop(command);
    restart(&client, &mut fixture).await;
    assert_eq!(
        api(&client, &fixture, SCRIPT, "", "DELETE", None).await.0,
        200
    );
    assert_eq!(
        api(&client, &fixture, "dynamic-independent", "", "DELETE", None)
            .await
            .0,
        200
    );
    fixture.process.stop().await;
    assert_clean_output(&fs::read(&fixture.log).unwrap_or_default());
    for address in [fixture.public_addr, fixture.admin_addr] {
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }
}

async fn restart(client: &platform_process::Client, fixture: &mut Fixture) {
    fixture.process.stop().await;
    fixture.process.restart(&fixture.config, &fixture.log);
    wait_ready(
        client,
        fixture.admin_addr,
        &mut fixture.process,
        &fixture.log,
    )
    .await;
}

async fn deploy_source(command: &WranglerCommand<'_>, label: &str) {
    fs::write(
        command.project.join("index.ts"),
        SOURCE.replace("__LABEL__", label),
    )
    .unwrap();
    assert_success(&command.run(&["deploy", "--config", "wrangler.jsonc"]).await);
}

async fn assert_facet(client: &platform_process::Client, fixture: &Fixture, count: u32) {
    let (status, value) = invoke(client, fixture, SCRIPT, "/facet").await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value, json!({ "count": count }));
}

async fn invoke(
    client: &platform_process::Client,
    fixture: &Fixture,
    script: &str,
    path: &str,
) -> (u16, Value) {
    let request = Request::builder()
        .uri(format!(
            "http://{}/__workers/{}/{script}/{}",
            fixture.public_addr,
            fixture.internal_account,
            path.trim_start_matches('/')
        ))
        .body(Body::empty())
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(30), client.request(request))
        .await
        .unwrap()
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024)
        .await
        .unwrap();
    let value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    (status, value)
}

async fn api(
    client: &platform_process::Client,
    fixture: &Fixture,
    script: &str,
    suffix: &str,
    method: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let request = Request::builder()
        .method(method)
        .uri(format!(
            "http://{}/client/v4/accounts/{}/workers/scripts/{script}{suffix}",
            fixture.admin_addr, fixture.public_account
        ))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |value| {
            Body::from(serde_json::to_vec(&value).unwrap())
        }))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), client.request(request))
        .await
        .unwrap()
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(Body::new(response.into_body()), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn promote(client: &platform_process::Client, fixture: &Fixture, version: &str) {
    let (status, value) = api(
        client,
        fixture,
        SCRIPT,
        "/deployments",
        "POST",
        Some(json!({
            "strategy": "percentage", "versions": [{ "version_id": version, "percentage": 100 }]
        })),
    )
    .await;
    assert_eq!(status, 200, "{value}");
}

async fn assert_state(
    client: &platform_process::Client,
    fixture: &Fixture,
    label: &str,
    parent_count: u32,
    child_label: &str,
    child_count: u32,
) {
    let (status, value) = invoke(client, fixture, SCRIPT, "").await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(
        value,
        json!({
            "label": label, "parentCount": parent_count,
            "child": { "label": child_label, "count": child_count },
            "other": { "label": "other", "count": child_count }
        })
    );
}

async fn wait_child_log(client: &platform_process::Client, fixture: &Fixture) {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let request = Request::builder().method("POST")
            .uri(format!("http://{}/client/v4/accounts/{}/workers/observability/telemetry/query", fixture.admin_addr, fixture.public_account))
            .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({
                "queryId": "loader-host-collector", "timeframe": { "from": now - 60_000, "to": now + 60_000 },
                "parameters": { "datasets": ["cloudflare-workers"], "filters": [] }, "view": "events", "limit": 2_000
            })).unwrap())).unwrap();
        let response = client.request(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(Body::new(response.into_body()), 8 * 1024 * 1024)
            .await
            .unwrap();
        if String::from_utf8_lossy(&bytes).contains("loader-child:shared") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "protected collector did not persist child logs"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
