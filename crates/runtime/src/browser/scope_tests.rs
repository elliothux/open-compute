use super::*;

#[test]
fn managed_download_namespace_matches_client_policy_and_fences_foreign_identity() {
    let mut scope = BrowserScope::new("own".into(), 1);
    let info = scope
        .target_info(&json!({"targetId":"native-page","browserContextId":"own","type":"page"}))
        .unwrap();
    for behavior in ["allowAndName", "deny"] {
        let (params, _) = scope.command("Browser.setDownloadBehavior", json!({"behavior":behavior,"downloadPath":"/tmp/client-artifacts","eventsEnabled":true}), None).unwrap();
        assert_eq!(params["browserContextId"], "own");
    }
    for params in [
        json!({"behavior":"allowAndName"}),
        json!({"behavior":"allowAndName","downloadPath":12}),
        json!({"behavior":"allowAndName","downloadPath":""}),
        json!({"behavior":"deny","eventsEnabled":"true"}),
        json!({"behavior":"deny","browserContextId":"foreign"}),
    ] {
        assert!(
            scope
                .command("Browser.setDownloadBehavior", params, None)
                .is_err()
        );
    }
    assert!(scope.event(&json!({"method":"Browser.downloadWillBegin","params":{"frameId":"foreign","guid":"foreign-guid"}})).is_none());
    let event = scope.event(&json!({"method":"Browser.downloadWillBegin","params":{"frameId":"native-page","guid":"native-guid","url":"https://example.com/file","suggestedFilename":"file"}})).unwrap();
    let guid = event["params"]["guid"].as_str().unwrap();
    assert_ne!(guid, "native-guid");
    assert_eq!(event["params"]["frameId"], info["targetId"]);
    let (params, _) = scope
        .command("Browser.cancelDownload", json!({"guid":guid}), None)
        .unwrap();
    assert_eq!(
        params,
        json!({"guid":"native-guid","browserContextId":"own"})
    );
    assert!(
        scope
            .command(
                "Browser.cancelDownload",
                json!({"guid":"native-guid"}),
                None
            )
            .is_err()
    );
    assert!(scope.event(&json!({"method":"Browser.downloadProgress","params":{"guid":"foreign-guid","state":"completed"}})).is_none());
    let progress = scope.event(&json!({"method":"Browser.downloadProgress","params":{"guid":"native-guid","state":"completed","filePath":"/private/host-file"}})).unwrap();
    assert_eq!(progress["params"]["guid"], guid);
    assert!(progress["params"].get("filePath").is_none());
    assert!(!scope.download_overflow());
    assert!(scope.event(&json!({"method":"Browser.downloadWillBegin","params":{"frameId":"native-page","guid":"overflow"}})).is_none());
    assert!(scope.download_overflow());
}

