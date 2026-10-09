//! Fixed Cloudflare CLI against the local Cloudflare v4 API and real pinned workerd.

use super::*;
use axum::middleware;
use futures::FutureExt as _;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use std::panic::AssertUnwindSafe;
use std::process::Output;

const CF_VERSION: &str = "1.0.0-beta.12";
const WORKER_NAME: &str = "cf-runtime-gate";
const WORKFLOW_NAME: &str = "cf-runtime-gate-flow";
const FIXTURE_SECRET: &str = "cf-runtime-gate-secret";

pub(super) async fn exercise(
    state: HttpState,
    storage: Arc<PlatformStorage>,
    account: open_compute_core::InstanceId,
    public_account: &str,
    token: &str,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let traced = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let app = merged_router(state.with_local_origin_addr(address)).layer(middleware::from_fn(
        move |request: axum::extract::Request, next: middleware::Next| {
            let traced = traced.clone();
            async move {
                traced
                    .lock()
                    .unwrap()
                    .push(format!("{} {}", request.method(), request.uri()));
                next.run(request).await
            }
        },
    ));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await
            .unwrap();
    });
    let outcome = AssertUnwindSafe(verify_project(
        &origin,
        storage,
        account,
        public_account,
        token,
        requests,
    ))
    .catch_unwind()
    .await;
    let _ = stop_tx.send(());
    server.await.unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}

