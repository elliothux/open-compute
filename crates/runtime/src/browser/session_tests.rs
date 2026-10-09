use super::*;
use crate::browser::manager::tests::{config, root};
use crate::browser::{BrowserCdp, BrowserManager};
use std::time::Duration;

async fn command(
    client: &BrowserCdp,
    method: &str,
    params: Value,
    attachment: Option<&str>,
) -> Value {
    let mut reply = client.command(method, params, attachment).await.unwrap();
    reply.as_object_mut().unwrap().remove("id");
    reply
}

#[tokio::test]
async fn managed_window_bounds_belong_to_live_owned_pages() {
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("scoped-windows")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let owner = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let foreign = ManagedBrowserSession::open(generation).await.unwrap();
    let client = owner.connect(None).await.unwrap();
    let other = foreign.connect(None).await.unwrap();
    let first = command(
        &client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let target = first["result"]["targetId"].as_str().unwrap();
    let attached = command(
        &client,
        "Target.attachToTarget",
        json!({"targetId":target,"flatten":true}),
        None,
    )
    .await;
    let attachment = attached["result"]["sessionId"].as_str().unwrap();
    let window = command(
        &client,
        "Browser.getWindowForTarget",
        json!({}),
        Some(attachment),
    )
    .await;
    let id = window["result"]["windowId"].as_u64().unwrap();
    assert_eq!(id, 1, "window identity is client-local");
    let repeated = command(
        &client,
        "Browser.getWindowForTarget",
        json!({"targetId":target}),
        Some(attachment),
    )
    .await;
    assert_eq!(repeated["result"]["windowId"], id);
    assert!(
        command(
            &other,
            "Browser.getWindowBounds",
            json!({"windowId":id}),
            None
        )
        .await
        .get("error")
        .is_some()
    );
    let second = command(
        &other,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let second_target = second["result"]["targetId"].as_str().unwrap();
    let second_attached = command(
        &other,
        "Target.attachToTarget",
        json!({"targetId":second_target,"flatten":true}),
        None,
    )
    .await;
    let second_attachment = second_attached["result"]["sessionId"].as_str().unwrap();
    assert!(
        command(
            &client,
            "Browser.getWindowForTarget",
            json!({"targetId":second_target}),
            Some(attachment)
        )
        .await
        .get("error")
        .is_some()
    );
    assert!(
        command(
            &other,
            "Browser.getWindowForTarget",
            json!({"targetId":target}),
            Some(second_attachment)
        )
        .await
        .get("error")
        .is_some()
    );
    let second_window = command(
        &other,
        "Browser.getWindowForTarget",
        json!({}),
        Some(second_attachment),
    )
    .await;
    let second_id = second_window["result"]["windowId"].as_u64().unwrap();
    let before = command(
        &other,
        "Browser.getWindowBounds",
        json!({"windowId":second_id}),
        Some(second_attachment),
    )
    .await;
    let resized = command(
        &client,
        "Browser.setWindowBounds",
        json!({"windowId":id,"bounds":{"width":1280,"height":720}}),
        Some(attachment),
    )
    .await;
    assert!(resized.get("error").is_none(), "{resized}");
    assert_eq!(
        command(
            &client,
            "Browser.getWindowBounds",
            json!({"windowId":id}),
            Some(attachment)
        )
        .await["result"]["bounds"]["width"],
        1280
    );
    assert_eq!(
        command(
            &other,
            "Browser.getWindowBounds",
            json!({"windowId":second_id}),
            Some(second_attachment)
        )
        .await,
        before
    );
    for params in [
        json!({"windowId":999}),
        json!({"windowId":"1"}),
        json!({"windowId":id,"path":"/private"}),
    ] {
        assert!(
            command(&client, "Browser.getWindowBounds", params, Some(attachment))
                .await
                .get("error")
                .is_some()
        );
    }
    let mut closed = client.subscribe();
    command(
        &client,
        "Target.closeTarget",
        json!({"targetId":target}),
        None,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = closed.recv().await.unwrap();
            if event["method"] == "Target.detachedFromTarget"
                && event["params"]["sessionId"] == attachment
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        command(
            &client,
            "Browser.setWindowBounds",
            json!({"windowId":id,"bounds":{"width":1}}),
            Some(attachment)
        )
        .await
        .get("error")
        .is_some()
    );
    owner.close().await.unwrap();
    foreign.close().await.unwrap();
}

#[tokio::test]
async fn managed_creation_publishes_owned_auto_attachment_before_the_reply() {
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("ordered-creation")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let owner = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let foreign = ManagedBrowserSession::open(generation).await.unwrap();
    let client = owner.connect(None).await.unwrap();
    let unrelated = foreign.connect(None).await.unwrap();
    command(
        &client,
        "Target.setAutoAttach",
        json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
        None,
    )
    .await;
    let mut events = client.subscribe();
    let created = command(
        &client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let target = created["result"]["targetId"].as_str().unwrap();
    let attached = events
        .try_recv()
        .expect("automatic attachment precedes creation response");
    assert_eq!(attached["method"], "Target.attachedToTarget");
    assert_eq!(attached["params"]["targetInfo"]["targetId"], target);
    let attachment = attached["params"]["sessionId"].as_str().unwrap();
    assert!(
        command(
            &client,
            "Page.setFontFamilies",
            json!({"fontFamilies":{"standard":"Arial"}}),
            Some(attachment)
        )
        .await
        .get("error")
        .is_none()
    );
    assert!(
        command(
            &unrelated,
            "Page.setFontFamilies",
            json!({"fontFamilies":{"standard":"Arial"}}),
            Some(attachment)
        )
        .await
        .get("error")
        .is_some()
    );
    command(
        &client,
        "Runtime.runIfWaitingForDebugger",
        json!({}),
        Some(attachment),
    )
    .await;
    command(&client, "Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true,"filter":[{"type":"page","exclude":true},{}]}), None).await;
    assert!(
        command(
            &client,
            "Target.createTarget",
            json!({"url":"about:blank"}),
            None
        )
        .await
        .get("error")
        .is_none()
    );
    owner.close().await.unwrap();
    assert!(foreign.is_alive());
    foreign.close().await.unwrap();
}

#[tokio::test]
async fn managed_browser_sessions_share_a_process_without_sharing_contexts_targets_or_cookies() {
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("sessions")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let first = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let second = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let first_client = first.connect(None).await.unwrap();
    assert_eq!(
        command(&first_client, "Target.getTargets", json!({}), None).await["result"]["targetInfos"],
        json!([]),
        "session acquire does not prewarm a renderer"
    );
    command(
        &first_client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let mut attached_events = first_client.subscribe();
    let policy = json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true});
    assert!(
        command(&first_client, "Target.setAutoAttach", policy, None)
            .await
            .get("error")
            .is_none()
    );
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = attached_events.recv().await.unwrap();
            if event["method"] == "Target.attachedToTarget" {
                break event;
            }
        }
    })
    .await
    .unwrap();
    let automatic_attachment = event["params"]["sessionId"].as_str().unwrap();
    assert_eq!(
        command(
            &first_client,
            "Runtime.evaluate",
            json!({"expression":"3+4","returnByValue":true}),
            Some(automatic_attachment)
        )
        .await["result"]["result"]["value"],
        7
    );
    assert!(
        command(
            &first_client,
            "Target.setAutoAttach",
            json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}),
            None
        )
        .await
        .get("error")
        .is_none()
    );
    let second_client = second.connect(None).await.unwrap();
    assert_eq!(
        command(&first_client, "Target.getBrowserContexts", json!({}), None).await,
        json!({"result":{"browserContextIds":[]}})
    );
    let created = command(
        &first_client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let target = created["result"]["targetId"].as_str().unwrap();
    assert!(
        command(
            &second_client,
            "Target.getTargetInfo",
            json!({"targetId":target}),
            None
        )
        .await
        .get("error")
        .is_some()
    );
    let hidden = command(&second_client, "Target.getTargets", json!({}), None).await;
    let hidden = hidden["result"]["targetInfos"].as_array().unwrap();
    assert!(hidden.iter().all(|info| info["targetId"] != target));
    command(
        &first_client,
        "Storage.setCookies",
        json!({"cookies":[{"name":"owned","value":"one","domain":"example.test","path":"/"}]}),
        None,
    )
    .await;
    assert_eq!(
        command(&first_client, "Storage.getCookies", json!({}), None).await["result"]["cookies"][0]
            ["value"],
        "one"
    );
    assert_eq!(
        command(&second_client, "Storage.getCookies", json!({}), None).await["result"]["cookies"],
        json!([])
    );

    let attached = command(
        &first_client,
        "Target.attachToTarget",
        json!({"targetId":target,"flatten":true}),
        None,
    )
    .await;
    let attachment = attached["result"]["sessionId"].as_str().unwrap();
    let params = json!({"expression":"1+1","returnByValue":true});
    assert_eq!(
        command(
            &first_client,
            "Runtime.evaluate",
            params.clone(),
            Some(attachment)
        )
        .await["result"]["result"]["value"],
        2
    );
    assert!(
        command(&second_client, "Runtime.evaluate", params, Some(attachment))
            .await
            .get("error")
            .is_some()
    );
    assert!(
        command(
            &first_client,
            "Page.navigate",
            json!({"url":"file:///etc/passwd"}),
            Some(attachment)
        )
        .await
        .get("error")
        .is_some()
    );
    assert!(
        command(
            &first_client,
            "Browser.getBrowserCommandLine",
            json!({}),
            None
        )
        .await
        .get("error")
        .is_some()
    );

    let peer = first.connect(None).await.unwrap();
    let peer_reply = command(
        &peer,
        "Target.attachToTarget",
        json!({"targetId":target,"flatten":true}),
        None,
    )
    .await;
    let peer_attachment = peer_reply["result"]["sessionId"].as_str().unwrap();
    assert_ne!(attachment, peer_attachment);
    assert!(
        command(
            &peer,
            "Runtime.evaluate",
            json!({"expression":"1"}),
            Some(attachment)
        )
        .await
        .get("error")
        .is_some()
    );
    let owned = command(
        &first_client,
        "Target.createBrowserContext",
        json!({"disposeOnDetach":true}),
        None,
    )
    .await;
    let owned = owned["result"]["browserContextId"].as_str().unwrap();
    assert_eq!(
        command(&peer, "Target.getBrowserContexts", json!({}), None).await["result"]["browserContextIds"],
        json!([owned])
    );
    drop(first_client);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if command(&peer, "Target.getBrowserContexts", json!({}), None).await["result"]["browserContextIds"] == json!([]) { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert_eq!(
        command(
            &peer,
            "Runtime.evaluate",
            json!({"expression":"2+2","returnByValue":true}),
            Some(peer_attachment)
        )
        .await["result"]["result"]["value"],
        4
    );
    let first_client = peer;
    let extra = command(
        &first_client,
        "Target.createBrowserContext",
        json!({}),
        None,
    )
    .await;
    let context = extra["result"]["browserContextId"].as_str().unwrap();
    assert_eq!(
        command(&first_client, "Target.getBrowserContexts", json!({}), None).await["result"]["browserContextIds"],
        json!([context])
    );
    assert!(
        command(
            &second_client,
            "Target.disposeBrowserContext",
            json!({"browserContextId":context}),
            None
        )
        .await
        .get("error")
        .is_some()
    );
    command(
        &first_client,
        "Target.disposeBrowserContext",
        json!({"browserContextId":context}),
        None,
    )
    .await;
    assert_eq!(
        command(&first_client, "Target.getBrowserContexts", json!({}), None).await["result"]["browserContextIds"],
        json!([])
    );
    command(&first_client, "Browser.close", json!({}), None).await;
    assert!(!first.is_alive());
    assert!(second.is_alive());
    tokio::time::timeout(Duration::from_secs(5), async {
        while first_client.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        first_client
            .command("Target.getTargets", json!({}), None)
            .await
            .is_err()
    );
    command(
        &second_client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let before_drop = generation
        .cdp()
        .command("Target.getBrowserContexts", json!({}), None)
        .await
        .unwrap();
    assert_eq!(
        before_drop["result"]["browserContextIds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    drop(second_client);
    drop(second);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let contexts = generation
                .cdp()
                .command("Target.getBrowserContexts", json!({}), None)
                .await
                .unwrap();
            if contexts["result"]["browserContextIds"] == json!([]) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let failed = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let innocent = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let contexts = failed.scope.lock().await.engine_contexts().unwrap();
    generation
        .cdp()
        .command(
            "Target.disposeBrowserContext",
            json!({"browserContextId":contexts[0]}),
            None,
        )
        .await
        .unwrap();
    assert!(failed.close().await.is_err());
    assert!(
        !generation.cdp().is_alive(),
        "uncertain cleanup invalidates the physical generation"
    );
    assert!(!innocent.is_alive());
    manager.reconcile().await.unwrap();
    assert!(!root.join("sessions").join(generation.id()).exists());
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn managed_clients_keep_capacity_through_native_detach_cleanup() {
    let root = root().keep();
    let mut limits = config();
    limits.max_sessions = 1;
    limits.max_connections = 1;
    let manager = BrowserManager::new(limits, root.join("bounded-clients")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let session = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let authority = session.connect(None).await.unwrap();
    let client = session.connect(None).await.unwrap();
    assert_eq!(
        session.connect(None).await.unwrap_err().code(),
        ErrorCode::BrowserLimitExceeded
    );
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), async {
        while generation.clients.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let replacement = session.connect(None).await.unwrap();
    assert!(
        command(&replacement, "Browser.getVersion", json!({}), None)
            .await
            .get("error")
            .is_none()
    );
    drop(replacement);
    drop(authority);
    session.close().await.unwrap();
    assert!(session.connect(None).await.is_err());
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn managed_site_storage_and_service_workers_are_isolated_between_sessions() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut request = [0; 8192];
                let length = socket.read(&mut request).await.unwrap();
                let (body, media) = if request[..length].starts_with(b"GET /sw.js ") {
                    (
                        "self.addEventListener('install',()=>self.skipWaiting());self.addEventListener('activate',e=>e.waitUntil(self.clients.claim()));",
                        "text/javascript",
                    )
                } else {
                    (
                        "<!doctype html><title>State fixture</title><h1>State fixture</h1>",
                        "text/html",
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("site-storage")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let first = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let second = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let clients = [
        first.connect(None).await.unwrap(),
        second.connect(None).await.unwrap(),
    ];
    let mut attachments = Vec::new();
    for client in &clients {
        let page = command(
            client,
            "Target.createTarget",
            json!({"url":"about:blank"}),
            None,
        )
        .await;
        let attached = command(
            client,
            "Target.attachToTarget",
            json!({"targetId":page["result"]["targetId"],"flatten":true}),
            None,
        )
        .await;
        let attachment = attached["result"]["sessionId"].as_str().unwrap().to_owned();
        let navigated = command(
            client,
            "Page.navigate",
            json!({"url":origin}),
            Some(&attachment),
        )
        .await;
        assert!(
            navigated["result"].get("errorText").is_none(),
            "{navigated}"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let ready = command(client, "Runtime.evaluate", json!({"expression":"document.readyState === 'complete' && document.title === 'State fixture'","returnByValue":true}), Some(&attachment)).await;
                if ready.pointer("/result/result/value") == Some(&Value::Bool(true)) { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        attachments.push(attachment);
    }
    let written = command(&clients[0], "Runtime.evaluate", json!({"expression":r#"(async()=>{
      document.cookie='owned=one;Path=/;SameSite=Lax'; localStorage.setItem('owned','one');
      await new Promise((resolve,reject)=>{
        const request=indexedDB.open('owned',1);
        request.onupgradeneeded=()=>request.result.createObjectStore('state');
        request.onerror=()=>reject(request.error);
        request.onsuccess=()=>{const db=request.result;const tx=db.transaction('state','readwrite');
          tx.objectStore('state').put('one','owned');tx.oncomplete=()=>{db.close();resolve();};tx.onerror=()=>reject(tx.error);};
      });
      const cache=await caches.open('owned');await cache.put('/owned',new Response('one'));
      await navigator.serviceWorker.register('/sw.js');await navigator.serviceWorker.ready;return true;
    })()"#,"awaitPromise":true,"returnByValue":true}), Some(&attachments[0])).await;
    assert_eq!(
        written.pointer("/result/result/value"),
        Some(&Value::Bool(true)),
        "{written}"
    );
    let read = r#"(async()=>({cookie:document.cookie,local:localStorage.getItem('owned'),database:(await indexedDB.databases()).some(db=>db.name==='owned'),cache:await caches.has('owned'),registrations:(await navigator.serviceWorker.getRegistrations()).length}))()"#;
    for (index, client) in clients.iter().enumerate() {
        let value = command(
            client,
            "Runtime.evaluate",
            json!({"expression":read,"awaitPromise":true,"returnByValue":true}),
            Some(&attachments[index]),
        )
        .await;
        let expected = if index == 0 {
            json!({"cookie":"owned=one","local":"one","database":true,"cache":true,"registrations":1})
        } else {
            json!({"cookie":"","local":null,"database":false,"cache":false,"registrations":0})
        };
        assert_eq!(
            value.pointer("/result/result/value"),
            Some(&expected),
            "{value}"
        );
    }
    first.close().await.unwrap();
    assert!(second.is_alive());
    second.close().await.unwrap();
    manager.shutdown().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn managed_isolated_worlds_cannot_read_operator_files_or_set_file_inputs() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let root = root().keep();
    let canary = root.join("operator-canary.txt");
    std::fs::write(&canary, "operator-file-boundary-canary").unwrap();
    let url = url::Url::from_file_path(&canary).unwrap().to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let file_url = url.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let file_url = file_url.clone();
            tokio::spawn(async move {
                let mut request = [0_u8; 8192];
                let mut used = 0;
                loop {
                    let amount = socket.read(&mut request[used..]).await.unwrap();
                    assert!(amount > 0);
                    used += amount;
                    if request[..used].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                    assert!(used < request.len());
                }
                let response = if request[..used].starts_with(b"GET /redirect ") {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {file_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    let body = format!(
                        "<!doctype html><title>File boundary</title><iframe src='{file_url}'></iframe><script>const frame=document.createElement('iframe');frame.src={};document.body.append(frame);</script>",
                        serde_json::to_string(&file_url).unwrap()
                    );
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                socket.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    let manager = BrowserManager::new(config(), root.join("file-boundary")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let session = ManagedBrowserSession::open(generation).await.unwrap();
    let client = session.connect(None).await.unwrap();
    let target = command(
        &client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let attached = command(
        &client,
        "Target.attachToTarget",
        json!({"targetId":target["result"]["targetId"],"flatten":true}),
        None,
    )
    .await;
    let attachment = attached["result"]["sessionId"].as_str().unwrap();
    let navigation = command(
        &client,
        "Page.navigate",
        json!({"url":origin}),
        Some(attachment),
    )
    .await;
    assert!(
        navigation["result"].get("errorText").is_none(),
        "{navigation}"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ready = command(&client, "Runtime.evaluate", json!({"expression":"document.readyState === 'complete' && document.title === 'File boundary'","returnByValue":true}), Some(attachment)).await;
            if ready.pointer("/result/result/value") == Some(&json!(true)) {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let frames = command(&client, "Runtime.evaluate", json!({"expression":"Array.from(document.querySelectorAll('iframe'),frame=>{try{return frame.contentDocument?.body?.textContent ?? null;}catch{return null;}})","returnByValue":true}), Some(attachment)).await;
    let frames = frames
        .pointer("/result/result/value")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(frames.len(), 2);
    assert!(
        frames.iter().all(|value| value.is_null() || value == ""),
        "{frames:?}"
    );
    let frame = command(&client, "Page.getFrameTree", json!({}), Some(attachment)).await;
    let world = command(&client, "Page.createIsolatedWorld", json!({"frameId":frame["result"]["frameTree"]["frame"]["id"],"worldName":"fixed-client-world","grantUniveralAccess":true}), Some(attachment)).await;
    let context = world["result"]["executionContextId"].as_i64().unwrap();
    let expression = format!(
        "(async()=>{{const file={};return {{fetch:await fetch(file).then(r=>r.text()).catch(()=>null),redirect:await fetch('/redirect').then(r=>r.text()).catch(()=>null),xhr:await new Promise(resolve=>{{const x=new XMLHttpRequest();x.onload=()=>resolve(x.responseText);x.onerror=()=>resolve(null);try{{x.open('GET',file);x.send();}}catch{{resolve(null);}}}})}};}})()",
        serde_json::to_string(&url).unwrap()
    );
    for context in [None, Some(context)] {
        let mut params = json!({"expression":expression,"awaitPromise":true,"returnByValue":true});
        if let Some(context) = context {
            params["contextId"] = context.into();
        }
        let read = command(&client, "Runtime.evaluate", params, Some(attachment)).await;
        assert_eq!(
            read.pointer("/result/result/value"),
            Some(&json!({"fetch":null,"xhr":null,"redirect":null})),
            "{read}"
        );
    }
    for (method, params) in [
        ("Page.navigate", json!({"url":url})),
        (
            "Fetch.continueRequest",
            json!({"requestId":"request","url":url}),
        ),
        (
            "DOM.setFileInputFiles",
            json!({"files":[canary],"nodeId":1}),
        ),
        ("Browser.getBrowserCommandLine", json!({})),
    ] {
        assert!(
            command(&client, method, params, Some(attachment))
                .await
                .get("error")
                .is_some(),
            "{method}"
        );
    }
    let redirected = command(
        &client,
        "Page.navigate",
        json!({"url":format!("{origin}redirect")}),
        Some(attachment),
    )
    .await;
    assert!(
        redirected
            .pointer("/result/errorText")
            .is_some_and(Value::is_string),
        "{redirected}"
    );
    session.close().await.unwrap();
    manager.shutdown().await.unwrap();
    server.abort();
    assert_eq!(
        std::fs::read_to_string(canary).unwrap(),
        "operator-file-boundary-canary"
    );
}

#[tokio::test]
async fn managed_debugger_sources_and_isolates_remain_session_scoped() {
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("debugger")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let first = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let second = ManagedBrowserSession::open(generation).await.unwrap();
    let clients = [
        first.connect(None).await.unwrap(),
        second.connect(None).await.unwrap(),
    ];
    let mut attachments = Vec::new();
    let mut isolates = Vec::new();
    for client in &clients {
        let page = command(
            client,
            "Target.createTarget",
            json!({"url":"about:blank"}),
            None,
        )
        .await;
        let attached = command(
            client,
            "Target.attachToTarget",
            json!({"targetId":page["result"]["targetId"],"flatten":true}),
            None,
        )
        .await;
        let attachment = attached["result"]["sessionId"].as_str().unwrap().to_owned();
        let isolate = command(client, "Runtime.getIsolateId", json!({}), Some(&attachment)).await;
        isolates.push(isolate["result"]["id"].as_str().unwrap().to_owned());
        attachments.push(attachment);
    }
    assert_ne!(
        isolates[0], isolates[1],
        "sessions have distinct renderer isolates"
    );
    let mut events = clients[0].subscribe();
    for (index, client) in clients.iter().enumerate() {
        for (method, params) in [
            ("Debugger.enable", json!({})),
            ("Debugger.setPauseOnExceptions", json!({"state":"none"})),
            ("Debugger.setAsyncCallStackDepth", json!({"maxDepth":0})),
            ("Debugger.setBlackboxPatterns", json!({"patterns":[]})),
        ] {
            let reply = command(client, method, params, Some(&attachments[index])).await;
            assert!(reply.get("error").is_none(), "{method}: {reply}");
        }
    }
    let source = "globalThis.debuggerCanary='owned-first-session';\n//# sourceURL=owned-script.js";
    command(
        &clients[0],
        "Runtime.evaluate",
        json!({"expression":source}),
        Some(&attachments[0]),
    )
    .await;
    let script = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["method"] == "Debugger.scriptParsed"
                && event["params"]["url"] == "owned-script.js"
            {
                assert_eq!(event["sessionId"], attachments[0]);
                break event["params"]["scriptId"].as_str().unwrap().to_owned();
            }
        }
    })
    .await
    .unwrap();
    let owned = command(
        &clients[0],
        "Debugger.getScriptSource",
        json!({"scriptId":script}),
        Some(&attachments[0]),
    )
    .await;
    assert_eq!(owned["result"]["scriptSource"], source);
    let foreign = command(
        &clients[1],
        "Debugger.getScriptSource",
        json!({"scriptId":script}),
        Some(&attachments[1]),
    )
    .await;
    assert_ne!(
        foreign["result"]["scriptSource"], source,
        "foreign script identifiers cannot read another renderer's source"
    );
    for (index, client) in clients.iter().enumerate() {
        command(
            client,
            "Debugger.disable",
            json!({}),
            Some(&attachments[index]),
        )
        .await;
    }
    first.close().await.unwrap();
    assert!(second.is_alive());
    second.close().await.unwrap();
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn managed_cross_site_iframe_attachments_and_debugger_waits_are_session_scoped() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut request = [0; 8192];
                let length = socket.read(&mut request).await.unwrap();
                let body = if request[..length].starts_with(b"GET /frame ") {
                    "<!doctype html><h1>Owned cross-site iframe</h1>".to_owned()
                } else {
                    format!(
                        "<!doctype html><h1>Parent</h1><iframe src=\"http://localhost:{port}/frame\"></iframe>"
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let root = root().keep();
    let manager = BrowserManager::new(config(), root.join("cross-site-iframe")).unwrap();
    let generation = manager.acquire().await.unwrap();
    let first = ManagedBrowserSession::open(generation.clone())
        .await
        .unwrap();
    let second = ManagedBrowserSession::open(generation).await.unwrap();
    let client = first.connect(None).await.unwrap();
    let foreign = second.connect(None).await.unwrap();
    let page = command(
        &client,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    let page = page["result"]["targetId"].as_str().unwrap();
    let attached = command(
        &client,
        "Target.attachToTarget",
        json!({"targetId":page,"flatten":true}),
        None,
    )
    .await;
    let parent = attached["result"]["sessionId"].as_str().unwrap();
    let mut events = client.subscribe();
    let policy = command(
        &client,
        "Target.setAutoAttach",
        json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
        Some(parent),
    )
    .await;
    assert!(policy.get("error").is_none(), "{policy}");
    let navigated = command(
        &client,
        "Page.navigate",
        json!({"url":format!("http://127.0.0.1:{port}/")}),
        Some(parent),
    )
    .await;
    assert!(navigated.get("error").is_none(), "{navigated}");
    assert_eq!(navigated["result"]["frameId"], page);
    let (child, child_target) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["method"] == "Target.attachedToTarget"
                && event["params"]["targetInfo"]["type"] == "iframe"
            {
                assert_eq!(event["sessionId"], parent);
                assert_eq!(event["params"]["waitingForDebugger"], true);
                assert!(
                    event["params"]["targetInfo"]
                        .get("browserContextId")
                        .is_none()
                );
                break (
                    event["params"]["sessionId"].as_str().unwrap().to_owned(),
                    event["params"]["targetInfo"]["targetId"].clone(),
                );
            }
        }
    })
    .await
    .unwrap();
    assert!(
        command(
            &foreign,
            "Runtime.evaluate",
            json!({"expression":"document.body.innerHTML"}),
            Some(&child)
        )
        .await
        .get("error")
        .is_some()
    );
    let resumed = command(
        &client,
        "Runtime.runIfWaitingForDebugger",
        json!({}),
        Some(&child),
    )
    .await;
    assert!(resumed.get("error").is_none(), "{resumed}");
    let tree = command(&client, "Page.getFrameTree", json!({}), Some(&child)).await;
    assert_eq!(tree["result"]["frameTree"]["frame"]["id"], child_target);
    let parent_isolate = command(&client, "Runtime.getIsolateId", json!({}), Some(parent)).await;
    let child_isolate = command(&client, "Runtime.getIsolateId", json!({}), Some(&child)).await;
    assert_ne!(
        parent_isolate["result"]["id"], child_isolate["result"]["id"],
        "fixture must use a distinct iframe renderer"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = command(&client, "Runtime.evaluate", json!({"expression":"document.querySelector('h1')?.textContent","returnByValue":true}), Some(&child)).await;
            if value.pointer("/result/result/value") == Some(&json!("Owned cross-site iframe")) { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();
    first.close().await.unwrap();
    assert!(second.is_alive());
    let remaining = command(
        &foreign,
        "Target.createTarget",
        json!({"url":"about:blank"}),
        None,
    )
    .await;
    assert!(remaining["result"]["targetId"].is_string());
    second.close().await.unwrap();
    manager.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}
