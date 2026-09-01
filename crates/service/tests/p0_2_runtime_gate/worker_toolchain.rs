//! TypeScript CLI against the production HTTP router and real pinned workerd.

use super::*;
use futures::FutureExt as _;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_core::{AccountId, DeploymentId, WorkerId};
use std::panic::AssertUnwindSafe;
use std::process::Output;

pub(super) async fn exercise(app: axum::Router, storage: Arc<PlatformStorage>, account: AccountId) {
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
    let outcome = AssertUnwindSafe(verify_project(&origin, storage, account))
        .catch_unwind()
        .await;
    let _ = stop_tx.send(());
    server.await.unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}

async fn verify_project(origin: &str, storage: Arc<PlatformStorage>, account: AccountId) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    std::fs::write(project.join("package.json"), r#"{"type":"module"}"#).unwrap();
    std::fs::write(
        project.join("tsconfig.json"),
        r#"{"compilerOptions":{"target":"ES2024","module":"Preserve","moduleResolution":"Bundler",
            "lib":["ES2024","DOM"],"types":[],"strict":true,"noEmit":true},"include":["*.ts"]}"#,
    )
    .unwrap();
    std::fs::write(
        project.join("open-compute.json"),
        serde_json::json!({
            "name": "typescript-cli", "main": "index.ts",
            "endpoint": origin, "vars": {"GREETING": "你好 🌍"},
            "secrets": {"TOKEN": {"env": "OPEN_COMPUTE_TS_FIXTURE_SECRET"}},
        })
        .to_string(),
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
    let source = r#"import { answer } from './value.js';
        interface Env { GREETING: string; TOKEN: string }
        export class NamedCounter { read(): number { return answer; } }
        export default { async fetch(_request: Request, env: Env): Promise<Response> {
            const { suffix } = await import('./lazy.js');
            return Response.json({greeting: env.GREETING, answer, suffix, hasSecret: env.TOKEN.length > 0});
        }};"#;
    std::fs::write(project.join("index.ts"), source).unwrap();

    let output = cli(
        project,
        &["build", "--out", "worker.bundle", "--json"],
        false,
    )
    .await;
    assert_success(&output);
    let bytes = std::fs::read(project.join("worker.bundle")).unwrap();
    let bundle = CanonicalBundle::parse(bytes.clone(), BundleLimits::default()).unwrap();
    assert_eq!(bundle.manifest().main_module, "worker.js");
    assert!(
        bundle.manifest().modules.len() > 1,
        "lazy imports remain valid modules"
    );
    assert!(!String::from_utf8_lossy(&bytes).contains("typescript-fixture-secret"));
    let duplicate = cli(project, &["build", "--out", "worker.bundle"], false).await;
    assert!(!duplicate.status.success());
    assert_eq!(std::fs::read(project.join("worker.bundle")).unwrap(), bytes);

    let output = cli(project, &["run", "--json"], true).await;
    assert_success(&output);
    let deployed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let worker: WorkerId = deployed["workerId"].as_str().unwrap().parse().unwrap();
    let first: DeploymentId = deployed["deploymentId"].as_str().unwrap().parse().unwrap();
    let url = deployed["url"].as_str().unwrap();
    assert!(url.starts_with(origin));
    let response = fetch_json(url).await;
    assert_eq!(
        response,
        serde_json::json!({"greeting":"你好 🌍", "answer":42, "suffix":"!", "hasSecret":true})
    );

    let repository = WorkerRepository::new(storage.db());
    assert_eq!(
        repository
            .get_worker(account, worker)
            .unwrap()
            .active_deployment_id,
        Some(first)
    );
    let before = repository.list_deployments(account, worker).unwrap().len();
    std::fs::write(
        project.join("value.ts"),
        "export const answer: number = 'wrong';",
    )
    .unwrap();
    let rejected = cli(project, &["run", "--json"], true).await;
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("TypeScript validation failed"));
    assert_eq!(
        repository.list_deployments(account, worker).unwrap().len(),
        before
    );
    assert_eq!(
        repository
            .get_worker(account, worker)
            .unwrap()
            .active_deployment_id,
        Some(first)
    );

    std::fs::write(
        project.join("value.ts"),
        "export const answer: number = 43;",
    )
    .unwrap();
    let output = cli(project, &["run", "--json"], true).await;
    assert_success(&output);
    let replacement: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(replacement["workerId"], deployed["workerId"]);
    assert_ne!(replacement["deploymentId"], deployed["deploymentId"]);
    assert_eq!(replacement["url"], deployed["url"]);
    let response = fetch_json(url).await;
    assert_eq!(response["answer"], 43);
    assert_eq!(
        repository.list_deployments(account, worker).unwrap().len(),
        before + 1
    );
}

async fn cli(project: &Path, args: &[&str], with_secret: bool) -> Output {
    let bun = std::env::var_os("OPEN_COMPUTE_TEST_BUN").unwrap_or_else(|| "bun".into());
    let mut command = tokio::process::Command::new(bun);
    command
        .arg(repo_root().join("packages/toolchain/src/bin.ts"))
        .args(args)
        .args(["--ocd", env!("CARGO_BIN_EXE_ocd")])
        .current_dir(project)
        .env_remove("OPEN_COMPUTE_ADMIN_TOKEN")
        .env_remove("OPEN_COMPUTE_TS_FIXTURE_SECRET")
        .kill_on_drop(true);
    if with_secret {
        command.env(
            "OPEN_COMPUTE_TS_FIXTURE_SECRET",
            "typescript-fixture-secret",
        );
    }
    tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .expect("TypeScript command timed out")
        .expect("Bun and the locked workspace dependencies must already be installed")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("typescript-fixture-secret"));
    }
}

async fn fetch_json(url: &str) -> serde_json::Value {
    let client: Client<HttpConnector, Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let response = client.get(url.parse().unwrap()).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}