async fn verify_project(
    origin: &str,
    storage: Arc<PlatformStorage>,
    account: open_compute_core::InstanceId,
    public_account: &str,
    token: &str,
    requests: Arc<Mutex<Vec<String>>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let cf = fixed_cf();
    write_project(project, public_account);
    let api_base_url = format!("{origin}/client/v4");
    let command = CfCommand {
        executable: cf,
        project,
        api_base_url: &api_base_url,
        account_id: public_account,
        token,
    };

    let version = command.run(&["--version"]).await;
    assert_success(&version);
    assert!(String::from_utf8_lossy(&version.stdout).contains(CF_VERSION));

    let deployed = command
        .run(&[
            "deploy",
            "--mode",
            "production",
            "--secrets-file",
            "secrets.json",
            "--message",
            "runtime gate initial",
        ])
        .await;
    assert_success_with_trace(&deployed, &requests);

    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .list_workers(account)
        .unwrap()
        .into_iter()
        .find(|worker| worker.name == WORKER_NAME)
        .expect("Cloudflare CLI deploy must create the configured Worker");
    let first = worker
        .active_version_id
        .expect("Cloudflare CLI deploy must create an active Version");
    let workflow_repository = WorkflowRepository::new(storage.db());
    let workflow = workflow_repository
        .definitions(
            account,
            Some(WORKFLOW_NAME),
            None,
            open_compute_storage::catalog_page::CatalogSort::Name,
            open_compute_storage::catalog_page::CatalogDirection::Asc,
            None,
            10,
        )
        .unwrap()
        .items
        .into_iter()
        .find(|definition| definition.name == WORKFLOW_NAME)
        .expect("Cloudflare CLI deploy must complete the upload-first Workflow reservation");
    assert_eq!(workflow.state, open_compute_core::ResourceState::Ready);
    assert!(workflow.reserved_class_name.is_none());
    let workflow_version = workflow_repository
        .version(account, workflow.current_version_id.unwrap())
        .unwrap();
    assert_eq!(workflow_version.target.worker_version_id, first);
    assert_eq!(workflow_version.target.class_name, "Flow");
    let snapshot = repository
        .version_snapshot(account, worker.id, first, false)
        .unwrap();
    assert_eq!(snapshot.workflow_bindings.len(), 1);
    assert_eq!(snapshot.workflow_bindings[0].descriptor.class_name, "Flow");
    assert_eq!(
        repository.list_versions(account, worker.id).unwrap().len(),
        1
    );
    assert_worker_response(origin, account, 42).await;

    let upload_url = format!("{origin}/upload");
    // Each size checks its own connection: rejecting an unread oversized body
    // can close HTTP/1 before the client pool observes that close.
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new())
        .pool_max_idle_per_host(0)
        .build(HttpConnector::new());
    for declared in [true, false] {
        for size in [16 * 1024, 32 * 1024, 32 * 1024 + 1] {
            let payload = vec![b'u'; size];
            let mut request = Request::builder()
                .method("POST")
                .uri(&upload_url)
                .header("host", format!("{WORKER_NAME}.{account}.localhost"));
            if declared {
                request = request.header(header::CONTENT_LENGTH, size);
            }
            let stream = futures::stream::iter(
                payload
                    .chunks(1024)
                    .map(|chunk| Ok::<_, Infallible>(Bytes::copy_from_slice(chunk)))
                    .collect::<Vec<_>>(),
            );
            let outcome = client
                .request(request.body(Body::from_stream(stream)).unwrap())
                .await;
            if size > 32 * 1024 {
                // Oversized bodies may be rejected mid-stream; the peer then resets
                // before hyper finishes writing (BrokenPipe / connection closed).
                match outcome {
                    Ok(response) => assert_eq!(response.status(), 413),
                    Err(error) => {
                        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
                        let mut peer_closed = false;
                        while let Some(cause) = source {
                            peer_closed |=
                                cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                                    matches!(
                                        error.kind(),
                                        std::io::ErrorKind::BrokenPipe
                                            | std::io::ErrorKind::ConnectionReset
                                            | std::io::ErrorKind::ConnectionAborted
                                    )
                                }) || cause.downcast_ref::<hyper::Error>().is_some_and(|error| {
                                    error.is_closed() || error.is_incomplete_message()
                                });
                            source = cause.source();
                        }
                        assert!(
                            peer_closed,
                            "oversized upload must be rejected or reset: {error:?}"
                        );
                    }
                }
            } else {
                let response = outcome.unwrap();
                assert_eq!(response.status(), 200);
                assert_eq!(
                    to_bytes(Body::new(response.into_body()), 32 * 1024)
                        .await
                        .unwrap(),
                    payload
                );
            }
        }
    }
    assert_worker_response(origin, account, 42).await;

    let listed = command
        .run(&["workers", "versions", "list", "--worker-id", WORKER_NAME])
        .await;
    assert_success(&listed);
    assert!(
        json_output(&listed)
            .to_string()
            .contains(&first.to_string())
    );
    let first_id = first.to_string();
    let viewed = command
        .run(&[
            "workers",
            "versions",
            "get",
            &first_id,
            "--worker-id",
            WORKER_NAME,
        ])
        .await;
    assert_success(&viewed);
    let viewed = json_output(&viewed);
    assert_eq!(viewed["id"], first_id);
    assert!(
        viewed["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|binding| binding["type"] == "workflow"
                && binding["workflow_name"] == WORKFLOW_NAME
                && binding["class_name"] == "Flow")
    );

    std::fs::write(
        project.join("value.ts"),
        "export const answer: number = 43;",
    )
    .unwrap();
    let uploaded = command
        .run(&[
            "workers",
            "versions",
            "create",
            "--mode",
            "production",
            "--secrets-file",
            "secrets.json",
            "--message",
            "runtime gate candidate",
        ])
        .await;
    assert_success(&uploaded);
    let versions = repository.list_versions(account, worker.id).unwrap();
    assert_eq!(versions.len(), 2);
    let candidate = versions
        .iter()
        .find(|version| version.id != first)
        .unwrap()
        .id;
    assert_eq!(
        repository
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(first),
        "versions upload must not alter traffic"
    );

    let spec =
        serde_json::json!([{ "version_id": candidate.to_string(), "percentage": 100 }]).to_string();
    let promoted = command
        .run(&[
            "workers",
            "deployments",
            "create",
            "--worker",
            WORKER_NAME,
            "--strategy",
            "percentage",
            "--versions",
            &spec,
        ])
        .await;
    assert_success(&promoted);
    assert_eq!(
        repository
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(candidate)
    );
    assert_worker_response(origin, account, 43).await;

    let output = command
        .run(&["workers", "deployments", "list", "--worker", WORKER_NAME])
        .await;
    assert_success(&output);
    assert!(
        json_output(&output)
            .to_string()
            .contains(&candidate.to_string())
    );

    let rollback = serde_json::json!([{ "version_id": first_id, "percentage": 100 }]).to_string();
    let rolled_back = command
        .run(&[
            "workers",
            "deployments",
            "create",
            "--worker",
            WORKER_NAME,
            "--strategy",
            "percentage",
            "--versions",
            &rollback,
        ])
        .await;
    assert_success(&rolled_back);
    assert_eq!(
        repository
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(first)
    );
    assert_worker_response(origin, account, 42).await;

    let trace = requests.lock().unwrap();
    let upload = trace
        .iter()
        .position(|line| {
            line.starts_with("PUT ") && line.contains("/workers/scripts/cf-runtime-gate?")
        })
        .expect("cf deploy must upload the configured Script");
    let workflow_put = trace
        .iter()
        .position(|line| {
            line.starts_with("PUT ") && line.contains("/workflows/cf-runtime-gate-flow")
        })
        .expect("cf deploy must configure the Workflow");
    assert!(
        upload < workflow_put,
        "Worker upload must precede Workflow activation"
    );
    assert!(trace.iter().any(|line| line.contains("/deployments")));
    assert!(trace.iter().all(|line| !line.contains("/__workers/")));
}

