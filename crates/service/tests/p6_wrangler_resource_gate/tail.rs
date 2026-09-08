use super::*;

pub(super) async fn exercise_live_tail(
    admin_addr: SocketAddr,
    public_addr: SocketAddr,
    public_account: &str,
    internal_account: &str,
) {
    let root = repo_root();
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new("bun")
            .arg("tests/live-tail-dashboard.mjs")
            .current_dir(root.join("packages/cloudflare-extension"))
            .env(
                "OPEN_COMPUTE_V4_BASE_URL",
                format!("http://{admin_addr}/client/v4"),
            )
            .env("OPEN_COMPUTE_V4_TOKEN", TOKEN)
            .env("OPEN_COMPUTE_V4_ACCOUNT_ID", public_account)
            .env(
                "OPEN_COMPUTE_P7_PUBLIC_URL",
                format!("http://{public_addr}/__workers/{internal_account}/p6-wrangler-resource-gate/live-tail"),
            )
            .env("OPEN_COMPUTE_P7_SECRET", TAIL_SECRET)
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .output(),
    )
    .await
    .expect("Dashboard Live Tail differential timed out")
    .expect("run Dashboard Live Tail differential");
    assert_success(&output);
}

pub(super) fn assert_observability_audit(data: &Path) {
    let connection = rusqlite::Connection::open(data.join("control.sqlite")).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT action, CAST(details_json AS TEXT)
             FROM control_audit_events
             WHERE action LIKE 'worker.tail.%' OR action = 'worker.observability.query'
             ORDER BY seq",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows.iter()
            .filter(|(action, _)| action == "worker.tail.create")
            .count(),
        12
    );
    assert_eq!(
        rows.iter()
            .filter(|(action, _)| action == "worker.tail.delete")
            .count(),
        11
    );
    assert!(
        rows.iter()
            .any(|(action, _)| action == "worker.observability.query")
    );
    for (_, details) in rows {
        assert!(!details.contains(TAIL_SECRET));
        let details: Value = serde_json::from_str(&details).unwrap();
        assert!(
            details
                .as_object()
                .is_some_and(|object| object.keys().all(|key| matches!(
                    key.as_str(),
                    "view" | "fromMs" | "toMs" | "resultCount" | "filterKeys"
                ))),
            "observability audit included content-bearing details: {details}"
        );
    }
}

