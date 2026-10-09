use super::*;
use crate::p3_3_test_support::RuntimeFeatureFixture;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use open_compute_core::{BrowserBackendConfig, DeploymentId, VersionId, WorkerId};
use open_compute_storage::runtime_features::version_runtime_features;
use open_compute_workers::VersionRuntimeFeatures;
use std::fs;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

pub(super) fn metrics() -> Arc<MetricsRegistry> {
    Arc::new(
        MetricsRegistry::new(
            &open_compute_core::MetricsConfig::default(),
            "test",
            "workerd",
        )
        .unwrap(),
    )
}

pub(super) fn config(url: String) -> BrowserConfig {
    BrowserConfig {
        public_origin: None,
        max_sessions: 1,
        max_pending_acquires: 1,
        acquire_timeout_ms: 150,
        command_timeout_ms: 100,
        max_connections: 1,
        max_actions: 1,
        max_body_bytes: 4096,
        max_download_bytes: 1024 * 1024,
        max_download_files: 16,
        max_result_bytes: 4096,
        max_message_bytes: 4096,
        max_queued_messages: 4,
        max_history_entries: 1000,
        history_retention_ms: 86_400_000,
        max_frontend_requests: 64,
        backend: BrowserBackendConfig::Cdp {
            url,
            authorization: None,
        },
    }
}