fn write_project(project: &Path, account_id: &str) {
    let mut vars = BTreeMap::from([("GREETING".to_owned(), "你好 🌍".to_owned())]);
    for index in 0..126 {
        vars.insert(format!("VALUE_{index}"), "x".repeat(5 * 1024));
    }
    let root = repo_root();
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("package.json")).unwrap()).unwrap();
    let catalog = &package["catalog"];
    std::fs::write(project.join("package.json"), serde_json::json!({
        "private":true, "type":"module", "devDependencies":{
            "cf":catalog["cf"], "vite":catalog["vite"], "@cloudflare/vite-plugin":catalog["@cloudflare/vite-plugin"]
        }
    }).to_string()).unwrap();
    std::os::unix::fs::symlink(root.join("node_modules"), project.join("node_modules")).unwrap();
    std::fs::write(project.join("vite.config.ts"), "import {cloudflare} from '@cloudflare/vite-plugin'; export default {plugins:[cloudflare({types:{generate:false}})]};").unwrap();
    std::fs::create_dir(project.join("xdg")).unwrap();
    let mut bindings = serde_json::Map::new();
    for (name, value) in vars {
        bindings.insert(name, serde_json::json!({"type":"text","value":value}));
    }
    bindings.insert(
        "FLOW".into(),
        serde_json::json!({"type":"workflow", "name":WORKFLOW_NAME, "worker":WORKER_NAME, "exportName":"Flow"}),
    );
    let config = serde_json::json!({
        "accountId": account_id,
        "worker": {
            "name":WORKER_NAME, "entrypoint":"index.ts", "compatibilityDate":"2026-09-08",
            "compatibilityFlags":["nodejs_compat"], "workersDev":false,
            "observability":{"enabled":false}, "env":bindings,
            "exports":{"Flow":{"type":"workflow", "name":WORKFLOW_NAME}}
        }
    });
    std::fs::write(
        project.join("cloudflare.config.ts"),
        format!("export default {config};"),
    )
    .unwrap();
    std::fs::write(
        project.join("secrets.json"),
        serde_json::to_vec(&serde_json::json!({"TOKEN": FIXTURE_SECRET})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        project.join("value.ts"),
        "export const answer: number = 42;",
    )
    .unwrap();
    std::fs::write(
        project.join("lazy.ts"),
        "export const suffix: string = '!';",
    )
    .unwrap();
    std::fs::write(
        project.join("index.ts"),
        r#"import { WorkflowEntrypoint } from 'cloudflare:workers';
import { answer } from './value.js';
interface Env { GREETING: string; TOKEN: string; VALUE_125: string }
export class Flow extends WorkflowEntrypoint<Env, unknown> {
  async run(): Promise<unknown> { return {ok: true}; }
}
export default { async fetch(_request: Request, env: Env): Promise<Response> {
  if (_request.method === 'POST') return new Response(await _request.arrayBuffer());
  const { suffix } = await import('./lazy.js');
  return Response.json({greeting: env.GREETING, answer, suffix, hasSecret: env.TOKEN.length > 0,
    variableCount: Object.values(env).filter(value => typeof value === 'string').length,
    variableBytes: new TextEncoder().encode(env.VALUE_125).byteLength});
}};"#,
    )
    .unwrap();
}