pub(super) async fn exercise_tail(
    command: &WranglerCommand<'_>,
    admin_addr: SocketAddr,
    public_addr: SocketAddr,
    public_account: &str,
    internal_account: &str,
) {
    fs::write(
        command.project.join("index.ts"),
        "export default { fetch(request) { if (new URL(request.url).pathname.endsWith('/error')) { console.error('p7-tail-error'); throw new Error('p7-tail-error'); } console.log('p7-tail-event invoice'); return new Response('tail-ok'); } };",
    )
    .unwrap();
    assert_success(&command.run(&["deploy", "--config", "wrangler.jsonc"]).await);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http();
    let version = active_version(&client, admin_addr, public_account).await;
    let cases = vec![
        (
            "json",
            vec!["--format=json".to_owned()],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            true,
        ),
        (
            "pretty",
            vec!["--format=pretty".to_owned()],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            false,
        ),
        (
            "status-ok",
            vec!["--format=json".to_owned(), "--status=ok".to_owned()],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            false,
        ),
        (
            "status-error",
            vec!["--format=json".to_owned(), "--status=error".to_owned()],
            "error",
            "GET",
            None,
            "p7-tail-error",
            500,
            false,
        ),
        (
            "method",
            vec![
                "--format=json".to_owned(),
                "--method=GET".to_owned(),
                "--method=POST".to_owned(),
            ],
            "tail-trigger",
            "POST",
            None,
            "p7-tail-event",
            200,
            false,
        ),
        (
            "header",
            vec![
                "--format=json".to_owned(),
                "--header=content-type:json".to_owned(),
            ],
            "tail-trigger",
            "GET",
            Some("application/json"),
            "p7-tail-event",
            200,
            false,
        ),
        (
            "sampling",
            vec![
                "--format=json".to_owned(),
                "--sampling-rate=0.25".to_owned(),
            ],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            false,
        ),
        (
            "search",
            vec!["--format=json".to_owned(), "--search=invoice".to_owned()],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            false,
        ),
        (
            "ip",
            vec!["--format=json".to_owned(), "--ip=self".to_owned()],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            false,
        ),
        (
            "version",
            vec![
                "--format=json".to_owned(),
                format!("--version-id={version}"),
            ],
            "tail-trigger",
            "GET",
            None,
            "p7-tail-event",
            200,
            false,
        ),
    ];
    for (
        name,
        mut options,
        path,
        method,
        content_type,
        marker,
        expected_status,
        verify_persistence,
    ) in cases
    {
        let stdout = command.project.join(format!("tail-{name}.stdout.log"));
        let stderr = command.project.join(format!("tail-{name}.stderr.log"));
        let mut args = vec!["tail".to_owned(), "p6-wrangler-resource-gate".to_owned()];
        args.append(&mut options);
        args.extend(["--config".to_owned(), "wrangler.jsonc".to_owned()]);
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let mut child = command.spawn(&refs, &stdout, &stderr);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "fixed Wrangler tail {name} exited early: {}",
                String::from_utf8_lossy(&fs::read(&stderr).unwrap_or_default())
            );
            let mut request = Request::builder()
                .method(method)
                .uri(format!("http://{public_addr}/__workers/{internal_account}/p6-wrangler-resource-gate/{path}?token={TAIL_SECRET}"))
                .header("authorization", format!("Bearer {TAIL_SECRET}"))
                .header("x-api-key", TAIL_SECRET);
            if let Some(value) = content_type {
                request = request.header("content-type", value);
            }
            let response = tokio::time::timeout(
                Duration::from_secs(3),
                client.request(request.body(Body::empty()).unwrap()),
            )
            .await
            .expect("tail trigger request timed out")
            .expect("tail trigger request failed");
            assert_eq!(response.status().as_u16(), expected_status);
            let _ = to_bytes(Body::new(response.into_body()), 1024)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(75)).await;
            if fs::read(&stdout)
                .unwrap_or_default()
                .windows(marker.len())
                .any(|value| value == marker.as_bytes())
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fixed Wrangler tail {name} produced no matching trace-v1 event"
            );
        }
        if verify_persistence {
            wait_persisted_tail_log(&client, admin_addr, public_account).await;
        }
        let pid = child
            .id()
            .and_then(|value| Pid::from_raw(value as i32))
            .unwrap();
        kill_process(pid, Signal::INT).expect("interrupt fixed Wrangler tail");
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("fixed Wrangler tail SIGINT cleanup timed out")
            .expect("wait for fixed Wrangler tail");
        assert!(
            status.success(),
            "fixed Wrangler tail {name} failed after SIGINT: {status}"
        );
        assert_clean_output(&fs::read(&stdout).unwrap_or_default());
        assert_clean_output(&fs::read(&stderr).unwrap_or_default());
        let deadline = Instant::now() + Duration::from_secs(3);
        while tail_count(&client, admin_addr, public_account).await != 0 {
            assert!(
                Instant::now() < deadline,
                "Wrangler SIGINT did not delete the {name} tail session"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

pub(super) async fn active_version(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
    public_account: &str,
) -> String {
    let request = Request::builder()
        .uri(format!("http://{admin_addr}/client/v4/accounts/{public_account}/workers/scripts/p6-wrangler-resource-gate/versions"))
        .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
        .body(Body::empty()).unwrap();
    let response = client.request(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    envelope["result"]["items"]
        .as_array()
        .and_then(|versions| versions.first())
        .and_then(|version| version["id"].as_str())
        .unwrap()
        .to_owned()
}

pub(super) async fn wait_persisted_tail_log(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
    public_account: &str,
) {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let body = serde_json::to_vec(&serde_json::json!({
        "queryId": "p7-wrangler-tail-persistence",
        "timeframe": {"from": now - 5 * 60_000, "to": now + 60_000},
        "parameters": {"datasets": ["cloudflare-workers"], "filters": []},
        "view": "events",
        "limit": 2_000
    }))
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let request = Request::builder()
            .method("POST")
            .uri(format!(
                "http://{admin_addr}/client/v4/accounts/{public_account}/workers/observability/telemetry/query"
            ))
            .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(body.clone()))
            .unwrap();
        let response = client.request(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(Body::new(response.into_body()), 8 * 1024 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let encoded = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!encoded.contains(TAIL_SECRET));
        if encoded.contains("p7-tail-event") {
            assert!(encoded.contains("REDACTED"));
            let events = value["result"]["events"]["events"].as_array().unwrap();
            assert!(
                events
                    .iter()
                    .all(|event| event["dataset"] == "cloudflare-workers")
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Workers Logs persistence did not commit the tail invocation"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) async fn tail_count(
    client: &platform_process::Client,
    admin_addr: SocketAddr,
    public_account: &str,
) -> usize {
    let request = Request::builder()
        .uri(format!(
            "http://{admin_addr}/client/v4/accounts/{public_account}/workers/scripts/p6-wrangler-resource-gate/tails"
        ))
        .header("authorization", format!("Bearer {READ_ONLY_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .expect("tail session list timed out")
        .expect("tail session list failed");
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(Body::new(response.into_body()), 64 * 1024)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    envelope["result"].as_array().unwrap().len()
}
