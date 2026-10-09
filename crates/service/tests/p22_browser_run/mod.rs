//! Browser Run through fixed cf, real ocd/workerd, SQLite and headless-shell.

#![cfg(feature = "test-support")]

mod connections;
mod devtools;
mod downloads;
mod live;
mod managed;
mod model;
mod native;
#[allow(
    dead_code,
    reason = "reuse the existing daemon ownership and recovery fixture"
)]
#[path = "../workflow_support/platform_process.rs"]
mod platform_process;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use open_compute_artifacts::MockS3;
use open_compute_runtime::browser::BrowserCdp;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

async fn cf(project: &Path, state: &Path, address: &str, account: &str, args: &[&str]) -> Value {
    let root = repo();
    let package: Value =
        serde_json::from_slice(&fs::read(root.join("node_modules/cf/package.json")).unwrap())
            .unwrap();
    assert_eq!(package["version"], "1.0.0-beta.12");
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("node")
            .arg(root.join("node_modules/cf/bin/cf"))
            .args(args)
            .current_dir(project)
            .env(
                "CLOUDFLARE_API_BASE_URL",
                format!("http://{address}/client/v4"),
            )
            .env("CLOUDFLARE_API_TOKEN", "workflow-deployer")
            .env("CLOUDFLARE_ACCOUNT_ID", account)
            .env("XDG_CONFIG_HOME", state)
            .env("CF_SEND_TELEMETRY", "false")
            .env("DO_NOT_TRACK", "1")
            .env("CI", "true")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env("all_proxy", "http://127.0.0.1:9")
            .env("NO_PROXY", "127.0.0.1,localhost,::1")
            .env("no_proxy", "127.0.0.1,localhost,::1")
            .env_remove("CF_API_BASE_URL")
            .env_remove("CLOUDFLARE_BASE_URL")
            .env_remove("CLOUDFLARE_API_KEY")
            .env_remove("CLOUDFLARE_EMAIL")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cf {args:?}: {stdout}\n{stderr}");
    for secret in ["workflow-deployer", "workflow-admin", "workflow-read-only"] {
        assert!(!stdout.contains(secret) && !stderr.contains(secret));
    }
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn copy_output(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let next = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_output(&entry.path(), &next);
        } else {
            fs::copy(entry.path(), next).unwrap();
        }
    }
}