struct CfCommand<'a> {
    executable: PathBuf,
    project: &'a Path,
    api_base_url: &'a str,
    account_id: &'a str,
    token: &'a str,
}

impl CfCommand<'_> {
    async fn run(&self, args: &[&str]) -> Output {
        assert!(self.api_base_url.starts_with("http://127.0.0.1:"));
        assert!(self.api_base_url.ends_with("/client/v4"));
        let mut command = tokio::process::Command::new("node");
        command
            .arg(&self.executable)
            .args(args)
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(repo_root().join("node_modules/.bin")).chain(
                        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                    ),
                )
                .unwrap(),
            )
            .current_dir(self.project)
            .env("CLOUDFLARE_API_BASE_URL", self.api_base_url)
            .env("CLOUDFLARE_API_TOKEN", self.token)
            .env("CLOUDFLARE_ACCOUNT_ID", self.account_id)
            .env("CF_SEND_TELEMETRY", "false")
            .env("WRANGLER_LOG_SANITIZE", "true")
            .env("DO_NOT_TRACK", "1")
            .env("CI", "true")
            .env("XDG_CONFIG_HOME", self.project.join("xdg"))
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env_remove("CF_API_BASE_URL")
            .env_remove("CLOUDFLARE_BASE_URL")
            .env_remove("CLOUDFLARE_API_KEY")
            .env_remove("CLOUDFLARE_EMAIL")
            .env_remove("CLOUDFLARE_API_USER_SERVICE_KEY")
            .kill_on_drop(true);
        tokio::time::timeout(Duration::from_secs(120), command.output())
            .await
            .expect("fixed Cloudflare CLI command timed out")
            .expect("the fixed Cloudflare CLI installation and Node.js must already be available")
    }
}

fn fixed_cf() -> PathBuf {
    let root = repo_root();
    let lock = std::fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"cf\": [\"cf@1.0.0-beta.12\""));
    let package = root.join("node_modules/cf");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package.join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], CF_VERSION);
    let executable = package.join("bin/cf");
    assert!(executable.is_file());
    executable
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains(FIXTURE_SECRET));
        assert!(!text.contains("api.cloudflare.com"));
    }
}

fn assert_success_with_trace(output: &Output, requests: &Mutex<Vec<String>>) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}\nlocal requests={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        requests.lock().unwrap()
    );
    assert_success(output);
}

fn json_output(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "Cloudflare CLI JSON output was invalid: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

async fn assert_worker_response(origin: &str, account: open_compute_core::InstanceId, answer: u64) {
    let url = format!("{origin}/hello");
    let client: Client<HttpConnector, Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let request = Request::builder()
        .uri(url)
        .header("host", format!("{WORKER_NAME}.{account}.localhost"))
        .body(Body::empty())
        .unwrap();
    let response = client.request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "greeting": "你好 🌍",
            "answer": answer,
            "suffix": "!",
            "hasSecret": true,
            "variableCount": 128,
            "variableBytes": 5 * 1024
        })
    );
}
