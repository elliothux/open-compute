//! Independent native client connections own their dispose-on-detach contexts.

use super::*;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::frame::{
            Frame,
            coding::{Data, OpCode},
        },
    },
};

pub(super) type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub(super) async fn connect(address: &str, account: &str, id: &str, suffix: &str) -> Socket {
    let mut request = format!(
        "ws://{address}/client/v4/accounts/{account}/browser-rendering/devtools/browser/{id}{suffix}"
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer workflow-deployer".parse().unwrap());
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .unwrap()
    .unwrap()
    .0
}

pub(super) async fn command(socket: &mut Socket, id: i64, method: &str, params: Value) -> Value {
    socket
        .send(Message::Text(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    response(socket, id).await
}

async fn response(socket: &mut Socket, id: i64) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = message {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["id"] == id {
                    assert!(value.get("error").is_none(), "{value}");
                    return value["result"].clone();
                }
            }
        }
    })
    .await
    .unwrap()
}

pub(super) async fn wire(address: &str, account: &str, id: &str) {
    let mut socket = connect(address, account, id, "").await;
    let request = json!({"id":1,"method":"Browser.getVersion","params":{}}).to_string();
    socket
        .send(Message::Binary(request.into_bytes().into()))
        .await
        .unwrap();
    assert!(response(&mut socket, 1).await["product"].is_string());

    let request = json!({"id":2,"method":"Browser.getVersion","params":{}})
        .to_string()
        .into_bytes();
    let middle = request.len() / 2;
    socket
        .send(Message::Frame(Frame::message(
            request[..middle].to_vec(),
            OpCode::Data(Data::Text),
            false,
        )))
        .await
        .unwrap();
    let ping = b"transport probe".to_vec();
    socket
        .send(Message::Ping(ping.clone().into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Pong(payload) => {
                    assert_eq!(payload.as_ref(), ping.as_slice());
                    break;
                }
                Message::Text(_) => {}
                message => panic!("unexpected fragmented-message control response: {message:?}"),
            }
        }
    })
    .await
    .unwrap();
    socket
        .send(Message::Frame(Frame::message(
            request[middle..].to_vec(),
            OpCode::Data(Data::Continue),
            true,
        )))
        .await
        .unwrap();
    assert!(response(&mut socket, 2).await["product"].is_string());
    socket.close(None).await.unwrap();
    drop(socket);

    let mut malformed = connect(address, account, id, "").await;
    malformed.send(Message::Text("{".into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match malformed.next().await.unwrap().unwrap() {
                Message::Close(Some(frame)) => {
                    assert_eq!(u16::from(frame.code), 1007);
                    break;
                }
                Message::Text(_) => {}
                message => panic!("unexpected malformed-message response: {message:?}"),
            }
        }
    })
    .await
    .unwrap();
    drop(malformed);
    let mut recovered = connect(address, account, id, "").await;
    assert!(
        command(&mut recovered, 3, "Browser.getVersion", json!({})).await["product"].is_string()
    );
    recovered.close(None).await.unwrap();
}

pub(super) async fn churn(socket: &mut Socket) {
    let before = command(socket, 100, "Target.getBrowserContexts", json!({})).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        for cycle in 0..32 {
            let created = command(
                socket,
                101 + cycle * 3,
                "Target.createBrowserContext",
                json!({"disposeOnDetach":true}),
            )
            .await;
            let context = &created["browserContextId"];
            assert!(context.is_string());
            let target = command(
                socket,
                102 + cycle * 3,
                "Target.createTarget",
                json!({"url":"about:blank","browserContextId":context}),
            )
            .await;
            assert!(target["targetId"].is_string());
            command(
                socket,
                103 + cycle * 3,
                "Target.disposeBrowserContext",
                json!({"browserContextId":context}),
            )
            .await;
        }
    })
    .await
    .expect("context churn exceeded its bounded deadline");
    let after = command(socket, 200, "Target.getBrowserContexts", json!({})).await;
    assert_eq!(
        before, after,
        "context churn leaked or disposed an anchor context"
    );
    assert!(command(socket, 201, "Browser.getVersion", json!({})).await["product"].is_string());
}

async fn contexts(observer: &BrowserCdp, expected: &[&str]) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = observer
                .command("Target.getBrowserContexts", json!({}), None)
                .await
                .unwrap();
            let actual = result
                .pointer("/result/browserContextIds")
                .unwrap()
                .as_array()
                .unwrap();
            if actual.len() == expected.len()
                && expected.iter().all(|id| actual.contains(&json!(id)))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("native connection ownership or detach disposal is incorrect");
}

pub(super) async fn exercise(address: &str, account: &str, id: &str, observer: &BrowserCdp) {
    wire(address, account, id).await;
    let mut first = connect(address, account, id, "").await;
    let mut second = connect(address, account, id, "").await;
    let one = command(
        &mut first,
        1,
        "Target.createBrowserContext",
        json!({"disposeOnDetach":true}),
    )
    .await;
    let one = one["browserContextId"].as_str().unwrap();
    let two = command(
        &mut second,
        1,
        "Target.createBrowserContext",
        json!({"disposeOnDetach":true}),
    )
    .await;
    let two = two["browserContextId"].as_str().unwrap();
    assert_ne!(one, two);
    contexts(observer, &[one, two]).await;
    first.close(None).await.unwrap();
    drop(first);
    contexts(observer, &[two]).await;
    churn(&mut second).await;
    assert!(command(&mut second, -7, "Browser.getVersion", json!({})).await["product"].is_string());
    second.close(None).await.unwrap();
    drop(second);
    contexts(observer, &[]).await;
}