#[test]
fn managed_scope_fences_context_target_attachment_and_privileged_methods() {
    let mut scope = BrowserScope::new("private-default".into(), 16);
    let context = scope.add_context("private-extra").unwrap();
    let page = scope.target_info(&json!({"targetId":"private-target","browserContextId":"private-default","type":"page","url":"about:blank","openerId":"outside"})).unwrap();
    let target = page["targetId"].as_str().unwrap().to_owned();
    assert_ne!(target, "private-target");
    assert!(page.get("browserContextId").is_none());
    assert!(page.get("openerId").is_none());
    assert_eq!(scope.target(&target).unwrap(), "private-target");
    assert!(scope.target("private-target").is_err());
    assert!(
        scope
            .target_info(&json!({"targetId":"foreign","browserContextId":"foreign"}))
            .is_none()
    );
    let own = scope
        .target_info(&json!({"targetId":"extra-page","browserContextId":"private-extra"}))
        .unwrap();
    assert_eq!(own["browserContextId"], context);
    let (params, _) = scope
        .command("Target.createTarget", json!({"url":"about:blank"}), None)
        .unwrap();
    assert_eq!(params["browserContextId"], "private-default");
    let (params, _) = scope
        .command(
            "Storage.getCookies",
            json!({"browserContextId":context}),
            None,
        )
        .unwrap();
    assert_eq!(params["browserContextId"], "private-extra");
    let session = scope
        .add_attachment("private-attachment", "test-target")
        .unwrap();
    let (_, attachment) = scope
        .command(
            "Runtime.evaluate",
            json!({"expression":"1+1"}),
            Some(&session),
        )
        .unwrap();
    assert_eq!(attachment.as_deref(), Some("private-attachment"));
    for url in [
        "http://127.0.0.1:8123/",
        "http://10.0.0.1/",
        "http://169.254.169.254/",
        "http://[::1]/",
    ] {
        assert!(
            scope
                .command("Target.createTarget", json!({"url":url}), None)
                .is_ok()
        );
        assert!(
            scope
                .command("Page.navigate", json!({"url":url}), Some(&session))
                .is_ok()
        );
    }
    for url in [
        "file:///etc/passwd",
        "chrome://version",
        "devtools://devtools/",
        "javascript:alert(1)",
        "not a URL",
    ] {
        assert!(
            scope
                .command("Target.createTarget", json!({"url":url}), None)
                .is_err()
        );
        assert!(
            scope
                .command("Page.navigate", json!({"url":url}), Some(&session))
                .is_err()
        );
        assert!(
            scope
                .command(
                    "Fetch.continueRequest",
                    json!({"requestId":"owned-request","url":url}),
                    Some(&session)
                )
                .is_err()
        );
    }
    for (method, params, attachment) in [
        ("Runtime.evaluate", json!({"expression":"1"}), None),
        (
            "Runtime.evaluate",
            json!({"expression":"1"}),
            Some("private-attachment"),
        ),
        (
            "Target.createTarget",
            json!({"url":"about:blank","browserContextId":"private-extra"}),
            None,
        ),
        (
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":false}),
            None,
        ),
        ("Target.closeTarget", json!({"targetId":"foreign"}), None),
        (
            "Browser.setDownloadBehavior",
            json!({"behavior":"allow","downloadPath":"/private"}),
            None,
        ),
        (
            "Target.createBrowserContext",
            json!({"proxyServer":"localhost"}),
            None,
        ),
        ("Target.attachToBrowserTarget", json!({}), None),
        ("Browser.getBrowserCommandLine", json!({}), None),
        ("Browser.close", json!({}), None),
        (
            "DOM.setFileInputFiles",
            json!({"files":["/private"]}),
            Some(session.as_str()),
        ),
        (
            "Network.clearBrowserCookies",
            json!({}),
            Some(session.as_str()),
        ),
        (
            "Storage.getCookies",
            json!({"browserContextId":"foreign"}),
            None,
        ),
        ("Unknown.command", json!({}), Some(session.as_str())),
    ] {
        assert!(
            scope.command(method, params, attachment).is_err(),
            "{method}"
        );
    }
    scope.remove_context(&context).unwrap();
    assert!(
        scope
            .command(
                "Storage.getCookies",
                json!({"browserContextId":context}),
                None
            )
            .is_err()
    );
}

#[test]
fn managed_events_hide_foreign_targets_and_browser_global_data() {
    let mut scope = BrowserScope::new("own".into(), 16);
    let own = json!({"method":"Target.targetCreated","params":{"targetInfo":{"targetId":"raw","browserContextId":"own","type":"page"}}});
    let public = scope.event(&own).unwrap();
    let target = public["params"]["targetInfo"]["targetId"].clone();
    assert_ne!(target, "raw");
    assert!(scope.event(&json!({"method":"Target.targetCreated","params":{"targetInfo":{"targetId":"outside","browserContextId":"outside"}}})).is_none());
    assert!(
        scope
            .event(&json!({"method":"Browser.downloadWillBegin","params":{"url":"secret"}}))
            .is_none()
    );
    assert!(
        scope
            .event(&json!({"method":"Target.targetDestroyed","params":{"targetId":"outside"}}))
            .is_none()
    );
    assert_eq!(
        scope
            .event(&json!({"method":"Target.targetDestroyed","params":{"targetId":"raw"}}))
            .unwrap()["params"]["targetId"],
        target
    );
    let session = scope
        .add_attachment("private-session", target.as_str().unwrap())
        .unwrap();
    let event = json!({"sessionId":"private-session","method":"Runtime.consoleAPICalled","params":{"args":[{"value":{"targetId":"user-value"}}]}});
    let public = scope.event(&event).unwrap();
    assert_eq!(public["sessionId"], session);
    assert_eq!(
        public["params"]["args"][0]["value"]["targetId"],
        "user-value"
    );
    assert!(
        scope
            .event(&json!({"sessionId":"foreign","method":"Page.loadEventFired","params":{}}))
            .is_none()
    );
    assert!(
        scope
            .event(&json!({"sessionId":"private-session","method":"Browser.unknown","params":{}}))
            .is_none()
    );
}

