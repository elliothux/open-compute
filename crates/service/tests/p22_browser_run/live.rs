//! Signed viewer links exercise native screencast, UI resources and readonly command authority.
use super::*;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

pub(super) async fn default_view(target: &Value, observer: &BrowserCdp) {
    let frontend = url::Url::parse(target["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
    render(
        &frontend,
        observer,
        false,
        "devtools",
        target["id"].as_str().unwrap(),
    )
    .await;
}

pub(super) async fn readonly_view(target: &Value, address: &str) {
    let url = url::Url::parse(target["webSocketDebuggerUrl"].as_str().unwrap()).unwrap();
    let mut request = format!("ws://{address}{}?{}", url.path(), url.query().unwrap())
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "host",
        format!("{}:{}", url.host_str().unwrap(), url.port().unwrap())
            .parse()
            .unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket.send(Message::Text(json!({"id":1,"method":"Runtime.evaluate","params":{"expression":"document.title='Forbidden'"}}).to_string().into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                continue;
            };
            let reply: Value = serde_json::from_str(&text).unwrap();
            if reply["id"] == 1 {
                assert_eq!(reply["error"]["message"], "BROWSER_UNSUPPORTED");
                break;
            }
        }
    })
    .await
    .unwrap();
    socket.close(None).await.unwrap();
}

