//! Public native `DevTools` HTTP/page operations use real renderer state and scoped authority.

use super::*;

pub(super) async fn exercise(
    client: &platform_process::Client,
    address: &str,
    account: &str,
    id: &str,
    observer: &BrowserCdp,
) {
    let base = format!("/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}");
    let get = |path: String| async move {
        request(
            client,
            address,
            address,
            &path,
            "GET",
            Some("workflow-deployer"),
            Value::Null,
        )
        .await
    };
    // Version exposes the platform proxy, never the external browser's endpoint.
    let (status, _, bytes) = get(format!("{base}/json/version")).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let version: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(version["Browser"].as_str().unwrap().contains("Chrome"));
    assert_eq!(version["Protocol-Version"], "1.3");
    assert!(
        version["WebKit-Version"]
            .as_str()
            .unwrap()
            .starts_with("537.36 (@")
    );
    assert!(
        version["webSocketDebuggerUrl"]
            .as_str()
            .unwrap()
            .contains("/client/v4/accounts/")
    );
    assert_eq!(
        url::Url::parse(version["webSocketDebuggerUrl"].as_str().unwrap())
            .unwrap()
            .port(),
        Some(address.parse::<std::net::SocketAddr>().unwrap().port())
    );
    let (status, _, bytes) = get(format!("{base}/json/protocol")).await;
    assert_eq!(status, 200);
    let schema: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(schema["domains"].as_array().unwrap().iter().any(|domain| {
        domain["domain"] == "Page"
            && domain["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command["name"] == "navigate")
    }));
    let (status, _, bytes) = request(
        client,
        address,
        address,
        &format!("{base}/json/new?url=about%3Ablank"),
        "PUT",
        Some("workflow-deployer"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let created: Value = serde_json::from_slice(&bytes).unwrap();
    let target = created["id"].as_str().unwrap();
    assert_eq!(created["type"], "page");
    assert!(
        url::Url::parse(created["webSocketDebuggerUrl"].as_str().unwrap())
            .unwrap()
            .path()
            .ends_with(&format!("/page/{target}"))
    );
    for suffix in ["json", "json/list"] {
        let (status, media, bytes) = get(format!("{base}/{suffix}")).await;
        assert_eq!(status, 200);
        assert!(media.starts_with("application/json"));
        let rows: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == target)
        );
    }
    let (status, _, bytes) = get(format!("{base}/json/list/{target}")).await;
    assert_eq!(status, 200);
    let default: Value = serde_json::from_slice(&bytes).unwrap();
    live::default_view(&default, observer).await;
    let (status, _, bytes) = request(
        client,
        address,
        address,
        &format!("{base}/json/list/{target}"),
        "GET",
        Some("workflow-read-only"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    live::readonly_view(&serde_json::from_slice::<Value>(&bytes).unwrap(), address).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["id"],
        target
    );
    let mut page = connections::connect(address, account, id, &format!("/page/{target}")).await;
    connections::command(&mut page, 1, "Runtime.enable", json!({})).await;
    let value = connections::command(
        &mut page,
        -2,
        "Runtime.evaluate",
        json!({"expression":"document.title = 'Native DevTools'; 7*6","returnByValue":true}),
    )
    .await;
    assert_eq!(value.pointer("/result/value"), Some(&json!(42)));
    page.close(None).await.unwrap();
    drop(page);
    let (status, _, bytes) = get(format!("{base}/json/list/{target}")).await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["title"],
        "Native DevTools"
    );
    live::exercise(client, address, account, id, target, observer).await;
    for (suffix, expected) in [
        (format!("json/activate/{target}"), "Target activated"),
        (format!("json/close/{target}"), "Target is closing"),
    ] {
        let (status, _, bytes) = get(format!("{base}/{suffix}")).await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["message"],
            expected
        );
    }
    for (suffix, status) in [
        (format!("json/list/{target}"), 404),
        ("json/list?unknown=true".into(), 501),
        ("json/list?url=x&url=y".into(), 400),
    ] {
        assert_eq!(get(format!("{base}/{suffix}")).await.0, status);
    }
    let (status, _, _) = request(
        client,
        address,
        address,
        &format!("{base}/json/new"),
        "PUT",
        Some("workflow-read-only"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 403);
}