#[tokio::test]
async fn managed_frontend_and_protocol_use_prepared_resources_and_session_authority() {
    let fixture = RuntimeFeatureFixture::create(VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".into(),
        browsers: vec!["BROWSER".into()],
        ..VersionRuntimeFeatures::default()
    })
    .await;
    let mut config = config(String::new());
    config.acquire_timeout_ms = 15_000;
    config.command_timeout_ms = 5_000;
    config.max_sessions = 2;
    config.max_connections = 8;
    config.max_message_bytes = 4 * 1024 * 1024;
    config.max_queued_messages = 256;
    config.max_result_bytes = 4 * 1024 * 1024;
    config.backend = BrowserBackendConfig::Managed {
        executable: std::env::var_os("OPEN_COMPUTE_TEST_BROWSER")
            .expect("prepared chrome-headless-shell required")
            .into(),
        browser_idle_timeout_ms: 10,
        shutdown_grace_ms: 100,
    };
    let service = BrowserService::new(
        fixture.storage.clone(),
        config,
        None,
        open_compute_core::AiConfig::default(),
        None,
        metrics(),
    )
    .unwrap();
    let id = service.acquire(60_000).await.unwrap();
    let (html, media) = service.frontend(&id, "inspector.html").await.unwrap();
    assert_eq!(media, "text/html; charset=utf-8");
    assert!(std::str::from_utf8(&html).unwrap().contains("inspector.js"));
    assert!(
        service
            .frontend("unknown-session", "inspector.html")
            .await
            .is_err()
    );
    assert!(service.frontend(&id, "../outside").await.is_err());
    let protocol = service
        .devtools(
            &id,
            "json/protocol",
            &axum::http::Method::GET,
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(protocol["version"]["major"], "1");
    assert!(
        protocol["domains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|domain| domain["domain"] == "Debugger")
    );
    inspect_managed_frontend(&service, &id).await;
    service.close(&id, false).await.unwrap();
    assert!(service.frontend(&id, "inspector.html").await.is_err());
    assert!(
        service
            .devtools(
                &id,
                "json/protocol",
                &axum::http::Method::GET,
                &BTreeMap::new()
            )
            .await
            .is_err()
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn managed_shutdown_reaps_the_browser_despite_session_authority_failures() {
    for poison in [false, true] {
        let fixture = RuntimeFeatureFixture::create(VersionRuntimeFeatures {
            compatibility_date: "2026-09-08".into(),
            browsers: vec!["BROWSER".into()],
            ..VersionRuntimeFeatures::default()
        })
        .await;
        let mut limits = config(String::new());
        limits.acquire_timeout_ms = 15_000;
        limits.command_timeout_ms = 5_000;
        limits.max_queued_messages = 256;
        limits.backend = BrowserBackendConfig::Managed {
            executable: std::env::var_os("OPEN_COMPUTE_TEST_BROWSER")
                .expect("prepared chrome-headless-shell required")
                .into(),
            browser_idle_timeout_ms: 60_000,
            shutdown_grace_ms: 100,
        };
        let service = BrowserService::new(
            fixture.storage.clone(),
            limits,
            None,
            open_compute_core::AiConfig::default(),
            None,
            metrics(),
        )
        .unwrap();
        service.acquire(60_000).await.unwrap();
        let generations = fixture.storage.data_dir().runtime_dir().join("browser");
        assert!(fs::read_dir(&generations).unwrap().next().is_some());
        if poison {
            let broken = service.clone();
            assert!(
                std::thread::spawn(move || {
                    let _guard = broken.sessions.lock().unwrap();
                    panic!("session store poison");
                })
                .join()
                .is_err()
            );
        } else {
            rusqlite::Connection::open(fixture.storage.data_dir().control_db_path())
                .unwrap()
                .execute_batch("CREATE TRIGGER reject_browser_close BEFORE UPDATE ON browser_sessions WHEN NEW.state='closing' BEGIN SELECT RAISE(ABORT, 'close failure'); END;")
                .unwrap();
        }
        assert_eq!(
            service.shutdown().await.unwrap_err().code(),
            if poison {
                ErrorCode::BrowserUnavailable
            } else {
                ErrorCode::ResourceInvariantViolation
            }
        );
        assert!(!service.is_available());
        assert!(service.capacity.is_closed());
        assert!(fs::read_dir(&generations).unwrap().next().is_none());
    }
}

async fn inspect_managed_frontend(service: &Arc<BrowserService>, id: &str) {
    use crate::cloudflare_v4::accounts::V4InstanceContext;
    use crate::health::HealthCoordinator;
    use crate::http::HttpState;
    use crate::metrics::MetricsRegistry;
    use open_compute_core::{MetricsConfig, SecretString};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap()),
        false,
        Some(SecretString::new("browser-admin")),
    )
    .with_v4_instance_context(V4InstanceContext::new(service.instance, 1_000))
    .with_control_origin_addr(address)
    .with_browser(Some(service.clone()));
    let app = crate::http::admin_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let target = service
        .devtools(id, "json/new", &axum::http::Method::PUT, &BTreeMap::new())
        .await
        .unwrap();
    let target = target["id"].as_str().unwrap();
    let native = &service.session(id).unwrap().cdp;
    let attachment = native
        .command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let attachment = attachment["result"]["sessionId"].as_str().unwrap();
    native
        .command(
            "Runtime.evaluate",
            json!({"expression":"globalThis.managedFrontendMarker=42"}),
            Some(attachment),
        )
        .await
        .unwrap();
    let origin = format!(
        "http://{}.localhost:{}/client/v4/accounts/{}/browser-rendering/live/",
        service.instance,
        address.port(),
        service.instance
    );
    let view = service.live_view(id, b"{}", &origin).await.unwrap();
    let frontend = view["devtoolsFrontendUrl"].as_str().unwrap();
    let observer_id = service.acquire(60_000).await.unwrap();
    let observer_session = service.session(&observer_id).unwrap();
    let observer = &observer_session.cdp;
    let page = observer
        .command("Target.createTarget", json!({"url":frontend}), None)
        .await
        .unwrap();
    let page = page["result"]["targetId"].as_str().unwrap();
    let attached = observer
        .command(
            "Target.attachToTarget",
            json!({"targetId":page,"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let attachment = attached["result"]["sessionId"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let reply = observer.command("Runtime.evaluate", json!({"expression":"!!document.querySelector('iframe')?.contentDocument?.querySelector('.root-view')","returnByValue":true}), Some(attachment)).await.unwrap();
            if reply.pointer("/result/result/value") == Some(&json!(true)) { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();
    let tree = observer
        .command("Page.getFrameTree", json!({}), Some(attachment))
        .await
        .unwrap();
    let frame = tree
        .pointer("/result/frameTree/childFrames/0/frame/id")
        .unwrap()
        .as_str()
        .unwrap();
    let mut events = observer.subscribe();
    observer
        .command("Runtime.enable", json!({}), Some(attachment))
        .await
        .unwrap();
    let context = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["method"] == "Runtime.executionContextCreated"
                && event
                    .pointer("/params/context/auxData/frameId")
                    .and_then(Value::as_str)
                    == Some(frame)
                && event.pointer("/params/context/auxData/isDefault") == Some(&json!(true))
            {
                break event["params"]["context"]["id"].clone();
            }
        }
    })
    .await
    .unwrap();
    let expression = r#"(async()=>{
        const {TargetManager}=await import('./core/sdk/sdk.js');
        const target=TargetManager.TargetManager.instance().primaryPageTarget();
        if(!target)return {ready:false};
        const result=await target.runtimeAgent().invoke_evaluate({expression:'globalThis.managedFrontendMarker',returnByValue:true});
        return {ready:true,marker:result.result?.value,error:result.getError()};
    })()"#;
    let mut last = Value::Null;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let reply = observer
                .command(
                    "Runtime.evaluate",
                    json!({"expression":expression,"contextId":context,"awaitPromise":true,"returnByValue":true}),
                    Some(attachment),
                )
                .await
                .unwrap();
            if reply.pointer("/result/result/value/marker") == Some(&json!(42)) {
                break;
            }
            last = reply;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "managed DevTools did not evaluate its owned target: {last}"
    );
    service.close(&observer_id, false).await.unwrap();
    server.abort();
    let _ = server.await;
}

