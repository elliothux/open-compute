use super::*;

#[tokio::test]
async fn cdp_live_view_uses_owned_current_targets_and_one_admitted_connection() {
    use crate::p3_3_test_support::RuntimeFeatureFixture;
    use open_compute_core::BrowserBackendConfig;
    use open_compute_workers::VersionRuntimeFeatures;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = RuntimeFeatureFixture::create(VersionRuntimeFeatures {
        compatibility_date: "2026-09-08".into(),
        browsers: vec!["BROWSER".into()],
        ..VersionRuntimeFeatures::default()
    })
    .await;
    let mut config = super::super::tests::config(String::new());
    config.acquire_timeout_ms = 15_000;
    config.command_timeout_ms = 5_000;
    config.max_sessions = 2;
    config.max_queued_messages = 256;
    config.backend = BrowserBackendConfig::Managed {
        executable: std::env::var_os("OPEN_COMPUTE_TEST_BROWSER")
            .expect("prepared headless-shell required")
            .into(),
        browser_idle_timeout_ms: 10,
        shutdown_grace_ms: 100,
    };
    let service = BrowserService::new(
        fixture.storage.clone(),
        config,
        None,
        open_compute_core::AiConfig::default(),
        Some(listener.local_addr().unwrap()),
        crate::browser::tests::metrics(),
    )
    .unwrap();
    let id = service.acquire(60_000).await.unwrap();
    let session = service.session(&id).unwrap();
    let mut targets = Vec::new();
    for _ in 0..2 {
        let reply = session
            .cdp
            .command("Target.createTarget", json!({"url":"about:blank"}), None)
            .await
            .unwrap();
        targets.push(reply["result"]["targetId"].as_str().unwrap().to_owned());
    }
    let (connection, _) = service.attach(&id, None).await.unwrap();
    let native = connection
        .cdp
        .command(
            "Target.attachToTarget",
            json!({"targetId":targets[1],"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let attachment = native["result"]["sessionId"].as_str().unwrap();
    let other = service.acquire(60_000).await.unwrap();
    let foreign = service
        .session(&other)
        .unwrap()
        .cdp
        .command("Target.createTarget", json!({"url":"about:blank"}), None)
        .await
        .unwrap();
    for (params, current, expected) in [
        (
            json!({"mode":"tab","expiresInMs":60_000}),
            Some(attachment),
            targets[1].as_str(),
        ),
        (
            json!({"targetId":targets[0]}),
            Some(attachment),
            targets[0].as_str(),
        ),
        (
            json!({"targetId":targets[1],"mode":"full"}),
            None,
            targets[1].as_str(),
        ),
    ] {
        let mut request = json!({"id":42,"method":"Cloudflare.getLiveView","params":params});
        if let Some(current) = current {
            request["sessionId"] = current.into();
        }
        let reply = service
            .command(&id, &connection.cdp, request, None)
            .await
            .unwrap();
        assert_eq!(reply["id"], 42);
        assert_eq!(reply["result"].as_object().unwrap().len(), 1);
        let url =
            url::Url::parse(reply["result"]["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
        assert_eq!(url.port(), Some(listener.local_addr().unwrap().port()));
        let jwt = url.fragment().unwrap().strip_prefix("jwt=").unwrap();
        let claims = service.claim(jwt, &id, true).unwrap();
        assert_eq!(claims.target, expected);
        assert!(!claims.readonly);
    }
    for params in [
        json!({"guardrails":{"mode":"readonly"}}),
        json!({"origin":"http://outside/"}),
        json!({"expiresInMs":1}),
        json!({"mode":"invalid"}),
        json!({"targetId":foreign["result"]["targetId"]}),
    ] {
        assert!(
            service
                .command(
                    &id,
                    &connection.cdp,
                    json!({"id":1,"method":"Cloudflare.getLiveView","params":params}),
                    None
                )
                .await
                .is_err()
        );
    }
    assert!(
        service
            .command(
                &id,
                &connection.cdp,
                json!({"id":1,"method":"Cloudflare.getLiveView","sessionId":"foreign-attachment"}),
                None
            )
            .await
            .is_err()
    );
    drop(connection);
    let (page, _) = service.attach(&id, Some(&targets[1])).await.unwrap();
    let reply = service
        .command(
            &id,
            &page.cdp,
            json!({"id":5,"method":"Cloudflare.getLiveView"}),
            page.target.as_deref(),
        )
        .await
        .unwrap();
    let url = url::Url::parse(reply["result"]["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
    let jwt = url.fragment().unwrap().strip_prefix("jwt=").unwrap();
    assert_eq!(service.claim(jwt, &id, true).unwrap().target, targets[1]);
    drop(page);
    use crate::cloudflare_v4::accounts::V4InstanceContext;
    use crate::health::HealthCoordinator;
    use crate::http::HttpState;
    use crate::metrics::MetricsRegistry;
    use futures::{SinkExt, StreamExt};
    use open_compute_core::{MetricsConfig, SecretString};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let address = listener.local_addr().unwrap();
    let authority = V4InstanceContext::new(service.instance, 1_000);
    let account = authority.public_id().to_owned();
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap()),
        false,
        Some(SecretString::new("browser-admin")),
    )
    .with_v4_instance_context(authority)
    .with_control_origin_addr(address)
    .with_browser(Some(service.clone()));
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::http::admin_router(state))
            .await
            .unwrap();
    });
    let endpoint = format!(
        "ws://{address}/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}/page/{}",
        targets[1]
    );
    let mut request = endpoint.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer browser-admin".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(Message::Text(
            json!({"id":7,"method":"Cloudflare.getLiveView","params":{"mode":"tab"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(reply["id"], 7);
    let url = url::Url::parse(reply["result"]["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
    let wire_jwt = url.fragment().unwrap().strip_prefix("jwt=").unwrap();
    assert_eq!(
        service.claim(wire_jwt, &id, true).unwrap().target,
        targets[1]
    );
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while service.connections.available_permits() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (_, origin) = service.public_urls(&id, None).unwrap();
    let view = service
        .default_view(&id, &targets[1], &origin, true)
        .unwrap();
    let mut endpoint = url::Url::parse(view["webSocketDebuggerUrl"].as_str().unwrap()).unwrap();
    endpoint.set_host(Some("127.0.0.1")).unwrap();
    let (mut readonly, _) = tokio_tungstenite::connect_async(endpoint.as_str())
        .await
        .unwrap();
    readonly
        .send(Message::Text(
            json!({"id":8,"method":"Cloudflare.getLiveView"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), readonly.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(reply["error"]["message"], "BROWSER_UNSUPPORTED");
    readonly.close(None).await.unwrap();
    service.close(&id, false).await.unwrap();
    assert!(service.claim(jwt, &id, true).is_err());
    service.close(&other, false).await.unwrap();
    service.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn cdp_live_view_without_an_operator_listener_is_explicitly_unsupported() {
    let (_fixture, service, peer) = super::super::tests::fixture().await;
    let id = service.acquire(60_000).await.unwrap();
    let cdp = service.session(&id).unwrap().cdp.clone();
    let error = service
        .command(
            &id,
            &cdp,
            json!({"id":1,"method":"Cloudflare.getLiveView"}),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BrowserUnsupported);
    service.shutdown().await.unwrap();
    peer.abort();
}

fn token_from(value: &Value) -> String {
    let url = url::Url::parse(value["webSocketDebuggerUrl"].as_str().unwrap()).unwrap();
    url.query_pairs()
        .find(|(name, _)| name == "jwt")
        .unwrap()
        .1
        .into_owned()
}
fn sign(service: &BrowserService, claims: &Claims) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    let message = format!("{HEADER}.{encoded}");
    let mut signer = Signer::new_from_slice(service.live_key.as_ref()).unwrap();
    signer.update(message.as_bytes());
    format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes())
    )
}

#[tokio::test]
async fn viewer_tickets_fence_signature_scope_expiry_generation_and_terminal_sessions() {
    let (_fixture, service, peer) = super::super::tests::fixture().await;
    let id = service.acquire(60_000).await.unwrap();
    let origin = "http://localhost:9222/client/v4/accounts/test/browser-rendering/live/";
    for mode in ["tab", "full", "devtools"] {
        let options = json!({"mode":mode,"guardrails":{"mode":"readonly"},"expiresInMs":60_000,"targetId":"fixture-page"}).to_string();
        let result = service
            .live_view(&id, options.as_bytes(), origin)
            .await
            .unwrap();
        assert_eq!(
            result["options"],
            json!({"mode":mode,"guardrails":{"mode":"readonly"}})
        );
        assert_eq!(result["id"], "fixture-page");
        let token = token_from(&result);
        let mut claims = service.claim(&token, &id, true).unwrap();
        assert!(claims.readonly);
        let frontend = url::Url::parse(result["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
        assert_eq!(
            frontend.path(),
            "/client/v4/accounts/test/browser-rendering/live/view"
        );
        assert!(frontend.query().is_none());
        assert_eq!(frontend.fragment(), Some(format!("jwt={token}").as_str()));
        assert!(service.claim(&format!("{token}x"), &id, true).is_err());
        assert!(service.claim(&token, "other-session", true).is_err());
        claims.expires_at_ms = now_ms() - 1;
        let expired = sign(&service, &claims);
        assert!(service.claim(&expired, &id, true).is_err());
        // Static frontend assets may finish loading after the connection deadline.
        assert!(service.claim(&expired, &id, false).is_ok());
        claims.generation = "other-generation".into();
        assert!(service.claim(&sign(&service, &claims), &id, false).is_err());
        claims.generation = service.generation.clone();
        claims.instance = InstanceId::generate();
        assert!(service.claim(&sign(&service, &claims), &id, false).is_err());
    }
    let view = service.live_view(&id, b"", origin).await.unwrap();
    assert_eq!(view["options"], json!({"mode":"devtools"}));
    for options in [
        json!({"mode":"other"}),
        json!({"expiresInMs":59_999}),
        json!({"expiresInMs":3_600_001}),
        json!({"guardrails":{"mode":"write"}}),
        json!({"targetId":"missing"}),
        json!({"secret":"forbidden"}),
    ] {
        assert!(
            service
                .live_view(&id, options.to_string().as_bytes(), origin)
                .await
                .is_err()
        );
    }
    let token = token_from(&view);
    service.close(&id, false).await.unwrap();
    assert!(service.claim(&token, &id, false).is_err());
    service.shutdown().await.unwrap();
    peer.abort();
}
