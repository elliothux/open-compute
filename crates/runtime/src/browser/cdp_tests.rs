use super::*;

fn pair() -> (UnixStream, UnixStream) {
    UnixStream::pair().unwrap()
}

#[tokio::test]
async fn private_pipe_correlates_commands_events_and_fragmented_messages() {
    let (input, engine_input) = pair();
    let (mut engine_output, output) = pair();
    let cdp = BrowserCdp::pipe(input, output, 4096, 8, Duration::from_secs(1)).unwrap();
    let mut events = cdp.subscribe();
    let engine = tokio::spawn(async move {
        let mut input = BufReader::new(engine_input);
        let mut bytes = Vec::new();
        input.read_until(0, &mut bytes).await.unwrap();
        assert_eq!(bytes.pop(), Some(0));
        let command: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(command["method"], "Runtime.evaluate");
        assert_eq!(command["sessionId"], "owned-session");
        let response = json!({"id": command["id"], "result": {"value": "渲染"}}).to_string();
        for chunk in response.as_bytes().chunks(3) {
            engine_output.write_all(chunk).await.unwrap();
        }
        engine_output.write_all(b"\0").await.unwrap();
        engine_output
            .write_all(b"{\"method\":\"Target.targetCreated\",\"params\":{}}\0")
            .await
            .unwrap();
    });
    let response = cdp
        .command(
            "Runtime.evaluate",
            json!({"expression": "1"}),
            Some("owned-session"),
        )
        .await
        .unwrap();
    assert_eq!(response["result"]["value"], "渲染");
    assert_eq!(
        events.recv().await.unwrap()["method"],
        "Target.targetCreated"
    );
    engine.await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while cdp.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        cdp.command("Browser.getVersion", json!({}), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn private_pipe_rejects_oversize_malformed_and_unterminated_frames() {
    for message in [
        b"not JSON\0".as_slice(),
        b"{}\0",
        b"12345678901234567890\0",
        b"unterminated",
    ] {
        let (input, mut engine_input) = pair();
        let (mut engine_output, output) = pair();
        let cdp = BrowserCdp::pipe(input, output, 16, 1, Duration::from_millis(100)).unwrap();
        engine_output.write_all(message).await.unwrap();
        engine_output.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while cdp.is_alive() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            cdp.command("Browser.getVersion", json!({}), None)
                .await
                .is_err()
        );
        drop(cdp);
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), engine_input.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn private_pipe_timeout_does_not_permanently_consume_queue_capacity() {
    let (input, engine_input) = pair();
    let (mut engine_output, output) = pair();
    let cdp = BrowserCdp::pipe(input, output, 4096, 1, Duration::from_millis(40)).unwrap();
    let engine = tokio::spawn(async move {
        let mut input = BufReader::new(engine_input);
        let mut bytes = Vec::new();
        input.read_until(0, &mut bytes).await.unwrap();
        bytes.clear();
        input.read_until(0, &mut bytes).await.unwrap();
        bytes.pop();
        let command: Value = serde_json::from_slice(&bytes).unwrap();
        engine_output
            .write_all(format!("{}\0", json!({"id":command["id"], "result":{}})).as_bytes())
            .await
            .unwrap();
    });
    assert!(
        cdp.command("Browser.getVersion", json!({}), None)
            .await
            .is_err()
    );
    assert!(
        cdp.command("Target.getTargets", json!({}), None)
            .await
            .is_ok()
    );
    engine.await.unwrap();
    let (input, _) = pair();
    let (_, output) = pair();
    assert!(BrowserCdp::pipe(input, output, 0, 0, Duration::ZERO).is_err());
    assert!(
        BrowserCdp::connect("invalid", None, 0, 0, Duration::ZERO)
            .await
            .is_err()
    );
}
