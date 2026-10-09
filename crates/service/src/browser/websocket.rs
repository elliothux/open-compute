//! Bounded native CDP WebSocket transport; private engine frames stay behind session authority.

use super::*;
use axum::extract::{
    FromRequestParts, Request,
    ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;

pub(super) async fn upgrade(
    service: Arc<BrowserService>,
    id: &str,
    request: Request,
) -> Result<Response, PlatformError> {
    upgrade_target(service, id, None, request).await
}

pub(super) async fn upgrade_target(
    service: Arc<BrowserService>,
    id: &str,
    target: Option<&str>,
    request: Request,
) -> Result<Response, PlatformError> {
    upgrade_connection(service, id, target, false, request).await
}

pub(super) async fn upgrade_live(
    service: Arc<BrowserService>,
    id: &str,
    target: &str,
    readonly: bool,
    request: Request,
) -> Result<Response, PlatformError> {
    let info = service
        .devtools(
            id,
            &format!("json/list/{target}"),
            &axum::http::Method::GET,
            &BTreeMap::new(),
        )
        .await?;
    if info["type"] != "page" {
        return Err(not_found());
    }
    upgrade_connection(service, id, Some(target), readonly, request).await
}
async fn upgrade_connection(
    service: Arc<BrowserService>,
    id: &str,
    target: Option<&str>,
    readonly: bool,
    request: Request,
) -> Result<Response, PlatformError> {
    let (mut parts, _) = request.into_parts();
    let websocket = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| invalid())?
        .max_message_size(service.config.max_message_bytes as usize)
        .max_frame_size(service.config.max_message_bytes as usize)
        .write_buffer_size(0)
        .max_write_buffer_size(
            service.config.max_message_bytes as usize * service.config.max_queued_messages as usize,
        );
    let (connection, events) = service.attach(id, target).await?;
    Ok(websocket.on_upgrade(move |socket| run(socket, connection, events, readonly)))
}

async fn run(
    socket: WebSocket,
    connection: BrowserConnection,
    mut events: BrowserCdpEvents,
    readonly: bool,
) {
    let service = connection.service.clone();
    let id = connection.id.clone();
    let queue = service.config.max_queued_messages as usize;
    let deadline = Duration::from_millis(service.config.command_timeout_ms);
    let (commands, mut requests) = mpsc::channel::<Value>(queue);
    let (responses, mut output) = mpsc::channel(queue);
    let worker_service = service.clone();
    let worker_id = id.clone();
    let worker_cdp = connection.cdp.clone();
    let worker_target = connection.target.clone();
    let worker = tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            let client_id = request.get("id").cloned();
            if readonly
                && !request
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(readonly_method)
            {
                if responses.send(json!({"id":client_id,"error":{"code":-32000,"message":"BROWSER_UNSUPPORTED"}})).await.is_err() { break; }
                continue;
            }

            let result = worker_service.command(&worker_id,&worker_cdp,request,worker_target.as_deref()).await.unwrap_or_else(|error|json!({"id":client_id,"error":{"code":-32000,"message":error.code().as_str()}}));
            if responses.send(result).await.is_err() {
                break;
            }
        }
    });
    let abort = AbortWorker(worker.abort_handle());
    let (mut writer, mut reader) = socket.split();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let code = loop {
        let outgoing = tokio::select! {
            biased;
            // A native event already published before a command response must not be overtaken.
            event = events.recv() => match event {
                Ok(event) => Message::Text(event.to_string().into()),
                Err(_) => break 1013,
            },
            message = reader.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let Ok(request) = serde_json::from_str(&text) else { break 1007; };
                    if commands.try_send(request).is_err() { break 1013; }
                    continue;
                }
                Some(Ok(Message::Binary(bytes))) => {
                    let Ok(request) = serde_json::from_slice(&bytes) else { break 1007; };
                    if commands.try_send(request).is_err() { break 1013; }
                    continue;
                }
                Some(Ok(Message::Ping(bytes))) => Message::Pong(bytes),
                Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => break 1000,
                Some(Err(_)) => break 1009,
            },
            response = output.recv() => match response {
                Some(response) => Message::Text(response.to_string().into()),
                None => break 1011,
            },
            _ = interval.tick() => {
                if service.session(&id).is_err() { break 1000; }
                continue;
            },
        };
        let length = match &outgoing {
            Message::Text(text) => text.len(),
            Message::Binary(bytes) => bytes.len(),
            _ => 0,
        };
        if length > service.config.max_message_bytes as usize {
            break 1009;
        }
        if !matches!(
            tokio::time::timeout(deadline, writer.send(outgoing)).await,
            Ok(Ok(()))
        ) {
            break 1011;
        }
    };
    drop(abort);
    let _ = tokio::time::timeout(
        deadline,
        writer.send(Message::Close(Some(CloseFrame {
            code,
            reason: "Browser connection closed".into(),
        }))),
    )
    .await;
}
struct AbortWorker(tokio::task::AbortHandle);
impl Drop for AbortWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn readonly_method(method: &str) -> bool {
    matches!(
        method,
        "Page.enable"
            | "Page.disable"
            | "Page.getLayoutMetrics"
            | "Page.startScreencast"
            | "Page.stopScreencast"
            | "Page.screencastFrameAck"
            | "Page.captureScreenshot"
            | "Runtime.enable"
            | "Runtime.disable"
            | "DOM.enable"
            | "DOM.disable"
            | "DOM.getDocument"
            | "DOM.getOuterHTML"
            | "DOM.describeNode"
            | "DOM.getBoxModel"
            | "CSS.enable"
            | "CSS.disable"
            | "CSS.getComputedStyleForNode"
            | "CSS.getMatchedStylesForNode"
            | "Log.enable"
            | "Log.disable"
    )
}