async fn request(
    client: &platform_process::Client,
    address: &str,
    host: &str,
    path: &str,
    method: &str,
    token: Option<&str>,
    body: Value,
) -> (u16, String, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://{address}{path}"))
        .header("host", host)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let response = tokio::time::timeout(
        Duration::from_secs(45),
        client.request(builder.body(body).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status().as_u16();
    let media = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = tokio::time::timeout(
        Duration::from_secs(45),
        to_bytes(Body::new(response.into_body()), 16 * 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    (status, media, bytes.to_vec())
}

async fn browser_metrics(
    client: &platform_process::Client,
    admin: &str,
    instance: &str,
    active_sessions: u64,
    outcomes: &[(&str, &str)],
) {
    let mut last = String::new();
    let text = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (status, media, bytes) = request(
                client,
                admin,
                admin,
                "/metrics",
                "GET",
                Some("workflow-admin"),
                Value::Null,
            )
            .await;
            assert_eq!(status, 200);
            assert!(media.starts_with("text/plain"));
            let text = String::from_utf8(bytes).unwrap();
            if text.contains(&format!(
                "browser_active_sessions{{instance_id=\"{instance}\"}} {active_sessions}\n"
            )) && text
                .lines()
                .filter(|line| line.starts_with("browser_in_flight_operations"))
                .all(|line| line.ends_with(" 0"))
            {
                return text;
            }
            last = text
                .lines()
                .filter(|line| {
                    line.starts_with("browser_active_sessions")
                        || line.starts_with("browser_in_flight_operations")
                })
                .collect::<Vec<_>>()
                .join("\n");
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("Browser caller operations or session gauge did not settle; expected {active_sessions} sessions:\n{last}")
    });
    for (operation, outcome) in outcomes {
        let prefix = format!(
            "browser_operations_total{{operation=\"{operation}\",outcome=\"{outcome}\",instance_id=\"{instance}\"}} "
        );
        let count = text
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(count > 0, "{operation}/{outcome} was not recorded");
    }
    for line in text.lines().filter(|line| line.starts_with("browser_")) {
        assert!(line.contains(&format!("instance_id=\"{instance}\"")));
        if line.starts_with("browser_in_flight_operations") {
            assert!(line.ends_with(" 0"), "caller operation leaked: {line}");
        }
    }
    for secret in [
        "custom-request-key",
        "fallback-request-key",
        "rejected-request-key",
        "workflow-admin",
        "workflow-deployer",
        "<html",
        "file://",
    ] {
        assert!(!text.contains(secret), "Browser metrics leaked content");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixed_clients_actions_public_cf_and_restart_use_real_browser() {
    for playwright in [false, true] {
        exercise(playwright).await;
    }
}

async fn exercise(playwright: bool) {
    let purpose = repo().join(".temp/p22-product");
    fs::create_dir_all(&purpose).unwrap();
    let root = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(purpose)
        .unwrap()
        .keep();
    let chrome_dir = root.join("chrome");
    fs::create_dir(&chrome_dir).unwrap();
    let (chrome, endpoint) = native::native_fixture(&chrome_dir).await;
    let observer = BrowserCdp::connect(
        &endpoint,
        None,
        16 * 1024 * 1024,
        32,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let (model_endpoint, model_task) = model::spawn().await;
    let data = root.join("data");
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &open_compute_core::config::DataConfig {
            path: data.clone(),
            master_key_file: data.join("keys/master.key"),
            ..open_compute_core::config::DataConfig::default()
        },
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let internal_account = storage.identity().instance_id.to_string();
    drop(storage);
    let (public, admin) = platform_process::distinct_addresses();
    let config = platform_process::config(&root, &root.join("data"), &mock.endpoint, public, admin);
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        r#"
[browser]
public_origin = "http://{admin}"
max_sessions = 4
max_pending_acquires = 4
acquire_timeout_ms = 10000
command_timeout_ms = 30000
max_connections = 4
max_actions = 2
max_body_bytes = 1048576
max_download_bytes = 16777216
max_download_files = 16
max_result_bytes = 16777216
max_message_bytes = 16777216
max_queued_messages = 256
max_history_entries = 1000
history_retention_ms = 86400000
max_frontend_requests = 64
[browser.backend]
kind = "cdp"
url = "{endpoint}"
"#
    ));
    text.push_str(&model::configuration(&model_endpoint));
    fs::write(&config, text).unwrap();
    let log = root.join("ocd.stderr.log");
    let mut process = platform_process::spawn(&config, &log);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    platform_process::ready(&client, admin, &mut process).await;
    let admin = admin.to_string();
    let public = public.to_string();
    let (status, _, bytes) = request(
        &client,
        &admin,
        &admin,
        "/client/v4/accounts",
        "GET",
        Some("workflow-read-only"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let accounts: Value = serde_json::from_slice(&bytes).unwrap();
    let account = accounts["result"][0]["id"].as_str().unwrap();
    let project = root.join("app");
    copy_output(
        &repo().join("test/applications/browser-run/.cloudflare"),
        &project.join(".cloudflare"),
    );
    fs::copy(
        repo().join("test/applications/browser-run/cloudflare.config.ts"),
        project.join("cloudflare.config.ts"),
    )
    .unwrap();
    std::os::unix::fs::symlink(repo().join("node_modules"), project.join("node_modules")).unwrap();
    let state = root.join("client-config");
    cf(
        &project,
        &state,
        &admin,
        account,
        &["deploy", "--prebuilt", "--mode", "production"],
    )
    .await;
    let host = format!("browser-run-fixture.{internal_account}.localhost");
    let cases: &[(&str, u16, &[u8], &str)] = if playwright {
        &[("/playwright", 200, b"Playwright", "application/json")]
    } else {
        &[
            ("/sessions", 200, b"[]", "application/json"),
            ("/content", 200, b"rendered", "application/json"),
            ("/screenshot", 200, b"\x89PNG", "image/png"),
            ("/pdf", 200, b"%PDF-", "application/pdf"),
            ("/scrape", 200, b"rendered", "application/json"),
            (
                "/links",
                200,
                b"https://example.com/linked",
                "application/json",
            ),
            ("/snapshot", 200, b"accessibilityTree", "application/json"),
            ("/accessibilityTree", 200, b"Submit", "application/json"),
            ("/markdown", 200, b"# rendered", "application/json"),
            ("/snapshot-markdown", 200, b"title:", "application/json"),
            ("/filter", 200, b"Filtered", "application/json"),
            (
                "/navigation-before-action",
                200,
                b"Loaded before action",
                "application/json",
            ),
            ("/json", 200, b"heading", "application/json"),
            ("/json-custom", 200, b"custom", "application/json"),
            (
                "/json-custom-workers-ai",
                200,
                b"rendered",
                "application/json",
            ),
            (
                "/json-custom-fallback",
                200,
                b"fallback",
                "application/json",
            ),
            (
                "/json-custom-denied",
                503,
                b"BROWSER_UNAVAILABLE",
                "application/json",
            ),
            ("/json-object", 200, b"heading", "application/json"),
            ("/json-schema", 200, b"heading", "application/json"),
            (
                "/json-schema-mismatch",
                503,
                b"BROWSER_UNAVAILABLE",
                "application/json",
            ),
            (
                "/json-malformed",
                503,
                b"BROWSER_UNAVAILABLE",
                "application/json",
            ),
            (
                "/json-rate",
                429,
                b"BROWSER_LIMIT_EXCEEDED",
                "application/json",
            ),
            (
                "/json-limit",
                429,
                b"BROWSER_LIMIT_EXCEEDED",
                "application/json",
            ),
            (
                "/pattern-timeout",
                504,
                b"BROWSER_TIMEOUT",
                "application/json",
            ),
            (
                "/invalid",
                400,
                b"BROWSER_INPUT_INVALID",
                "application/json",
            ),
            ("/timeout", 504, b"BROWSER_TIMEOUT", "application/json"),
        ]
    };
    for &(path, status, expected, media) in cases {
        let (actual, content_type, bytes) =
            request(&client, &public, &host, path, "GET", None, Value::Null).await;
        assert_eq!(
            actual,
            status,
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(content_type.starts_with(media), "{path}: {content_type}");
        assert!(
            bytes.windows(expected.len()).any(|value| value == expected),
            "{path}: {}",
            String::from_utf8_lossy(&bytes)
        );
        if path == "/screenshot" {
            assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 320);
            assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 240);
        }
        if path == "/playwright" {
            downloads::check(&bytes);
        }
        if path == "/snapshot-markdown" {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                value["result"]["markdown"]
                    .as_str()
                    .unwrap()
                    .starts_with("---\ntitle: \"Actions\"\n---\n\n")
            );
        }
        if !playwright {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let contexts = observer
                        .command("Target.getBrowserContexts", json!({}), None)
                        .await
                        .unwrap();
                    if contexts.pointer("/result/browserContextIds") == Some(&json!([])) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("action leaked a native BrowserContext");
        }
    }
    browser_metrics(
        &client,
        &admin,
        &internal_account,
        0,
        if playwright {
            &[("acquire", "success"), ("command", "success")]
        } else {
            &[
                ("acquire", "success"),
                ("command", "success"),
                ("content", "success"),
                ("json", "success"),
                ("json", "failure"),
                ("json", "limit"),
                ("content", "timeout"),
            ]
        },
    )
    .await;
    if !playwright {
        let base = format!("/client/v4/accounts/{account}/browser-rendering");
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new("bun")
                .arg(repo().join("test/conformance/applications/check-browser-run.ts"))
                .arg(format!("http://{admin}/client/v4"))
                .arg(account)
                .current_dir(repo())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "SDK: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let (status, _, _) = request(
            &client,
            &admin,
            &admin,
            &format!("{base}/content"),
            "POST",
            Some("workflow-read-only"),
            json!({"html":"denied"}),
        )
        .await;
        assert_eq!(status, 403);
        let created = cf(
            &project,
            &state,
            &admin,
            account,
            &[
                "browser-run",
                "devtools",
                "browser",
                "create",
                "--keep-alive",
                "60000",
                "--targets",
            ],
        )
        .await;
        let id = created["sessionId"]
            .as_str()
            .expect("cf must preserve raw creation response");
        assert_eq!(
            created["webSocketDebuggerUrl"],
            format!(
                "ws://{admin}/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}"
            )
        );
        assert!(created["targets"].as_array().unwrap().iter().any(|target| {
            target["devtoolsFrontendUrl"]
                .as_str()
                .is_some_and(|url| url.contains("#jwt="))
        }));
        let sessions = cf(
            &project,
            &state,
            &admin,
            account,
            &["browser-run", "devtools", "session", "list"],
        )
        .await;
        assert!(
            sessions
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["sessionId"] == id)
        );
        connections::exercise(&admin, account, id, &observer).await;
        devtools::exercise(&client, &admin, account, id, &observer).await;
        browser_metrics(
            &client,
            &admin,
            &internal_account,
            1,
            &[("connect", "success"), ("command", "success")],
        )
        .await;
        // Disconnect leaves a live lease; exercise frames after the single-session metric assertion.
        native::fixed_client_frames(&client, &public, &host, true).await;
        process.stop().await;
        process.restart(&config, &log);
        platform_process::ready(&client, admin.parse().unwrap(), &mut process).await;
        browser_metrics(&client, &admin, &internal_account, 0, &[]).await;
        let (status, _, _) = request(
            &client,
            &admin,
            &admin,
            &format!("{base}/devtools/browser/{id}"),
            "GET",
            Some("workflow-deployer"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 404, "old session survived daemon generation");
        let (status, _, bytes) = request(
            &client,
            &public,
            &host,
            "/history",
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
        let history: Value = serde_json::from_slice(&bytes).unwrap();
        let lost = history
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["sessionId"] == id)
            .unwrap();
        assert_eq!(lost["closeReason"], 0);
        assert_eq!(lost["closeReasonText"], "Unknown");
        assert!(lost["endTime"].is_number());
        let (status, _, bytes) = request(
            &client,
            &public,
            &host,
            "/puppeteer?disconnect=true",
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
        assert!(String::from_utf8_lossy(&bytes).contains("Isolated"));
        let output: Value = serde_json::from_slice(&bytes).unwrap();
        let id = output["sessionId"].as_str().unwrap();
        let deleted = cf(
            &project,
            &root.join("cf-state"),
            &admin,
            account,
            &[
                "browser-run",
                "devtools",
                "browser",
                "delete",
                id,
                "--force",
            ],
        )
        .await;
        assert_eq!(deleted["status"], "closed", "{deleted}");
        tokio::time::timeout(Duration::from_secs(3), async {
            while observer.is_alive() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("public delete did not close the physical CDP browser");
    }
    process.stop().await;
    chrome
        .shutdown(Duration::from_millis(100), Duration::from_secs(1))
        .await;
    assert!(!chrome_dir.join("chrome.lease").exists());
    let diagnostics = fs::read_to_string(&log).unwrap();
    for key in [
        "custom-request-key",
        "fallback-request-key",
        "rejected-request-key",
    ] {
        assert!(
            !diagnostics.contains(key),
            "request credential leaked to daemon log"
        );
    }
    model_task.abort();
    let _ = model_task.await;
}