pub(super) async fn fixture() -> (
    RuntimeFeatureFixture,
    Arc<BrowserService>,
    tokio::task::JoinHandle<()>,
) {
    let fixture = RuntimeFeatureFixture::create(VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".into(),
        browsers: vec!["BROWSER".into()],
        ..VersionRuntimeFeatures::default()
    })
    .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "ws://{}/devtools/browser/{}",
        listener.local_addr().unwrap(),
        uuid::Uuid::now_v7()
    );
    let peer = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                while let Some(Ok(Message::Text(text))) = socket.next().await {
                    let command: Value = serde_json::from_str(&text).unwrap();
                    let result = match command["method"].as_str().unwrap() {
                        "Browser.getVersion" => {
                            json!({"product":"HeadlessChrome/153.0.8010.12","protocolVersion":"1.3","userAgent":"HeadlessChrome/153.0.8010.12","jsVersion":"fixture-v8","revision":"@fixture-revision"})
                        }
                        "Runtime.evaluate" => json!({"result":{"type":"number","value":2}}),
                        "Target.getTargets" => {
                            json!({"targetInfos":[
                                {"targetId":"fixture-page","type":"page","url":"about:blank","title":"Fixture"},
                                {"targetId":"fixture-service-worker","type":"service_worker","url":"https://example.com/sw.js","title":"Worker"}
                            ]})
                        }
                        _ => json!({}),
                    };
                    if socket
                        .send(Message::Text(
                            json!({"id":command["id"],"result":result})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    });
    let mut limits = config(url);
    limits.acquire_timeout_ms = 5_000;
    limits.command_timeout_ms = 2_000;
    let service = BrowserService::new(
        fixture.storage.clone(),
        limits,
        None,
        open_compute_core::AiConfig::default(),
        None,
        metrics(),
    )
    .unwrap();
    (fixture, service, peer)
}

fn request(fixture: &RuntimeFeatureFixture, path: &str, method: &str, body: Body) -> Request<Body> {
    let (_, bindings) = version_runtime_features(fixture.storage.db(), fixture.version).unwrap();
    let browser = bindings
        .iter()
        .find(|binding| binding.name == "BROWSER")
        .unwrap();
    Request::builder()
        .method(method)
        .uri(path)
        .header("x-open-compute-instance-id", fixture.account.to_string())
        .header("x-open-compute-worker-id", fixture.worker.to_string())
        .header("x-open-compute-version-id", fixture.version.to_string())
        .header("x-open-compute-binding-name", "BROWSER")
        .header(
            "x-open-compute-descriptor-sha256",
            hex::encode(browser.descriptor_sha256),
        )
        .header("x-open-compute-capability-version", "1")
        .body(body)
        .unwrap()
}

