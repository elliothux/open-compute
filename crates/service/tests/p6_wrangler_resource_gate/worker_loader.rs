//! Native public Loader through fixed Wrangler, v4 authority, and the supervised runtime.

use super::*;
use serde_json::json;

const SCRIPT: &str = "p6-wrangler-resource-gate";
const SOURCE: &str = r#"
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
let parentCount = 0;
let receivedTailCount = 0;
const label = "__LABEL__";
function code(value) {
  return {
    compatibilityDate: "2026-09-08",
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
    compatibilityDate: "2026-09-08",
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
    if (events.some(event => event.logs.some(log => String(log.message).includes("loader-child:tail")))) receivedTailCount++;
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
      return Response.json({ count: receivedTailCount });
    }
    if (path.endsWith("/rpc")) {
      const stub = env.LOADER.load(facetCode());
      const entrypoint = stub.getEntrypoint("Named", { props: { marker: "scoped" } });
      return Response.json(await entrypoint.echo({ nested: [1, true, null] }));
    }
    if (path.endsWith("/egress")) {
      const stub = env.LOADER.load({
        compatibilityDate: "2026-09-08", mainModule: "main.js",
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
        compatibilityDate: "2026-09-08", mainModule: "main.js", globalOutbound: null,
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
    if (path.endsWith("/invalid")) {
      let limitsRejected = false, delegationRejected = false;
      try {
        await env.LOADER.load({ ...code("limited"), limits: { cpuMs: 300001 } })
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

const TRANSFER_SOURCE: &str = r#"
function code() {
  return {
    compatibilityDate: "2026-09-08",
    mainModule: "child.js",
    globalOutbound: null,
    modules: { "child.js": `export default { fetch() { return new Response("ok"); } };` },
  };
}
export default {
  fetch(_request, env) {
    const transferError = value => {
      try { env.LOADER.load({ ...code(), env: { VALUE: value } }); return null; }
      catch (error) { return error?.name ?? "Error"; }
    };
    return Response.json({
      transferErrors: {
        d1: transferError(env.DB),
        kv: transferError(env.KV),
        queue: transferError(env.QUEUE),
        r2: transferError(env.BUCKET),
      },
      keys: Object.keys(env).sort(),
    });
  },
};
"#;

const FORWARD_SOURCE: &str = r#"
import { loadWorker } from "open-compute:worker-loader";
export default {
  async fetch(_request, env) {
    const stub = loadWorker(env.LOADER, {
      compatibilityDate: "2026-09-08",
      mainModule: "child.js",
      globalOutbound: null,
      modules: { "child.js": `
        export default { async fetch(_request, env) {
          await env.KV.put("forwarded-key", "kv-value");
          const kv = await env.KV.get("forwarded-key");
          const row = await env.DB.prepare("SELECT 42 AS answer").first();
          await env.BUCKET.put("forwarded-key", "r2-value");
          const object = await env.BUCKET.get("forwarded-key");
          const r2 = await object.text();
          await env.QUEUE.send({ source: "forwarded-child" });
          await env.KV.delete("forwarded-key");
          await env.BUCKET.delete("forwarded-key");
          return Response.json({ kv, d1: row.answer, r2, ordinary: env.ORDINARY });
        } };
      ` },
      env: {
        KV: env.KV,
        DB: env.DB,
        BUCKET: env.BUCKET,
        QUEUE: env.QUEUE,
        ORDINARY: "visible",
      },
    });
    return stub.getEntrypoint().fetch("https://forwarded.invalid");
  },
};
"#;

pub(super) async fn resource_limits_settings_clone_and_restart() {
    let mut fixture = Fixture::new().await;
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };
    let config_path = fixture.project.join("wrangler.jsonc");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["limits"] = json!({ "cpu_ms": 4_321, "subrequests": 321 });
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    assert_success(&command.run(&["deploy", "--config", "wrangler.jsonc"]).await);
    assert_eq!(invoke(&client, &fixture, SCRIPT, "").await.0, 200);

    let (status, settings) = api(&client, &fixture, SCRIPT, "/settings", "GET", None).await;
    assert_eq!(status, 200, "{settings}");
    assert_eq!(
        settings["result"]["limits"],
        json!({ "cpu_ms": 4_321, "subrequests": 321 })
    );
    let original = active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    let (status, patched) = patch_limits(&client, &fixture, Some(5_432), Some(432)).await;
    assert_eq!(status, 200, "{patched}");
    assert_eq!(
        patched["result"]["limits"],
        json!({ "cpu_ms": 5_432, "subrequests": 432 })
    );
    let replacement = active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    assert_ne!(replacement, original);
    assert_eq!(deployed_version(&client, &fixture).await, original);
    let (status, partial) = patch_limits(&client, &fixture, Some(6_543), None).await;
    assert_eq!(status, 200, "{partial}");
    assert_eq!(
        partial["result"]["limits"],
        json!({ "cpu_ms": 6_543, "subrequests": 432 })
    );
    let partial_replacement =
        active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    assert_ne!(partial_replacement, replacement);
    let (status, bound) = patch_settings(
        &client,
        &fixture,
        json!({"bindings":[{"name":"LABEL","type":"plain_text","text":"first"}]}),
    )
    .await;
    assert_eq!(status, 200, "{bound}");
    assert_eq!(
        bound["result"]["bindings"],
        json!([{"name":"LABEL","type":"plain_text","text":"first"}])
    );
    let bound_version = active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    assert_ne!(bound_version, partial_replacement);
    let (status, inherited) = patch_settings(
        &client,
        &fixture,
        json!({"bindings":[{"name":"LABEL","type":"inherit"}]}),
    )
    .await;
    assert_eq!(status, 200, "{inherited}");
    assert_eq!(inherited["result"]["bindings"], bound["result"]["bindings"]);
    let (status, rejected) = patch_settings(
        &client,
        &fixture,
        json!({"bindings":[{"name":"MISSING","type":"inherit"}]}),
    )
    .await;
    assert_eq!(status, 400, "{rejected}");
    let (status, removed) = patch_settings(&client, &fixture, json!({"bindings":[]})).await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["result"]["bindings"], json!([]));
    assert_success(
        &command
            .run(&["d1", "create", D1_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let listed = command
        .run(&["d1", "list", "--json", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let d1_id = json_stdout(&listed)
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == D1_NAME)
        .and_then(|item| item["uuid"].as_str())
        .unwrap()
        .to_owned();
    let d1_binding = json!({"name":"DB","type":"d1","database_id":d1_id});
    let (status, bound_d1) =
        patch_settings(&client, &fixture, json!({"bindings":[d1_binding.clone()]})).await;
    assert_eq!(status, 200, "{bound_d1}");
    assert_eq!(bound_d1["result"]["bindings"], json!([d1_binding]));
    let before_annotation =
        active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    let (status, annotated) = patch_settings(
        &client,
        &fixture,
        json!({"annotations":{"workers/message":"saved settings","workers/tag":"audit"}}),
    )
    .await;
    assert_eq!(status, 200, "{annotated}");
    assert_eq!(
        annotated["result"]["annotations"]["workers/message"],
        "saved settings"
    );
    let annotated_version =
        active_version(&client, fixture.admin_addr, &fixture.public_account).await;
    assert_ne!(annotated_version, before_annotation);
    let (status, rejected_annotation) = patch_settings(
        &client,
        &fixture,
        json!({"annotations":{"workers/triggered_by":"forged"}}),
    )
    .await;
    assert_eq!(status, 400, "{rejected_annotation}");
    assert_eq!(
        active_version(&client, fixture.admin_addr, &fixture.public_account).await,
        annotated_version
    );
    assert_eq!(deployed_version(&client, &fixture).await, original);
    let (status, historical) = api(
        &client,
        &fixture,
        SCRIPT,
        &format!("/versions/{original}"),
        "GET",
        None,
    )
    .await;
    assert_eq!(status, 200, "{historical}");
    assert_eq!(
        historical["result"]["resources"]["script_runtime"]["limits"],
        json!({ "cpu_ms": 4_321 })
    );

    drop(command);
    restart(&client, &mut fixture).await;
    let (status, persisted) = api(&client, &fixture, SCRIPT, "/settings", "GET", None).await;
    assert_eq!(status, 200, "{persisted}");
    assert_eq!(
        persisted["result"]["limits"],
        json!({ "cpu_ms": 6_543, "subrequests": 432 })
    );
    assert_eq!(
        persisted["result"]["bindings"],
        bound_d1["result"]["bindings"]
    );
    assert_eq!(persisted["result"]["annotations"]["workers/tag"], "audit");
    assert_eq!(deployed_version(&client, &fixture).await, original);
    assert_eq!(invoke(&client, &fixture, SCRIPT, "").await.0, 200);

    let (status, secret) = api(
        &client,
        &fixture,
        SCRIPT,
        "/secrets",
        "PUT",
        Some(json!({"name":"DEPLOYED_SECRET","type":"secret_text","text":"hidden"})),
    )
    .await;
    assert_eq!(status, 200, "{secret}");
    assert_ne!(deployed_version(&client, &fixture).await, original);
    let (status, deployed_settings) =
        api(&client, &fixture, SCRIPT, "/settings", "GET", None).await;
    assert_eq!(status, 200, "{deployed_settings}");
    assert_eq!(
        deployed_settings["result"]["limits"],
        persisted["result"]["limits"]
    );
    assert_eq!(
        deployed_settings["result"]["bindings"],
        json!([
            {"name":"DEPLOYED_SECRET","type":"secret_text"},
            {"name":"DB","type":"d1","database_id":d1_id}
        ])
    );
    fixture.process.stop().await;
    assert_clean_output(&fs::read(&fixture.log).unwrap_or_default());
}

async fn deployed_version(client: &platform_process::Client, fixture: &Fixture) -> String {
    let (status, deployments) = api(client, fixture, SCRIPT, "/deployments", "GET", None).await;
    assert_eq!(status, 200, "{deployments}");
    deployments["result"]["deployments"][0]["versions"][0]["version_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

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
    // The pinned SDK sends these bracketed FormData fields on scripts.update.
    // Upload to a previously absent script before Wrangler creates its own.
    let boundary = "sdk-first-worker-loader";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata[main_module]\"\r\n\r\nindex.js\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"metadata[compatibility_date]\"\r\n\r\n2026-09-08\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"metadata[bindings][][type]\"\r\n\r\nworker_loader\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"metadata[bindings][][name]\"\r\n\r\nLOADER\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"files[]\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n\
         export default {{ fetch(request, env) {{ return Response.json({{ loader: typeof env.LOADER.get }}); }} }};\r\n--{boundary}--\r\n"
    );
    let response = client
        .request(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "http://{}/client/v4/accounts/{}/workers/scripts/sdk-first-loader",
                    fixture.admin_addr, fixture.public_account
                ))
                .header("authorization", format!("Bearer {TOKEN}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let upload_status = response.status();
    let upload_body = to_bytes(Body::new(response.into_body()), 1024 * 1024)
        .await
        .unwrap();
    assert!(
        upload_status.is_success(),
        "SDK first upload: {upload_status}: {}; ocd={}",
        String::from_utf8_lossy(&upload_body),
        fs::read_to_string(&fixture.log).unwrap_or_default()
    );
    let (_, settings) = api(
        &client,
        &fixture,
        "sdk-first-loader",
        "/settings",
        "GET",
        None,
    )
    .await;
    assert!(json_contains(
        &settings,
        "bindings",
        &json!([{ "name": "LOADER", "type": "worker_loader" }])
    ));
    let (status, body) = invoke(&client, &fixture, "sdk-first-loader", "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({ "loader": "function" }));
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
    assert_eq!(
        settings["result"]["limits"],
        json!({ "cpu_ms": 30_000, "subrequests": 10_000 })
    );
    config["worker_loaders"] = json!([{ "binding": "LOADER" }, { "binding": "OTHER" }]);
    let mut transfer_config = config.clone();
    transfer_config["name"] = json!("dynamic-transfer");
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
    config.as_object_mut().unwrap().remove("observability");
    fs::write(
        fixture.project.join("wrangler.jsonc"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    let (status, invalid) = invoke(&client, &fixture, SCRIPT, "/invalid").await;
    assert_eq!(status, 200);
    assert_eq!(
        invalid,
        json!({
            "limitsRejected": true,
            "delegationRejected": true,
            "keys": ["LOADER", "OBJECTS", "OTHER"]
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
    assert_state(&client, &fixture, "two", 1, "shared", 1).await;
    assert_facet(&client, &fixture, 2).await;
    promote(&client, &fixture, &first).await;
    assert_state(&client, &fixture, "one", 1, "shared", 1).await;
    promote(&client, &fixture, &first).await;
    assert_state(&client, &fixture, "one", 2, "shared", 2).await;
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
    let transfer_command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };
    let kv_id = configure_nontransferable_bindings(&transfer_command, &mut transfer_config).await;
    fs::write(
        fixture.project.join("transfer.jsonc"),
        serde_json::to_vec_pretty(&transfer_config).unwrap(),
    )
    .unwrap();
    fs::write(fixture.project.join("index.ts"), TRANSFER_SOURCE).unwrap();
    assert_success(
        &transfer_command
            .run(&["deploy", "--config", "transfer.jsonc"])
            .await,
    );
    let (status, invalid) = invoke(&client, &fixture, "dynamic-transfer", "/invalid").await;
    assert_eq!(status, 200);
    assert_eq!(
        invalid,
        json!({
            "transferErrors": {
                "d1": "DataCloneError",
                "kv": "DataCloneError",
                "queue": "DataCloneError",
                "r2": "DataCloneError",
            },
            "keys": ["BUCKET", "DB", "KV", "LOADER", "OTHER", "QUEUE"]
        })
    );
    let mut forward_config = transfer_config.clone();
    forward_config["name"] = json!("dynamic-forward");
    forward_config["no_bundle"] = json!(true);
    fs::write(
        fixture.project.join("forward.jsonc"),
        serde_json::to_vec_pretty(&forward_config).unwrap(),
    )
    .unwrap();
    fs::write(fixture.project.join("index.ts"), FORWARD_SOURCE).unwrap();
    assert_success(
        &transfer_command
            .run(&["deploy", "--config", "forward.jsonc"])
            .await,
    );
    let (status, forwarded) = invoke(&client, &fixture, "dynamic-forward", "").await;
    assert_eq!(status, 200, "{forwarded}");
    assert_eq!(
        forwarded,
        json!({ "kv": "kv-value", "d1": 42, "r2": "r2-value", "ordinary": "visible" })
    );
    drop(transfer_command);
    // Existing admission retains executed Versions until the supervised generation exits:
    // an HTTP response alone cannot prove that background tenant work has drained.
    assert_eq!(
        api(&client, &fixture, SCRIPT, "", "DELETE", None).await.0,
        409
    );
    let (status, deleted) = api(&client, &fixture, SCRIPT, "?force=true", "DELETE", None).await;
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
    fixture.process.stop().await;
    let storage = PlatformStorage::bootstrap(&storage_config(&fixture.data), &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .list_workers(account)
        .unwrap()
        .into_iter()
        .find(|worker| worker.name == SCRIPT)
        .unwrap();
    repository
        .begin_force_delete(
            account,
            worker.id,
            RequestId::generate(),
            open_compute_core::wall_time_ms(),
        )
        .unwrap();
    drop(storage);
    fixture.process.restart(&fixture.config, &fixture.log);
    wait_ready(
        &client,
        fixture.admin_addr,
        &mut fixture.process,
        &fixture.log,
    )
    .await;
    assert_eq!(
        invoke(&client, &fixture, SCRIPT, "").await.0,
        404,
        "startup must finish a persisted force-delete intent before admission"
    );
    assert_eq!(
        api(&client, &fixture, "dynamic-independent", "", "DELETE", None)
            .await
            .0,
        200
    );
    assert_eq!(
        api(
            &client,
            &fixture,
            "dynamic-transfer",
            "?force=true",
            "DELETE",
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        api(
            &client,
            &fixture,
            "dynamic-forward",
            "?force=true",
            "DELETE",
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        api(
            &client,
            &fixture,
            "sdk-first-loader",
            "?force=true",
            "DELETE",
            None
        )
        .await
        .0,
        200
    );
    let command = WranglerCommand {
        executable: fixed_wrangler(),
        project: &fixture.project,
        api_base_url: format!("http://{}/client/v4", fixture.admin_addr),
        account_id: &fixture.public_account,
    };
    delete_nontransferable_bindings(&command, &kv_id).await;
    fixture.process.stop().await;
    assert_clean_output(&fs::read(&fixture.log).unwrap_or_default());
    for address in [fixture.public_addr, fixture.admin_addr] {
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }
}

async fn configure_nontransferable_bindings(
    command: &WranglerCommand<'_>,
    config: &mut Value,
) -> String {
    assert_success(
        &command
            .run(&[
                "kv",
                "namespace",
                "create",
                KV_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let listed = command
        .run(&["kv", "namespace", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let kv_id = json_stdout(&listed)
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == KV_NAME)
        .and_then(|item| item["id"].as_str())
        .unwrap()
        .to_owned();
    assert_success(
        &command
            .run(&["d1", "create", D1_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let listed = command
        .run(&["d1", "list", "--json", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let d1_id = json_stdout(&listed)
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == D1_NAME)
        .and_then(|item| item["uuid"].as_str())
        .unwrap()
        .to_owned();
    assert_success(
        &command
            .run(&[
                "r2",
                "bucket",
                "create",
                R2_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&["queues", "create", QUEUE_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    config["kv_namespaces"] = json!([{ "binding": "KV", "id": &kv_id }]);
    config["d1_databases"] =
        json!([{ "binding": "DB", "database_name": D1_NAME, "database_id": d1_id }]);
    config["r2_buckets"] = json!([{ "binding": "BUCKET", "bucket_name": R2_NAME }]);
    config["queues"] = json!({ "producers": [{ "binding": "QUEUE", "queue": QUEUE_NAME }] });
    kv_id
}

async fn delete_nontransferable_bindings(command: &WranglerCommand<'_>, kv_id: &str) {
    for args in [
        vec![
            "kv",
            "namespace",
            "delete",
            "--namespace-id",
            kv_id,
            "--skip-confirmation",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "d1",
            "delete",
            D1_NAME,
            "--skip-confirmation",
            "--config",
            "transfer.jsonc",
        ],
        vec![
            "r2",
            "bucket",
            "delete",
            R2_NAME,
            "--config",
            "wrangler.jsonc",
        ],
        vec!["queues", "delete", QUEUE_NAME, "--config", "wrangler.jsonc"],
    ] {
        assert_success(&command.run(&args).await);
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
        .uri(format!("http://{}{}", fixture.public_addr, path))
        .header("host", worker_host(&fixture.internal_account, script))
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

async fn patch_limits(
    client: &platform_process::Client,
    fixture: &Fixture,
    cpu_ms: Option<u32>,
    subrequests: Option<u32>,
) -> (u16, Value) {
    let mut limits = serde_json::Map::new();
    if let Some(cpu_ms) = cpu_ms {
        limits.insert("cpu_ms".to_owned(), json!(cpu_ms));
    }
    if let Some(subrequests) = subrequests {
        limits.insert("subrequests".to_owned(), json!(subrequests));
    }
    patch_settings(client, fixture, json!({ "limits": limits })).await
}

async fn patch_settings(
    client: &platform_process::Client,
    fixture: &Fixture,
    settings: Value,
) -> (u16, Value) {
    let boundary = "w2-settings";
    let settings = serde_json::to_string(&settings).unwrap();
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"settings\"\r\nContent-Type: application/json\r\n\r\n{settings}\r\n--{boundary}--\r\n"
    );
    let request = Request::builder()
        .method("PATCH")
        .uri(format!(
            "http://{}/client/v4/accounts/{}/workers/scripts/{SCRIPT}/settings",
            fixture.admin_addr, fixture.public_account
        ))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(30), client.request(request))
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