pub(super) async fn exercise(
    client: &platform_process::Client,
    address: &str,
    account: &str,
    id: &str,
    target: &str,
    observer: &BrowserCdp,
) {
    let route =
        format!("/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}/live_view");
    for (mode, readonly) in [("tab", true), ("full", false), ("devtools", false)] {
        let mut options = json!({"mode":mode,"targetId":target,"expiresInMs":60_000});
        if readonly {
            options["guardrails"] = json!({"mode":"readonly"});
        }
        let (status, _, bytes) = request(
            client,
            address,
            address,
            &route,
            "POST",
            Some("workflow-deployer"),
            options,
        )
        .await;
        assert_eq!(status, 200);
        let view: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(view["id"], target);
        let frontend = url::Url::parse(view["devtoolsFrontendUrl"].as_str().unwrap()).unwrap();
        let websocket = url::Url::parse(view["webSocketDebuggerUrl"].as_str().unwrap()).unwrap();
        let host = format!(
            "{}:{}",
            frontend.host_str().unwrap(),
            frontend.port().unwrap()
        );
        assert_eq!(
            frontend.host_str(),
            Some(address.split(':').next().unwrap())
        );
        assert!(frontend.query().is_none());
        let (status, media, bytes) = request(
            client,
            address,
            &host,
            frontend.path(),
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert!(media.starts_with("text/html"));
        assert!(String::from_utf8(bytes).unwrap().contains("Remote browser"));
        let script = frontend.join("view.js").unwrap();
        let (status, media, bytes) = request(
            client,
            address,
            &host,
            script.path(),
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert!(media.starts_with("text/javascript"));
        assert!(!bytes.is_empty());
        let path = format!("{}?{}", websocket.path(), websocket.query().unwrap());
        let mut connection = format!("ws://{address}{path}")
            .into_client_request()
            .unwrap();
        connection
            .headers_mut()
            .insert("host", host.parse().unwrap());
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(3),
            tokio_tungstenite::connect_async(connection),
        )
        .await
        .unwrap()
        .unwrap();
        if readonly {
            socket.send(Message::Text(json!({"id":1,"method":"Runtime.evaluate","params":{"expression":"document.title='Forbidden'"}}).to_string().into())).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                        continue;
                    };
                    let result: Value = serde_json::from_str(&text).unwrap();
                    if result["id"] == 1 {
                        assert_eq!(result["error"]["message"], "BROWSER_UNSUPPORTED");
                        break;
                    }
                }
            })
            .await
            .unwrap();
            connections::command(&mut socket, 2, "Page.enable", json!({})).await;
            socket.send(Message::Text(json!({"id":3,"method":"Page.startScreencast","params":{"format":"jpeg","maxWidth":640,"maxHeight":480}}).to_string().into())).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                        continue;
                    };
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event["method"] == "Page.screencastFrame" {
                        assert!(!event["params"]["data"].as_str().unwrap().is_empty());
                        connections::command(
                            &mut socket,
                            4,
                            "Page.screencastFrameAck",
                            json!({"sessionId":event["params"]["sessionId"]}),
                        )
                        .await;
                        break;
                    }
                }
            })
            .await
            .unwrap();
            connections::command(&mut socket, 5, "Page.stopScreencast", json!({})).await;
        } else {
            let value = connections::command(
                &mut socket,
                1,
                "Runtime.evaluate",
                json!({"expression":"7*6","returnByValue":true}),
            )
            .await;
            assert_eq!(value.pointer("/result/value"), Some(&json!(42)));
        }
        socket.close(None).await.unwrap();
        drop(socket);
        let prefix = format!("/client/v4/accounts/{account}/browser-rendering/live/{id}");
        let jwt = websocket
            .query_pairs()
            .find(|(name, _)| name == "jwt")
            .unwrap()
            .1
            .into_owned();
        let (status, _, _) = request(
            client,
            address,
            &host,
            &format!("{prefix}/targets?jwt={jwt}"),
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, if mode == "full" { 200 } else { 404 });
        if mode == "devtools" {
            let mut inspector = frontend
                .join(&format!("{id}/devtools/inspector.html"))
                .unwrap();
            inspector
                .query_pairs_mut()
                .append_pair("jwt", &jwt)
                .append_pair(
                    "ws",
                    &format!(
                        "{}{}?{}",
                        host,
                        websocket.path(),
                        websocket.query().unwrap()
                    ),
                );
            let response = client
                .request(
                    Request::builder()
                        .uri(format!(
                            "http://{address}{}?{}",
                            inspector.path(),
                            inspector.query().unwrap()
                        ))
                        .header("host", &host)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let cookie = response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            assert!(
                response.headers()["set-cookie"]
                    .to_str()
                    .unwrap()
                    .contains("HttpOnly; SameSite=Strict")
            );
            let html = to_bytes(Body::new(response.into_body()), 16384)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&html).contains("inspector.js"));
            let path = format!("{prefix}/devtools/entrypoints/inspector/inspector.js");
            assert_eq!(
                request(client, address, &host, &path, "GET", None, Value::Null)
                    .await
                    .0,
                404
            );
            let response = client
                .request(
                    Request::builder()
                        .uri(format!("http://{address}{path}"))
                        .header("host", &host)
                        .header("cookie", cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert!(
                response.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("text/javascript")
            );
            let bytes = to_bytes(Body::new(response.into_body()), 1024 * 1024)
                .await
                .unwrap();
            assert!(!bytes.is_empty());
        }
        render(&frontend, observer, readonly, mode, target).await;
        let (status, _, _) = request(
            client,
            address,
            &host,
            &format!("{prefix}/targets?jwt={jwt}x"),
            "GET",
            None,
            Value::Null,
        )
        .await;
        assert_eq!(status, 404);
    }
    for (role, options, expected) in [
        ("workflow-read-only", json!({}), 403),
        ("workflow-deployer", json!({"mode":"invalid"}), 400),
        ("workflow-deployer", json!({"expiresInMs":1}), 400),
        ("workflow-deployer", json!({"targetId":"not-present"}), 404),
    ] {
        assert_eq!(
            request(
                client,
                address,
                address,
                &route,
                "POST",
                Some(role),
                options
            )
            .await
            .0,
            expected
        );
    }
}