#[test]
fn managed_replies_hide_engine_locators_and_fence_stream_handles() {
    let mut scope = BrowserScope::new("default".into(), 16);
    let context = scope
        .reply(
            "Target.createBrowserContext",
            &json!({}),
            json!({"id":7,"result":{"browserContextId":"engine-context"}}),
        )
        .unwrap()["result"]["browserContextId"]
        .as_str()
        .unwrap()
        .to_owned();
    let target = scope
        .reply(
            "Target.createTarget",
            &json!({}),
            json!({"result":{"targetId":"engine-target"}}),
        )
        .unwrap()["result"]["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(scope.target(&target).unwrap(), "engine-target");
    let attachment = scope
        .reply(
            "Target.attachToTarget",
            &json!({"targetId":target}),
            json!({"result":{"sessionId":"engine-session"}}),
        )
        .unwrap()["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let stream = scope
        .reply(
            "Page.printToPDF",
            &json!({}),
            json!({"result":{"stream":"engine-stream"}}),
        )
        .unwrap()["result"]["stream"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(stream, "engine-stream");
    let (params, _) = scope
        .command(
            "IO.read",
            json!({"handle":stream,"size":1024}),
            Some(&attachment),
        )
        .unwrap();
    assert_eq!(params["handle"], "engine-stream");
    assert!(
        scope
            .command(
                "IO.read",
                json!({"handle":"engine-stream"}),
                Some(&attachment)
            )
            .is_err()
    );
    scope
        .reply("IO.close", &json!({"handle":stream}), json!({"result":{}}))
        .unwrap();
    assert!(
        scope
            .command("IO.read", json!({"handle":stream}), Some(&attachment))
            .is_err()
    );
    let list=scope.reply("Target.getTargets",&json!({}),json!({"result":{"targetInfos":[{"targetId":"own","browserContextId":"default"},{"targetId":"foreign","browserContextId":"foreign"}]}})).unwrap();
    assert_eq!(list["result"]["targetInfos"].as_array().unwrap().len(), 1);
    assert!(
        scope
            .reply(
                "Target.getTargetInfo",
                &json!({}),
                json!({"result":{"targetInfo":{"targetId":"foreign","browserContextId":"foreign"}}})
            )
            .is_err()
    );
    scope
        .reply(
            "Target.disposeBrowserContext",
            &json!({"browserContextId":context}),
            json!({"result":{}}),
        )
        .unwrap();
    assert!(
        scope
            .command(
                "Storage.getCookies",
                json!({"browserContextId":context}),
                None
            )
            .is_err()
    );
    let safe = scope.reply("Runtime.evaluate",&json!({}),json!({"error":{"code":-32000,"message":"private endpoint/token/path","data":"secret"}})).unwrap();
    assert_eq!(
        safe,
        json!({"error":{"code":-32000,"message":"Browser command failed"}})
    );
    assert!(
        scope
            .reply("Browser.getVersion", &json!({}), json!({"unexpected":true}))
            .is_err()
    );
}

#[test]
fn managed_detach_preserves_target_identity_after_native_target_destruction() {
    let mut scope = BrowserScope::new("context".into(), 16);
    let attached = scope.event(&json!({"method":"Target.attachedToTarget","params":{"sessionId":"native-parent","targetInfo":{"targetId":"native-page","browserContextId":"context","type":"page"},"waitingForDebugger":true}})).unwrap();
    let parent = attached["params"]["sessionId"].as_str().unwrap();
    let page = attached["params"]["targetInfo"]["targetId"]
        .as_str()
        .unwrap();
    assert!(
        scope
            .command(
                "Emulation.setUserAgentOverride",
                json!({"userAgent":"owned"}),
                Some(parent)
            )
            .is_ok()
    );
    let child = scope.event(&json!({"sessionId":"native-parent","method":"Target.attachedToTarget","params":{"sessionId":"native-child","targetInfo":{"targetId":"native-frame","browserContextId":"context","type":"iframe"},"waitingForDebugger":true}})).unwrap();
    let child_session = child["params"]["sessionId"].as_str().unwrap();
    let child_target = child["params"]["targetInfo"]["targetId"].as_str().unwrap();
    scope
        .event(&json!({"method":"Target.targetDestroyed","params":{"targetId":"native-frame"}}))
        .unwrap();
    let detached = scope.event(&json!({"sessionId":"native-parent","method":"Target.detachedFromTarget","params":{"sessionId":"native-child","targetId":"native-frame"}})).unwrap();
    assert_eq!(detached["sessionId"], parent);
    assert_eq!(detached["params"]["sessionId"], child_session);
    assert_eq!(detached["params"]["targetId"], child_target);
    assert!(!detached.to_string().contains("native-"));
    scope
        .event(&json!({"method":"Target.targetDestroyed","params":{"targetId":"native-page"}}))
        .unwrap();
    let detached = scope.event(&json!({"method":"Target.detachedFromTarget","params":{"sessionId":"native-parent","targetId":"native-page"}})).unwrap();
    assert_eq!(detached["params"]["sessionId"], parent);
    assert_eq!(detached["params"]["targetId"], page);
    assert!(scope.event(&json!({"method":"Target.detachedFromTarget","params":{"sessionId":"unknown","targetId":"foreign"}})).is_none());
    assert!(
        scope
            .command("Runtime.evaluate", json!({"expression":"1"}), Some(parent))
            .is_err()
    );
}

#[test]
fn managed_frames_share_target_identity_without_granting_target_access_or_rewriting_user_values() {
    let mut scope = BrowserScope::new("own".into(), 16);
    let page = scope
        .reply(
            "Target.createTarget",
            &json!({}),
            json!({"result":{"targetId":"parent"}}),
        )
        .unwrap()["result"]["targetId"]
        .clone();
    let attachment = scope
        .add_attachment("attachment", page.as_str().unwrap())
        .unwrap();
    let tree = scope.reply("Page.getFrameTree", &json!({}), json!({"result":{"frameTree":{"frame":{"id":"parent"},"childFrames":[{"frame":{"id":"child","parentId":"parent"}}]}}})).unwrap();
    assert_eq!(tree["result"]["frameTree"]["frame"]["id"], page);
    let child = tree["result"]["frameTree"]["childFrames"][0]["frame"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        tree["result"]["frameTree"]["childFrames"][0]["frame"]["parentId"],
        page
    );
    assert!(
        scope.target(&child).is_err(),
        "frame metadata alone cannot authorize a target"
    );
    let target = scope
        .target_info(&json!({"targetId":"child","browserContextId":"own","type":"iframe"}))
        .unwrap();
    assert_eq!(target["targetId"], child);
    assert_eq!(scope.target(&child).unwrap(), "child");
    for method in [
        "Page.createIsolatedWorld",
        "Page.setDocumentContent",
        "DOM.getFrameOwner",
    ] {
        let (params, _) = scope
            .command(method, json!({"frameId":child}), Some(&attachment))
            .unwrap();
        assert_eq!(params["frameId"], "child");
        assert!(
            scope
                .command(method, json!({"frameId":"child"}), Some(&attachment))
                .is_err()
        );
        assert!(
            scope
                .command(method, json!({"frameId":"foreign"}), Some(&attachment))
                .is_err()
        );
    }
    for method in [
        "Page.frameAttached",
        "Page.frameDetached",
        "Network.requestWillBeSent",
        "Fetch.requestPaused",
    ] {
        let event = scope.event(&json!({"sessionId":"attachment","method":method,"params":{"frameId":"child","parentFrameId":"parent"}})).unwrap();
        assert_eq!(event["params"]["frameId"], child);
        assert_eq!(event["params"]["parentFrameId"], page);
    }
    let event = scope.event(&json!({"sessionId":"attachment","method":"Page.frameNavigated","params":{"frame":{"id":"child","parentId":"parent"}}})).unwrap();
    assert_eq!(event["params"]["frame"]["id"], child);
    let event = scope.event(&json!({"sessionId":"attachment","method":"Runtime.executionContextCreated","params":{"context":{"id":1,"auxData":{"frameId":"child"}}}})).unwrap();
    assert_eq!(event["params"]["context"]["auxData"]["frameId"], child);
    for (method, key) in [("DOM.getDocument", "root"), ("DOM.describeNode", "node")] {
        let reply = scope.reply(method, &json!({}), json!({"result":{key:{"frameId":"parent","children":[{"frameId":"child"}],"shadowRoots":[{"frameId":"child"}],"contentDocument":{"frameId":"child"}}}})).unwrap();
        assert_eq!(reply["result"][key]["frameId"], page);
        assert_eq!(reply["result"][key]["children"][0]["frameId"], child);
        assert_eq!(reply["result"][key]["shadowRoots"][0]["frameId"], child);
        assert_eq!(reply["result"][key]["contentDocument"]["frameId"], child);
    }
    let event = scope.event(&json!({"sessionId":"attachment","method":"DOM.setChildNodes","params":{"nodes":[{"frameId":"child"}]}})).unwrap();
    assert_eq!(event["params"]["nodes"][0]["frameId"], child);
    let event = scope.event(&json!({"sessionId":"attachment","method":"DOM.childNodeInserted","params":{"node":{"frameId":"child"}}})).unwrap();
    assert_eq!(event["params"]["node"]["frameId"], child);
    let value = json!({"frameId":"child","targetId":"parent","id":"child"});
    assert_eq!(
        scope
            .reply(
                "Runtime.evaluate",
                &json!({}),
                json!({"result":{"result":{"value":value}}})
            )
            .unwrap()["result"]["result"]["value"],
        value
    );
    assert_eq!(scope.event(&json!({"sessionId":"attachment","method":"Runtime.consoleAPICalled","params":{"args":[{"value":value}]}})).unwrap()["params"]["args"][0]["value"], value);
    for (method, result) in [
        ("Page.getFrameTree", json!({"frameTree":{"frame":null}})),
        ("DOM.getDocument", json!({"root":1})),
        ("DOM.describeNode", json!({"node":{"children":42}})),
    ] {
        assert!(
            scope
                .reply(method, &json!({}), json!({"result":result}))
                .is_err()
        );
    }
}
