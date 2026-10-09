use super::daemon_support::fixture::{Fixture, RequestTarget};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest as _};

const SCRIPT: &str = "loader-capacity";
const NEIGHBOR: &str = "loader-capacity-neighbor";
const OBJECTS: usize = 129;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_objects_preserve_loader_capacity_and_persist_through_restart() {
    let mut fixture = Fixture::new(None).await;
    let version = fixture
        .upload_javascript(
            SCRIPT,
            include_str!("loader-capacity.js"),
            &[json!({"name":"OBJECTS", "type":"durable_object_namespace", "class_name":"Counter", "script_name":SCRIPT})],
            Some(json!({"default":{"type":"worker"},"Counter":{"type":"durable-object","storage":"sqlite"}})),
            None,
        )
        .await;
    fixture.promote(SCRIPT, &version).await;
    let mut states = Vec::new();
    let mut sockets = Vec::new();
    for index in 0..OBJECTS {
        assert_eq!(
            fixture
                .invoke(SCRIPT, &format!("/mixed?name=object-{index}"))
                .await,
            json!(["rpc-first:start", "fetch-second:start", "rpc-third:start"]),
            "cold mixed dispatch for object {index}"
        );
        let state = fixture
            .invoke(SCRIPT, &format!("/increment?name=object-{index}"))
            .await;
        assert_eq!(state["count"], 1, "object {index}");
        states.push(state);
        let mut request = format!("ws://{}/ws?name=object-{index}", fixture.public)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "host",
            format!("{SCRIPT}.{}.localhost", fixture.internal_account)
                .parse()
                .unwrap(),
        );
        let (socket, response) = tokio::time::timeout(
            Duration::from_secs(30),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), 101, "object {index}");
        sockets.push(socket);
    }
    assert_neighbors(&fixture).await;
    fixture.invoke(SCRIPT, "/alarm?name=object-0").await;
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let state = fixture.invoke(SCRIPT, "/state?name=object-0").await;
        if state["alarmed"] == true {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native persisted alarm did not run"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for (index, socket) in sockets.iter_mut().enumerate() {
        socket
            .send(Message::Text("still-alive".into()))
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let state: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(state["message"], "still-alive");
        assert_eq!(state["count"], 1);
        assert_eq!(state["id"], states[index]["id"]);
        socket.close(None).await.unwrap();
    }
    drop(sockets);
    fixture.restart().await;
    for (index, before) in states.iter().enumerate() {
        let after = fixture
            .invoke(SCRIPT, &format!("/state?name=object-{index}"))
            .await;
        assert_eq!(
            after["count"], 1,
            "committed SQLite data changed for {index}"
        );
        assert_eq!(after["id"], before["id"]);
        assert_ne!(after["boot"], before["boot"]);
    }
    assert_neighbors(&fixture).await;
    assert_eq!(
        fixture.invoke(SCRIPT, "/state?name=object-0").await["alarmed"],
        true
    );
    fixture.process.stop().await;
}

async fn assert_neighbors(fixture: &Fixture) {
    let source = "export default {fetch(){return Response.json({neighbor:true})}}";
    let version = fixture
        .upload_javascript(NEIGHBOR, source, &[], None, None)
        .await;
    fixture.promote(NEIGHBOR, &version).await;
    assert_eq!(
        fixture.invoke(NEIGHBOR, "/").await,
        json!({"neighbor":true})
    );
    let (live, _, _) = fixture
        .request(
            "/health/live",
            "GET",
            "",
            Vec::new(),
            RequestTarget::Unauthenticated,
        )
        .await;
    assert_eq!(live, 200);
}