async fn render(
    frontend: &url::Url,
    observer: &BrowserCdp,
    readonly: bool,
    mode: &str,
    target: &str,
) {
    let context = observer
        .command(
            "Target.createBrowserContext",
            json!({"disposeOnDetach":true}),
            None,
        )
        .await
        .unwrap();
    let context = context
        .pointer("/result/browserContextId")
        .unwrap()
        .as_str()
        .unwrap();
    let page = observer
        .command(
            "Target.createTarget",
            json!({"url":frontend.as_str(),"browserContextId":context}),
            None,
        )
        .await
        .unwrap();
    let page = page.pointer("/result/targetId").unwrap().as_str().unwrap();
    let attachment = observer
        .command(
            "Target.attachToTarget",
            json!({"targetId":page,"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let attachment = attachment
        .pointer("/result/sessionId")
        .unwrap()
        .as_str()
        .unwrap();
    let expression = if mode == "devtools" {
        "(()=>{const doc=document.querySelector('iframe')?.contentDocument; const text=(root)=>{if(!root)return '';let result=root.textContent??'';for(const node of root.querySelectorAll('*'))if(node.shadowRoot)result+=text(node.shadowRoot);return result;};const content=text(doc);return {ready:!!doc?.querySelector('.root-view')&&content.includes('Elements')&&content.includes('Console'),content};})()"
    } else {
        "({ready:!!document.querySelector('main img')?.naturalWidth,disabled:document.querySelector('nav input')?.disabled,status:document.querySelector('main p')?.textContent})"
    };
    let mut last = Value::Null;
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let value = observer
                .command(
                    "Runtime.evaluate",
                    json!({"expression":expression,"returnByValue":true}),
                    Some(attachment),
                )
                .await
                .unwrap();
            if value.pointer("/result/result/value/ready") == Some(&json!(true)) {
                break value;
            }
            last = value;
            tokio::task::yield_now().await;
        }
    })
    .await;
    let value = result.unwrap_or_else(|_| panic!("viewer {mode} failed to render: {last}"));
    if mode != "devtools" {
        assert_eq!(
            value.pointer("/result/result/value/disabled"),
            Some(&json!(readonly))
        );
        assert_eq!(
            value.pointer("/result/result/value/status"),
            Some(&json!(if readonly { "Read only" } else { "Connected" }))
        );
        if mode == "full" {
            input(observer, attachment, target).await;
        }
    } else {
        assert!(
            !value
                .pointer("/result/result/value/content")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("WebSocket disconnected")
        );
    }
    if mode == "devtools" && !readonly {
        panels(observer, attachment, target).await;
    }
    observer
        .command(
            "Target.disposeBrowserContext",
            json!({"browserContextId":context}),
            None,
        )
        .await
        .unwrap();
}

async fn panels(observer: &BrowserCdp, viewer: &str, target: &str) {
    let tree = observer
        .command("Page.getFrameTree", json!({}), Some(viewer))
        .await
        .unwrap();
    let frame = tree["result"]["frameTree"]["childFrames"][0]["frame"]["id"]
        .as_str()
        .unwrap();
    let mut events = observer.subscribe();
    observer
        .command("Runtime.enable", json!({}), Some(viewer))
        .await
        .unwrap();
    let context = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["sessionId"] == viewer
                && event["method"] == "Runtime.executionContextCreated"
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
    let attached = observer
        .command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let target_attachment = attached["result"]["sessionId"].as_str().unwrap();
    observer
        .command("Runtime.enable", json!({}), Some(target_attachment))
        .await
        .unwrap();
    observer
        .command(
            "Runtime.evaluate",
            json!({"expression":"console.log('Browser panel probe')"}),
            Some(target_attachment),
        )
        .await
        .unwrap();
    for (panel, marker) in [
        ("Console", "Browser panel probe"),
        ("Network", "Network panel"),
        ("Elements", "Elements panel"),
    ] {
        let select = format!(
            r#"(async()=>{{const {{InspectorView}}=await import('./ui/legacy/legacy.js');await InspectorView.InspectorView.instance().showPanel({:?});return true;}})()"#,
            panel.to_lowercase()
        );
        let selected = observer
            .command(
                "Runtime.evaluate",
                json!({"expression":select,"contextId":context,"awaitPromise":true,"returnByValue":true}),
                Some(viewer),
            )
            .await
            .unwrap();
        assert_eq!(
            selected.pointer("/result/result/value"),
            Some(&json!(true)),
            "{selected}"
        );
        let expression = format!(
            r#"(()=>{{const doc=document.querySelector('iframe')?.contentDocument;const nodes=[];const visit=root=>{{if(!root)return;for(const node of root.querySelectorAll('*')){{nodes.push(node);if(node.shadowRoot)visit(node.shadowRoot);}}}};visit(doc);const tab=nodes.find(node=>node.getAttribute('role')==='tab'&&node.getAttribute('aria-label')==={panel:?});return {{ready:tab?.getAttribute('aria-selected')==='true'&&nodes.some(node=>node.textContent?.includes({marker:?})||node.getAttribute('aria-label')?.includes({marker:?})),tabs:nodes.filter(node=>node.getAttribute('role')==='tab').map(node=>[node.getAttribute('aria-label'),node.getAttribute('aria-selected')]),labels:nodes.map(node=>node.getAttribute('aria-label')).filter(Boolean).slice(-50)}};}})()"#
        );
        let mut last = Value::Null;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let value = observer
                    .command(
                        "Runtime.evaluate",
                        json!({"expression":expression,"returnByValue":true}),
                        Some(viewer),
                    )
                    .await
                    .unwrap();
                if value.pointer("/result/result/value/ready") == Some(&json!(true)) {
                    break;
                }
                last = value;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("DevTools {panel} did not display {marker}: {last}"));
    }
    observer
        .command(
            "Target.detachFromTarget",
            json!({"sessionId":target_attachment}),
            None,
        )
        .await
        .unwrap();
}