#[tokio::test]
async fn binding_authority_routes_capacity_activity_close_and_restart_are_fenced() {
    let (fixture, service, peer) = fixture().await;
    for (header, value) in [
        (
            "x-open-compute-instance-id",
            InstanceId::generate().to_string(),
        ),
        ("x-open-compute-worker-id", WorkerId::generate().to_string()),
        (
            "x-open-compute-version-id",
            VersionId::generate().to_string(),
        ),
        ("x-open-compute-binding-name", "UNBOUND".into()),
        ("x-open-compute-descriptor-sha256", "00".repeat(32)),
        ("x-open-compute-capability-version", "2".into()),
        (
            "x-open-compute-deployment-id",
            DeploymentId::generate().to_string(),
        ),
    ] {
        let mut bad = request(
            &fixture,
            "/internal/browser/v1/sessions",
            "GET",
            Body::empty(),
        );
        bad.headers_mut().insert(
            axum::http::HeaderName::from_static(header),
            value.parse().unwrap(),
        );
        assert_eq!(service.handle(bad).await.status(), StatusCode::NOT_FOUND);
    }
    let response = service
        .handle(request(
            &fixture,
            "/internal/browser/v1/devtools/browser?keep_alive=60000",
            "POST",
            Body::empty(),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let id = response["sessionId"].as_str().unwrap();
    assert_eq!(
        service.list(false, 10, 0).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(service.limits().unwrap()["allowedBrowserAcquisitions"], 0);
    assert!(
        service
            .metrics
            .render(&open_compute_core::PlatformStatus::starting())
            .contains("browser_active_sessions 1")
    );
    assert_eq!(
        service.acquire(60_000).await.unwrap_err().code(),
        ErrorCode::BrowserTimeout
    );
    let (connection, _) = service.attach(id, None).await.unwrap();
    let cdp = connection.cdp.clone();
    assert!(
        matches!(service.attach(id, None).await, Err(error) if error.code() == ErrorCode::BrowserLimitExceeded)
    );
    assert!(service.list(false, 10, 0).unwrap()["sessions"][0]["connectionId"].is_string());
    let response = service
        .command(
            id,
            &cdp,
            json!({"id":37,"method":"Runtime.evaluate","params":{"expression":"1+1"}}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response["id"], 37);
    assert_eq!(response["result"]["result"]["value"], 2);
    let negative = service
        .command(
            id,
            &cdp,
            json!({"id":-7,"method":"Browser.getVersion"}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(negative["id"], -7);
    assert!(
        service
            .command(
                id,
                &cdp,
                json!({"id":38,"method":"Browser.getVersion","secret":true}),
                None,
            )
            .await
            .is_err()
    );
    drop(connection);
    assert!(
        service.list(false, 10, 0).unwrap()["sessions"][0]
            .get("connectionId")
            .is_none()
    );
    service
        .command(id, &cdp, json!({"id":39,"method":"Browser.close"}), None)
        .await
        .unwrap();
    assert!(service.session(id).is_err());
    assert_eq!(
        service.list(true, 10, 0).unwrap()["history"][0]["closeReason"],
        1
    );
    let next = service.acquire(60_000).await.unwrap();
    service.invalidate_sessions().await.unwrap();
    assert!(service.session(&next).is_err());
    let history = service.list(true, 10, 0).unwrap();
    assert_eq!(history["history"].as_array().unwrap().len(), 2);
    assert_eq!(history["history"][0]["closeReason"], 1);
    assert_eq!(history["history"][1]["sessionId"], next);
    assert_eq!(history["history"][1]["closeReason"], 0);
    assert_eq!(history["history"][1]["closeReasonText"], "Unknown");
    service.shutdown().await.unwrap();
    assert!(!service.is_available());
    let text = service
        .metrics
        .render(&open_compute_core::PlatformStatus::starting());
    for metric in [
        "browser_operations_total{operation=\"acquire\",outcome=\"success\"} 2",
        "browser_operations_total{operation=\"acquire\",outcome=\"timeout\"} 1",
        "browser_operations_total{operation=\"connect\",outcome=\"success\"} 1",
        "browser_operations_total{operation=\"connect\",outcome=\"limit\"} 1",
        "browser_operations_total{operation=\"command\",outcome=\"failure\"} 1",
        "browser_active_sessions 0",
    ] {
        assert!(text.contains(metric), "missing metric: {metric}");
    }
    assert!(!text.contains(id));
    peer.abort();
}

#[tokio::test]
async fn browser_history_is_bounded_on_close_maintenance_and_restart() {
    let (fixture, original, peer) = fixture().await;
    let mut limits = original.config.clone();
    original.shutdown().await.unwrap();
    limits.max_sessions = 2;
    limits.max_history_entries = 2;
    limits.history_retention_ms = 60_000;
    let service = BrowserService::new(
        fixture.storage.clone(),
        limits.clone(),
        None,
        open_compute_core::AiConfig::default(),
        None,
        metrics(),
    )
    .unwrap();
    let anchor = service.acquire(60_000).await.unwrap();
    for _ in 0..8 {
        let id = service.acquire(60_000).await.unwrap();
        service.close(&id, false).await.unwrap();
        let rows = BrowserSessions::new(fixture.storage.db())
            .list(service.instance, true, 10, 0)
            .unwrap();
        assert!(rows.len() <= 2);
        assert!(rows.iter().all(|row| row.closed_at_ms.is_some()));
        assert!(!rows.iter().any(|row| row.id == anchor));
        let active = BrowserSessions::new(fixture.storage.db())
            .list(service.instance, false, 10, 0)
            .unwrap();
        assert!(
            active
                .iter()
                .any(|row| row.id == anchor && row.state == BrowserSessionState::Ready)
        );
        let history = service.list(true, 10, 0).unwrap();
        assert!(history["history"].as_array().unwrap().iter().all(
            |row| row["endTime"].is_number()
                && row["closeReason"].is_number()
                && row["closeReasonText"].is_string()
        ));
    }
    let expired = BrowserSessionRecord {
        id: uuid::Uuid::now_v7().to_string(),
        instance_id: service.instance,
        generation: uuid::Uuid::now_v7().to_string(),
        contract_sha256: [3; 32],
        state: BrowserSessionState::Ready,
        keep_alive_ms: 60_000,
        connections: 0,
        created_at_ms: now_ms() - 120_000,
        last_activity_at_ms: now_ms() - 120_000,
        connected_at_ms: None,
        closed_at_ms: None,
        close_reason: None,
    };
    let store = BrowserSessions::new(fixture.storage.db());
    store.create(&expired, 2).unwrap();
    store
        .begin_close(service.instance, &expired.id, &expired.generation)
        .unwrap();
    store
        .finish_close(
            service.instance,
            &expired.id,
            &expired.generation,
            BrowserSessionCloseReason::Normal,
            now_ms() - 120_000,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while store
            .get(service.instance, &expired.id, &expired.generation)
            .unwrap()
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(service.session(&anchor).is_ok());
    service.shutdown().await.unwrap();
    let restarted = BrowserService::new(
        fixture.storage.clone(),
        limits,
        None,
        open_compute_core::AiConfig::default(),
        None,
        metrics(),
    )
    .unwrap();
    let rows = store.list(service.instance, true, 10, 0).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.closed_at_ms.is_some()));
    assert!(restarted.session(&anchor).is_err());
    let connection =
        rusqlite::Connection::open(fixture.storage.data_dir().control_db_path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_browser_history_delete BEFORE DELETE ON browser_sessions
             BEGIN SELECT RAISE(ABORT,'retention fixture failure'); END",
        )
        .unwrap();
    let id = restarted.acquire(60_000).await.unwrap();
    assert_eq!(
        restarted.close(&id, false).await.unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert!(
        restarted.list(false, 10, 0).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.list(service.instance, true, 10, 0).unwrap().len(), 3);
    connection
        .execute_batch("DROP TRIGGER reject_browser_history_delete")
        .unwrap();
    let id = restarted.acquire(60_000).await.unwrap();
    restarted.close(&id, false).await.unwrap();
    assert_eq!(store.list(service.instance, true, 10, 0).unwrap().len(), 2);
    restarted.shutdown().await.unwrap();
    peer.abort();
}

#[tokio::test]
async fn binding_unknown_routes_options_duplicate_queries_and_backend_fail_closed() {
    let (fixture, service, peer) = fixture().await;
    for (path, body, expected) in [
        (
            "/internal/browser/v1/devtools/browser?keep_alive=1",
            "",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/internal/browser/v1/devtools/browser?keep_alive=60000&keep_alive=60000",
            "",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/internal/browser/v1/devtools/browser?browser=kitesurf",
            "",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/internal/browser/v1/devtools/browser",
            "{}",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/internal/browser/v1/nonexistent",
            "",
            StatusCode::NOT_IMPLEMENTED,
        ),
    ] {
        assert_eq!(
            service
                .handle(request(&fixture, path, "POST", Body::from(body)))
                .await
                .status(),
            expected
        );
    }
    assert!(service.session("bad-id").is_err());
    assert!(service.list(false, 0, 0).is_err());
    peer.abort();
    assert!(service.acquire(60_000).await.is_err());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn public_browser_api_preserves_raw_sessions_and_enforces_account_and_roles() {
    use crate::cloudflare_v4::accounts::V4InstanceContext;
    use crate::health::HealthCoordinator;
    use crate::http::HttpState;
    use crate::metrics::MetricsRegistry;
    use open_compute_core::{MetricsConfig, SecretString};
    use tower::ServiceExt as _;
    let (fixture, service, peer) = fixture().await;
    let authority = V4InstanceContext::new(fixture.account, 1_000);
    let account = authority.public_id().to_owned();
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap()),
        false,
        Some(SecretString::new("browser-admin")),
    )
    .with_v4_tokens(
        SecretString::new("browser-deployer"),
        SecretString::new("browser-reader"),
    )
    .with_v4_instance_context(authority)
    .with_local_origin_addr("127.0.0.1:8786".parse().unwrap())
    .with_browser(Some(service.clone()));
    let response = crate::http::admin_router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/client/v4/accounts/{account}/browser-rendering/devtools/browser"
                ))
                .header("authorization", "Bearer browser-deployer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 501);
    assert_eq!(service.list(false, 10, 0).unwrap()["sessions"], json!([]));
    let state = state.with_control_origin_addr("127.0.0.1:8787".parse().unwrap());
    let app = crate::http::admin_router(state.clone());
    let base = format!("/client/v4/accounts/{account}/browser-rendering");
    let send = |method: &str, suffix: &str, token: &str, body: &str| {
        app.clone().oneshot(
            Request::builder()
                .method(method)
                .uri(format!("{base}{suffix}"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
    };
    let response = send("GET", "/devtools/session", "browser-reader", "")
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        b"[]"
    );
    assert_eq!(
        send("POST", "/devtools/browser", "browser-reader", "")
            .await
            .unwrap()
            .status(),
        403
    );
    let response = send(
        "POST",
        "/devtools/browser?keep_alive=60000&lab=false",
        "browser-deployer",
        "{}",
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    let created: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let id = created["sessionId"].as_str().unwrap();
    assert_eq!(
        created["webSocketDebuggerUrl"],
        format!(
            "ws://{}.localhost:8787/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}",
            service.instance_id()
        )
    );
    let (connection, _) = service.attach(id, None).await.unwrap();
    let response = send(
        "GET",
        "/devtools/session?limit=10&offset=0",
        "browser-reader",
        "",
    )
    .await
    .unwrap();
    let listed: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(listed.is_array());
    assert_eq!(listed[0]["sessionId"], id);
    assert!(listed[0]["connectionStartTime"].is_number());
    drop(connection);
    for (method, suffix, token, body, status) in [
        (
            "GET",
            "/devtools/session".to_owned(),
            "browser-reader",
            "",
            200,
        ),
        (
            "GET",
            format!("/devtools/browser/{id}/json/version"),
            "browser-reader",
            "",
            200,
        ),
        (
            "PUT",
            format!("/devtools/browser/{id}/json/new"),
            "browser-reader",
            "",
            403,
        ),
        (
            "POST",
            format!("/devtools/browser/{id}/live_view"),
            "browser-reader",
            "{}",
            403,
        ),
        ("POST", "/content".to_owned(), "browser-reader", "{}", 403),
        ("POST", "/content".to_owned(), "browser-deployer", "[]", 400),
        (
            "POST",
            "/devtools/browser".to_owned(),
            "browser-reader",
            "",
            403,
        ),
        (
            "DELETE",
            "/devtools/browser/unknown".to_owned(),
            "browser-deployer",
            "",
            404,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/client/v4/accounts/{account}/browser-run{suffix}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method} {suffix}");
        if status == 200 {
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            if suffix == "/devtools/session" {
                assert_eq!(value[0]["sessionId"], id);
            } else {
                assert_eq!(value["Protocol-Version"], "1.3");
            }
        }
    }
    let response = send(
        "GET",
        &format!("/devtools/browser/{id}/json/list"),
        "browser-reader",
        "",
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    let targets: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(
        targets[0]["devtoolsFrontendUrl"]
            .as_str()
            .unwrap()
            .contains("#jwt=")
    );
    assert!(targets[1].get("devtoolsFrontendUrl").is_none());
    assert!(
        targets[1]["webSocketDebuggerUrl"]
            .as_str()
            .unwrap()
            .ends_with("/page/fixture-service-worker")
    );
    let version = send(
        "GET",
        &format!("/devtools/browser/{id}/json/version"),
        "browser-reader",
        "",
    )
    .await
    .unwrap();
    assert_eq!(version.status(), 200);
    let version: Value =
        serde_json::from_slice(&to_bytes(version.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(version["WebKit-Version"], "537.36 (@fixture-revision)");
    assert_eq!(
        version["webSocketDebuggerUrl"],
        format!(
            "ws://{}.localhost:8787{base}/devtools/browser/{id}",
            fixture.account
        )
    );
    let response = send(
        "POST",
        &format!("/devtools/browser/{id}/live_view"),
        "browser-deployer",
        "{}",
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    let view: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let websocket = url::Url::parse(view["webSocketDebuggerUrl"].as_str().unwrap()).unwrap();
    let frontend = url::Url::parse(view["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
    let jwt = frontend.fragment().unwrap().strip_prefix("jwt=").unwrap();
    let mut inspector = frontend
        .join(&format!("{id}/devtools/inspector.html"))
        .unwrap();
    inspector
        .query_pairs_mut()
        .append_pair("jwt", jwt)
        .append_pair(
            "ws",
            &format!(
                "{}:8787{}?{}",
                websocket.host_str().unwrap(),
                websocket.path(),
                websocket.query().unwrap()
            ),
        );
    let resource = || {
        app.clone().oneshot(
            Request::builder()
                .uri(format!(
                    "{}?{}",
                    inspector.path(),
                    inspector.query().unwrap()
                ))
                .body(Body::empty())
                .unwrap(),
        )
    };
    let full = service.frontend_capacity.acquire_many(64).await.unwrap();
    assert_eq!(resource().await.unwrap().status(), 429);
    drop(full);
    let busy = service.connections.acquire().await.unwrap();
    assert_eq!(resource().await.unwrap().status(), 504);
    drop(busy);
    for (method, suffix, body, status) in [
        (
            "GET",
            format!("/devtools/browser/{id}/json/version"),
            "{}",
            400,
        ),
        (
            "GET",
            format!("/devtools/browser/{id}/json/version?bad=1"),
            "",
            501,
        ),
        (
            "GET",
            format!("/devtools/browser/{id}/json/version?bad=1&bad=2"),
            "",
            400,
        ),
        (
            "GET",
            format!("/devtools/browser/{id}/json/list/bad!id"),
            "",
            404,
        ),
        ("GET", format!("/devtools/browser/{id}/unknown"), "", 501),
        (
            "GET",
            format!("/devtools/browser/{id}/page/bad!id"),
            "",
            400,
        ),
        ("DELETE", format!("/devtools/browser/{id}"), "{}", 400),
        (
            "DELETE",
            format!("/devtools/browser/{id}?force=true"),
            "",
            400,
        ),
    ] {
        assert_eq!(
            send(method, &suffix, "browser-deployer", body)
                .await
                .unwrap()
                .status(),
            status,
            "{suffix}"
        );
    }
    assert_eq!(
        send(
            "GET",
            &format!("/devtools/browser/{id}/page/target"),
            "browser-reader",
            ""
        )
        .await
        .unwrap()
        .status(),
        403
    );
    for (method, suffix, body, status) in [
        ("GET", "/devtools/session?limit=0", "", 400),
        ("GET", "/devtools/session?offset=no", "", 400),
        ("GET", "/devtools/session?limit=1&limit=2", "", 400),
        ("GET", "/devtools/session?unknown=true", "", 400),
        ("POST", "/devtools/browser?lab=true", "", 501),
        ("POST", "/devtools/browser?keep_alive=invalid", "", 400),
        ("POST", "/devtools/browser", "{invalid", 501),
        ("POST", "/devtools/browser", "{\"guardrails\":{}}", 501),
        ("GET", "/devtools/browser/bad-id", "", 404),
        ("DELETE", "/devtools/browser/bad-id?force=invalid", "", 400),
        ("POST", "/content?unknown=true", "{}", 501),
        ("POST", "/content?cacheTTL=x", "{}", 400),
        ("POST", "/content?cacheTTL=86401", "{}", 400),
        ("POST", "/content?cacheTTL=0&cacheTTL=1", "{}", 400),
        ("POST", "/content", "[]", 400),
        (
            "POST",
            "/content",
            "{\"html\":\"page\",\"cacheTTL\":0}",
            400,
        ),
        ("POST", "/content?cacheTTL=0", "{\"html\":\"page\"}", 503),
        ("POST", "/content", "not JSON", 400),
        ("POST", "/content", "{\"html\":\"page\"}", 503),
        ("POST", "/json", "{}", 503),
    ] {
        let response = send(method, suffix, "browser-deployer", body)
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{suffix}");
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("devtools/browser/"));
    }
    assert_eq!(
        send(
            "GET",
            &format!("/devtools/browser/{id}"),
            "browser-deployer",
            ""
        )
        .await
        .unwrap()
        .status(),
        400
    );
    assert_eq!(
        send(
            "DELETE",
            &format!("/devtools/browser/{id}"),
            "browser-deployer",
            ""
        )
        .await
        .unwrap()
        .status(),
        200
    );
    assert!(service.session(id).is_err());
    let no_origin = crate::http::admin_router(
        state.with_control_origin_addr("192.0.2.1:8787".parse().unwrap()),
    );
    let response = no_origin
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{base}/devtools/browser?targets=true"))
                .header("authorization", "Bearer browser-deployer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 501);
    assert_eq!(service.limits().unwrap()["allowedBrowserAcquisitions"], 1);
    assert!(
        service.list(false, 10, 0).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let busy = service.connections.acquire().await.unwrap();
    assert_eq!(
        send(
            "POST",
            "/devtools/browser?targets=true",
            "browser-deployer",
            ""
        )
        .await
        .unwrap()
        .status(),
        429
    );
    drop(busy);
    assert_eq!(service.limits().unwrap()["allowedBrowserAcquisitions"], 1);
    for prefix in ["browser-rendering", "browser-run"] {
        let response = app.clone().oneshot(Request::builder().uri(format!("/client/v4/accounts/00000000000000000000000000000000/{prefix}/devtools/session"))
            .header("authorization","Bearer browser-reader").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), 404);
    }
    service.shutdown().await.unwrap();
    peer.abort();
}

#[tokio::test]
async fn cancelled_acquires_hold_capacity_until_native_readiness_and_cleanup_finish() {
    let (fixture, initial, peer) = fixture().await;
    initial.shutdown().await.unwrap();
    peer.abort();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "ws://{}/devtools/browser/cancelled",
        listener.local_addr().unwrap()
    );
    let server_entered = entered.clone();
    let server_release = release.clone();
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let entered = server_entered.clone();
            let release = server_release.clone();
            tokio::spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                let Some(Ok(Message::Text(request))) = socket.next().await else {
                    return;
                };
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(request["method"], "Browser.getVersion");
                entered.notify_one();
                release.notified().await;
                let _ = socket.send(Message::Text(json!({"id":request["id"],"result":{"product":"HeadlessChrome/153.0.8010.12","protocolVersion":"1.3"}}).to_string().into())).await;
                while socket.next().await.is_some() {}
            });
        }
    });
    let mut limits = config(url);
    limits.acquire_timeout_ms = 500;
    limits.command_timeout_ms = 2_000;
    let service = BrowserService::new(
        fixture.storage.clone(),
        limits,
        None,
        open_compute_core::AiConfig::default(),
        None,
        metrics(),
    )
    .unwrap();
    for cancellation in [true, false] {
        let caller_service = service.clone();
        let caller = tokio::spawn(async move { caller_service.acquire(60_000).await });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        if cancellation {
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
        } else {
            assert_eq!(
                caller.await.unwrap().unwrap_err().code(),
                ErrorCode::BrowserTimeout
            );
        }
        assert_eq!(service.pending.available_permits(), 0);
        assert_eq!(service.capacity.available_permits(), 0);
        assert_eq!(
            service.acquire(60_000).await.unwrap_err().code(),
            ErrorCode::BrowserLimitExceeded
        );
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.pending.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(service.capacity.available_permits(), 1);
        assert_eq!(service.list(false, 10, 0).unwrap()["sessions"], json!([]));
    }
    let capacity = service.capacity.clone().acquire_owned().await.unwrap();
    let caller_service = service.clone();
    let caller = tokio::spawn(async move { caller_service.acquire(60_000).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.pending.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.pending.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(capacity);
    service.shutdown().await.unwrap();
    let text = service
        .metrics
        .render(&open_compute_core::PlatformStatus::starting());
    for metric in [
        "browser_operations_total{operation=\"acquire\",outcome=\"cancelled\"} 2",
        "browser_operations_total{operation=\"acquire\",outcome=\"timeout\"} 1",
        "browser_operations_total{operation=\"acquire\",outcome=\"limit\"} 2",
        "browser_in_flight_operations{operation=\"acquire\"} 0",
        "browser_active_sessions 0",
    ] {
        assert!(text.contains(metric), "missing metric: {metric}");
    }
    server.abort();
}

#[tokio::test]
async fn public_browser_urls_use_operator_origin_and_ignore_forwarded_headers() {
    use crate::cloudflare_v4::accounts::V4InstanceContext;
    use crate::health::HealthCoordinator;
    use crate::http::HttpState;
    use open_compute_core::SecretString;
    use tower::ServiceExt;

    let (fixture, original, peer) = fixture().await;
    original.shutdown().await.unwrap();
    for origin in ["https://control.example:8443/", "http://[2001:db8::1]:8787"] {
        let mut config = original.config.clone();
        config.public_origin = Some(origin.into());
        let service = BrowserService::new(
            fixture.storage.clone(),
            config,
            None,
            open_compute_core::AiConfig::default(),
            None,
            metrics(),
        )
        .unwrap();
        let state = HttpState::for_test(
            HealthCoordinator::new(),
            metrics(),
            false,
            Some(SecretString::new("browser-admin")),
        )
        .with_v4_instance_context(V4InstanceContext::new(service.instance, 1000))
        .with_control_origin_addr("127.0.0.1:8787".parse().unwrap())
        .with_browser(Some(service.clone()));
        let path = format!(
            "/client/v4/accounts/{}/browser-rendering/devtools/browser",
            service.instance
        );
        let response = crate::http::admin_router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header("authorization", "Bearer browser-admin")
                    .header("host", "attacker.example")
                    .header("x-forwarded-host", "attacker.example")
                    .header("x-forwarded-proto", "http")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let id = value["sessionId"].as_str().unwrap();
        let expected = origin.trim_end_matches('/').replacen("http", "ws", 1);
        assert_eq!(
            value["webSocketDebuggerUrl"],
            format!("{expected}{path}/{id}")
        );
        let (_, live_origin) = service.public_urls(id, Some(8787)).unwrap();
        let view = service
            .default_view(id, "fixture-page", &live_origin, false)
            .unwrap();
        assert!(
            view["devtoolsFrontendUrl"]
                .as_str()
                .unwrap()
                .starts_with(origin.trim_end_matches('/'))
        );
        assert!(
            view["webSocketDebuggerUrl"]
                .as_str()
                .unwrap()
                .starts_with(&expected)
        );
        // The viewer account path selects the same instance without relying on a localhost Host.
        let route = url::Url::parse(view["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
        let response = crate::http::admin_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(route.path())
                    .header("host", "attacker.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let foreign = route
            .path()
            .replace(service.instance.as_str(), "foreign-account");
        let response = crate::http::admin_router(state)
            .oneshot(Request::builder().uri(foreign).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        service.shutdown().await.unwrap();
    }
    peer.abort();
}
