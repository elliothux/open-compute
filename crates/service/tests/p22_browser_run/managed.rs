//! Managed product fixture uses the same installed shell, pinned workerd and official producer.

use super::*;

#[tokio::test]
async fn managed_puppeteer_actions_frames_and_restart_use_real_browser() {
    exercise(false).await;
}

#[tokio::test]
async fn managed_playwright_default_page_uses_real_browser() {
    exercise(true).await;
}

async fn exercise(playwright: bool) {
    let purpose = repo().join(".temp/p22-product");
    fs::create_dir_all(&purpose).unwrap();
    let root = tempfile::Builder::new()
        .prefix("managed-")
        .tempdir_in(purpose)
        .unwrap()
        .keep();
    let executable = std::env::var_os("OPEN_COMPUTE_TEST_BROWSER")
        .expect("explicit chrome-headless-shell fixture required");
    let executable = executable.into_string().unwrap();
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
    let config = platform_process::config(&root, &data, &mock.endpoint, public, admin);
    let mut text = fs::read_to_string(&config)
        .unwrap()
        .replace("[runtime]\n", "[runtime]\ndrain_timeout_ms = 1000\n");
    text.push_str(&format!(
        r#"
[browser]
max_sessions = 4
max_pending_acquires = 4
acquire_timeout_ms = 10000
command_timeout_ms = 10000
max_connections = 8
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
kind = "managed"
executable = {}
browser_idle_timeout_ms = 1000
shutdown_grace_ms = 100
"#,
        serde_json::to_string(&executable).unwrap()
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
    assert!(
        !data.join("runtime/browser").exists(),
        "startup must not prewarm a browser"
    );
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
    cf(
        &project,
        &root.join("client-config"),
        &admin,
        account,
        &["deploy", "--prebuilt", "--mode", "production"],
    )
    .await;
    let host = format!("browser-run-fixture.{internal_account}.localhost");
    if playwright {
        let (status, media, bytes) = request(
            &client,
            &public,
            &host,
            "/playwright",
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(
            status,
            200,
            "Playwright default page: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(media.starts_with("application/json"));
        assert!(String::from_utf8_lossy(&bytes).contains("Playwright"));
        downloads::check(&bytes);
    } else {
        for (path, media, expected) in [
            ("/puppeteer", "application/json", "Isolated"),
            ("/content", "application/json", "rendered"),
            ("/screenshot", "image/png", ""),
            ("/pdf", "application/pdf", "%PDF-"),
            ("/scrape", "application/json", "rendered"),
            ("/links", "application/json", "https://example.com/linked"),
            ("/snapshot", "application/json", "accessibilityTree"),
            ("/accessibilityTree", "application/json", "Submit"),
            ("/markdown", "application/json", "# rendered"),
            ("/json", "application/json", "heading"),
            ("/json-custom", "application/json", "custom"),
            ("/json-custom-workers-ai", "application/json", "rendered"),
            ("/json-custom-fallback", "application/json", "fallback"),
            ("/json-object", "application/json", "heading"),
            ("/json-schema", "application/json", "heading"),
        ] {
            let (status, actual_media, bytes) =
                request(&client, &public, &host, path, "GET", None, Value::Null).await;
            assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&bytes));
            assert!(actual_media.starts_with(media), "{path}: {actual_media}");
            if path == "/screenshot" {
                assert!(bytes.starts_with(b"\x89PNG"));
            } else {
                assert!(
                    String::from_utf8_lossy(&bytes).contains(expected),
                    "{path}: {}",
                    String::from_utf8_lossy(&bytes)
                );
            }
        }
        native::fixed_client_frames(&client, &public, &host, false).await;
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
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let id = value["sessionId"].as_str().unwrap();
        connections::wire(&admin, account, id).await;
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
        let history = history.as_array().unwrap();
        assert!(!history.iter().any(|row| row["sessionId"] == id));
        assert!(history.iter().all(|row| row["endTime"].is_number()
            && row["closeReason"].is_number()
            && row["closeReasonText"].is_string()));
        let mut first = connections::connect(&admin, account, id, "").await;
        let mut second = connections::connect(&admin, account, id, "").await;
        let one = connections::command(
            &mut first,
            1,
            "Target.createBrowserContext",
            json!({"disposeOnDetach":true}),
        )
        .await;
        let two = connections::command(
            &mut second,
            1,
            "Target.createBrowserContext",
            json!({"disposeOnDetach":true}),
        )
        .await;
        assert_ne!(one["browserContextId"], two["browserContextId"]);
        first.close(None).await.unwrap();
        drop(first);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let contexts =
                    connections::command(&mut second, 2, "Target.getBrowserContexts", json!({}))
                        .await;
                if contexts["browserContextIds"] == json!([two["browserContextId"]]) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        connections::churn(&mut second).await;
        second.close(None).await.unwrap();
        drop(second);
        let stop = format!("/operator/api/instances/{internal_account}/stop");
        let (status, _, _) = request(
            &client,
            &admin,
            &admin,
            &stop,
            "POST",
            Some("workflow-admin"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 202);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (status, _, bytes) = request(
                    &client,
                    &admin,
                    &admin,
                    "/operator/api/instances",
                    "GET",
                    Some("workflow-admin"),
                    Value::Null,
                )
                .await;
                assert_eq!(status, 200);
                let listing: Value = serde_json::from_slice(&bytes).unwrap();
                let row = listing["instances"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["instance_id"] == internal_account)
                    .unwrap();
                assert_ne!(row["state"], "failed", "{listing}");
                if row["state"] == "stopped" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("instance stop must finish its observers and native browser cleanup");
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "instance stop terminated the daemon"
        );
        assert!(
            fs::read_dir(data.join("runtime/browser"))
                .unwrap()
                .next()
                .is_none(),
            "stopped instance retained a native browser generation"
        );
        let start = format!("/operator/api/instances/{internal_account}/start");
        let (status, _, _) = request(
            &client,
            &admin,
            &admin,
            &start,
            "POST",
            Some("workflow-admin"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 202);
        platform_process::ready(&client, admin.parse().unwrap(), &mut process).await;
        process.stop().await;
        let current = fs::read_to_string(&config).unwrap();
        fs::write(
            &config,
            current.replace("default_generation_model = \"fixture/browser-json\"\n", ""),
        )
        .unwrap();
        process.restart(&config, &log);
        platform_process::ready(&client, admin.parse().unwrap(), &mut process).await;
        let base = format!("/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}");
        let (status, _, _) = request(
            &client,
            &admin,
            &admin,
            &base,
            "GET",
            Some("workflow-deployer"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 404, "managed session survived daemon restart");
        let (status, _, bytes) = request(
            &client,
            &public,
            &host,
            "/puppeteer",
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "restart: {}", String::from_utf8_lossy(&bytes));
        let (status, _, bytes) = request(
            &client,
            &public,
            &host,
            "/json-custom",
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(
            status,
            200,
            "custom without default: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(String::from_utf8_lossy(&bytes).contains("custom"));
        let (status, _, bytes) =
            request(&client, &public, &host, "/json", "GET", None, Value::Null).await;
        assert_eq!(status, 503);
        assert!(String::from_utf8_lossy(&bytes).contains("BROWSER_UNAVAILABLE"));
    }
    browser_metrics(
        &client,
        &admin,
        &internal_account,
        0,
        &[("acquire", "success"), ("command", "success")],
    )
    .await;
    process.stop().await;
    assert!(
        fs::read_dir(data.join("runtime/browser"))
            .unwrap()
            .next()
            .is_none(),
        "browser generation was not cleaned after reap"
    );
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