async fn input(observer: &BrowserCdp, viewer: &str, target: &str) {
    let reply = observer
        .command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .await
        .unwrap();
    let target = reply
        .pointer("/result/sessionId")
        .unwrap()
        .as_str()
        .unwrap();
    let evaluate = |attachment: &str, expression: &str| {
        let attachment = attachment.to_owned();
        let params = json!({"expression":expression,"returnByValue":true});
        async move {
            observer
                .command("Runtime.evaluate", params, Some(&attachment))
                .await
                .unwrap()
        }
    };
    evaluate(target, "document.body.innerHTML='<input aria-label=\"Remote input\" style=\"position:fixed;inset:0;width:100%;height:100%;box-sizing:border-box\">';document.body.dataset.clicks='0';document.body.dataset.wheels='0';document.addEventListener('click',()=>document.body.dataset.clicks=String(+document.body.dataset.clicks+1));document.addEventListener('wheel',()=>document.body.dataset.wheels=String(+document.body.dataset.wheels+1));").await;
    let sent = evaluate(viewer, "(()=>{const img=document.querySelector('main img'),r=img.getBoundingClientRect();const selected=document.querySelector('nav select').value;img.dispatchEvent(new MouseEvent('click',{clientX:r.left+r.width/2,clientY:r.top+r.height/2}));return selected;})()").await;
    assert!(
        !sent
            .pointer("/result/result/value")
            .unwrap()
            .as_str()
            .unwrap()
            .is_empty()
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let reply = evaluate(target, "({focused:document.activeElement.tagName==='INPUT',clicks:+document.body.dataset.clicks})").await;
            if reply.pointer("/result/result/value/focused") == Some(&json!(true))
                && reply.pointer("/result/result/value/clicks").and_then(Value::as_u64).is_some_and(|n|n>0) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    evaluate(viewer, "(()=>{const img=document.querySelector('main img'),r=img.getBoundingClientRect();for(const type of ['keydown','keyup'])img.dispatchEvent(new KeyboardEvent(type,{key:'z',code:'KeyZ',keyCode:90}));img.dispatchEvent(new WheelEvent('wheel',{clientX:r.left+r.width/2,clientY:r.top+r.height/2,deltaY:120}));})()").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let reply = evaluate(target, "({text:document.querySelector('input').value,clicks:+document.body.dataset.clicks,wheels:+document.body.dataset.wheels})").await;
            if reply.pointer("/result/result/value/text") == Some(&json!("z")) && reply.pointer("/result/result/value/wheels").and_then(Value::as_u64).is_some_and(|n|n>0) {
                assert!(reply.pointer("/result/result/value/clicks").unwrap().as_u64().unwrap()>0);
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    evaluate(viewer, "document.querySelector('nav input').value='about:blank#viewer-navigation';document.querySelector('nav button').click();").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if evaluate(target, "location.href")
                .await
                .pointer("/result/result/value")
                == Some(&json!("about:blank#viewer-navigation"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    observer
        .command("Target.detachFromTarget", json!({"sessionId":target}), None)
        .await
        .unwrap();
}
