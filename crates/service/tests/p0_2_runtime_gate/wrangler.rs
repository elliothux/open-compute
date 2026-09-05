//! Fixed Wrangler against the local Cloudflare v4 API and real pinned workerd.

use super::*;
use axum::middleware;
use futures::FutureExt as _;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use std::panic::AssertUnwindSafe;
use std::process::Output;

const WRANGLER_VERSION: &str = "4.127.1";
const WORKER_NAME: &str = "wrangler-runtime-gate";
const WORKFLOW_NAME: &str = "wrangler-runtime-gate-flow";
const FIXTURE_SECRET: &str = "wrangler-runtime-gate-secret";

pub(super) async fn exercise(
    app: axum::Router,
    storage: Arc<PlatformStorage>,
    account: open_compute_core::AccountId,
    public_account: &str,
    token: &str,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let traced = requests.clone();
    let app = app.layer(middleware::from_fn(
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
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
    account: open_compute_core::AccountId,
    public_account: &str,
    token: &str,
    requests: Arc<Mutex<Vec<String>>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let wrangler = fixed_wrangler();
    write_project(project, public_account, &wrangler);
    let api_base_url = format!("{origin}/client/v4");
    let command = WranglerCommand {
        executable: wrangler,
        project,
        api_base_url: &api_base_url,
        account_id: public_account,
        token,
    };

    let version = command.run(&["--version"]).await;
    assert_success(&version);
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        WRANGLER_VERSION
    );

    let deployed = command
        .run(&[
            "deploy",
            "--config",
            "wrangler.jsonc",
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
        .expect("Wrangler deploy must create the configured Worker");
    let first = worker
        .active_version_id
        .expect("Wrangler deploy must create an active Version");
    let workflow_repository = WorkflowRepository::new(storage.db());
    let workflow = workflow_repository
        .definitions(
            account,
            Some(WORKFLOW_NAME),
            None,
            open_compute_storage::CatalogSort::Name,
            open_compute_storage::CatalogDirection::Asc,
            None,
            10,
        )
        .unwrap()
        .items
        .into_iter()
        .find(|definition| definition.name == WORKFLOW_NAME)
        .expect("Wrangler deploy must complete the upload-first Workflow reservation");
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

    let upload_url = format!("{origin}/__workers/{account}/{WORKER_NAME}/upload");
    let client: Client<HttpConnector, Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    for declared in [true, false] {
        for size in [16 * 1024, 32 * 1024, 32 * 1024 + 1] {
            let payload = vec![b'u'; size];
            let mut request = Request::builder().method("POST").uri(&upload_url);
            if declared {
                request = request.header(header::CONTENT_LENGTH, size);
            }
            let stream = futures::stream::iter(
                payload
                    .chunks(1024)
                    .map(|chunk| Ok::<_, Infallible>(Bytes::copy_from_slice(chunk)))
                    .collect::<Vec<_>>(),
            );
            let response = client
                .request(request.body(Body::from_stream(stream)).unwrap())
                .await
                .unwrap();
            if size > 32 * 1024 {
                assert_eq!(response.status(), 503);
            } else {
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

    let listed = command
        .run(&["versions", "list", "--config", "wrangler.jsonc", "--json"])
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
            "versions",
            "view",
            &first_id,
            "--config",
            "wrangler.jsonc",
            "--json",
        ])
        .await;
    assert_success(&viewed);
    let viewed = json_output(&viewed);
    assert_eq!(viewed["id"], first_id);
    assert!(
        viewed["resources"]["bindings"]
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
            "versions",
            "upload",
            "--config",
            "wrangler.jsonc",
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

    let spec = format!("{candidate}@100");
    let promoted = command
        .run(&[
            "versions",
            "deploy",
            &spec,
            "--config",
            "wrangler.jsonc",
            "--message",
            "runtime gate promote",
            "--yes",
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

    for args in [
        [
            "deployments",
            "list",
            "--config",
            "wrangler.jsonc",
            "--json",
        ],
        [
            "deployments",
            "status",
            "--config",
            "wrangler.jsonc",
            "--json",
        ],
    ] {
        let output = command.run(&args).await;
        assert_success(&output);
        let json = json_output(&output);
        assert!(json.to_string().contains(&candidate.to_string()));
    }

    let rolled_back = command
        .run(&[
            "rollback",
            &first_id,
            "--config",
            "wrangler.jsonc",
            "--message",
            "runtime gate rollback",
            "--yes",
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
    assert!(trace.iter().any(|line| {
        line.starts_with("PUT ")
            && line.contains("/client/v4/accounts/")
            && line.contains("/workers/scripts/wrangler-runtime-gate")
            && line.contains("excludeScript=true")
    }));
    let upload = trace
        .iter()
        .position(|line| {
            line.starts_with("PUT ")
                && line.contains("/workers/scripts/wrangler-runtime-gate")
                && line.contains("excludeScript=true")
        })
        .expect("Wrangler deploy must upload the Worker");
    let workflow_put = trace
        .iter()
        .position(|line| {
            line.starts_with("PUT ") && line.contains("/workflows/wrangler-runtime-gate-flow")
        })
        .expect("Wrangler deploy must configure the Workflow");
    let account_subdomain = trace
        .iter()
        .position(|line| {
            line.starts_with("GET ")
                && line.ends_with(&format!("/accounts/{public_account}/workers/subdomain"))
        })
        .expect("Wrangler Workflow deploy must read the account subdomain prerequisite");
    assert!(
        upload < account_subdomain && account_subdomain < workflow_put,
        "Wrangler must upload, read the discarded account prerequisite, then configure Workflow"
    );
    assert!(trace.iter().any(|line| {
        line.starts_with("POST ") && line.contains("/versions?bindings_inherit=strict")
    }));
    assert!(trace.iter().any(|line| line.contains("/deployments")));
    assert!(
        trace
            .iter()
            .all(|line| line.contains(" /client/v4/") || line.contains(" /__workers/"))
    );
}

fn write_project(project: &Path, account_id: &str, wrangler: &Path) {
    let schema = wrangler
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("config-schema.json");
    assert!(
        schema.is_file(),
        "fixed Wrangler config schema must be installed"
    );
    std::fs::create_dir(project.join("xdg")).unwrap();
    std::fs::write(
        project.join("wrangler.jsonc"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "$schema": schema,
            "name": WORKER_NAME,
            "main": "index.ts",
            "account_id": account_id,
            "compatibility_date": "2026-08-30",
            "compatibility_flags": ["nodejs_compat"],
            "workers_dev": false,
            "observability": {"enabled": false},
            "workflows": [{
                "binding": "FLOW",
                "name": WORKFLOW_NAME,
                "class_name": "Flow"
            }],
            "vars": {"GREETING": "你好 🌍"}
        }))
        .unwrap(),
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
interface Env { GREETING: string; TOKEN: string }
export class Flow extends WorkflowEntrypoint<Env, unknown> {
  async run(): Promise<unknown> { return {ok: true}; }
}
export default { async fetch(_request: Request, env: Env): Promise<Response> {
  if (_request.method === 'POST') return new Response(await _request.arrayBuffer());
  const { suffix } = await import('./lazy.js');
  return Response.json({greeting: env.GREETING, answer, suffix, hasSecret: env.TOKEN.length > 0});
}};"#,
    )
    .unwrap();
}

struct WranglerCommand<'a> {
    executable: PathBuf,
    project: &'a Path,
    api_base_url: &'a str,
    account_id: &'a str,
    token: &'a str,
}

impl WranglerCommand<'_> {
    async fn run(&self, args: &[&str]) -> Output {
        assert!(self.api_base_url.starts_with("http://127.0.0.1:"));
        assert!(self.api_base_url.ends_with("/client/v4"));
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(args)
            .current_dir(self.project)
            .env("CLOUDFLARE_API_BASE_URL", self.api_base_url)
            .env("CLOUDFLARE_API_TOKEN", self.token)
            .env("CLOUDFLARE_ACCOUNT_ID", self.account_id)
            .env("WRANGLER_SEND_METRICS", "false")
            .env("WRANGLER_SEND_ERROR_REPORTS", "false")
            .env("WRANGLER_NO_SKILLS_UPDATE_PROMPTS", "true")
            .env("WRANGLER_HIDE_BANNER", "true")
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
            .expect("fixed Wrangler command timed out")
            .expect("the fixed Wrangler installation and Node.js must already be available")
    }
}

fn fixed_wrangler() -> PathBuf {
    let root = repo_root();
    let lock = std::fs::read_to_string(root.join("bun.lock")).unwrap();
    assert!(lock.contains("\"wrangler\": [\"wrangler@4.127.1\""));
    let prefix = format!("wrangler@{WRANGLER_VERSION}+");
    let mut installs = std::fs::read_dir(root.join("node_modules/.bun"))
        .expect("locked Bun dependencies must already be installed")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    installs.sort();
    assert_eq!(
        installs.len(),
        1,
        "exactly one fixed Wrangler must be installed"
    );
    let package = installs[0].join("node_modules/wrangler");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(package.join("package.json")).unwrap()).unwrap();
    assert_eq!(metadata["version"], WRANGLER_VERSION);
    let executable = package.join("bin/wrangler.js");
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
            "Wrangler JSON output was invalid: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

async fn assert_worker_response(origin: &str, account: open_compute_core::AccountId, answer: u64) {
    let url = format!("{origin}/__workers/{account}/{WORKER_NAME}/hello");
    let client: Client<HttpConnector, Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let response = client.get(url.parse().unwrap()).await.unwrap();
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
            "hasSecret": true
        })
    );
}
