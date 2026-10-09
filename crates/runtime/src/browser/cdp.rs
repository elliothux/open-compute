//! One bounded command owner for a browser connection, independent of session policy.

use futures::{SinkExt, StreamExt};
use open_compute_core::{ErrorCode, PlatformError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, protocol::WebSocketConfig,
};

type Reply = Result<Value, PlatformError>;
struct Command {
    method: String,
    params: Value,
    session: Option<String>,
    reply: oneshot::Sender<Reply>,
}

#[derive(Debug)]
struct TransportTasks(Vec<tokio::task::AbortHandle>);

impl Drop for TransportTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// Bounded browser event subscription; lag means the consumer must close its connection.
pub type BrowserCdpEvents = broadcast::Receiver<Value>;

/// Shared CDP command owner; public session policy belongs to the browser lifecycle manager.
#[derive(Clone, Debug)]
pub struct BrowserCdp {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Value>,
    alive: Arc<AtomicBool>,
    deadline: Duration,
    _tasks: Arc<TransportTasks>,
}

impl BrowserCdp {
    /// Attach to an already validated operator-owned native CDP WebSocket URL.
    pub async fn connect(
        url: &str,
        authorization: Option<&str>,
        max_message: usize,
        queue: usize,
        deadline: Duration,
    ) -> Result<Self, PlatformError> {
        if max_message == 0 || queue == 0 || deadline.is_zero() {
            return Err(unavailable());
        }
        let mut request = url.into_client_request().map_err(|_| unavailable())?;
        if let Some(value) = authorization {
            request
                .headers_mut()
                .insert("authorization", value.parse().map_err(|_| unavailable())?);
        }
        let config = WebSocketConfig::default()
            .max_message_size(Some(max_message))
            .max_frame_size(Some(max_message));
        let (socket, _) = tokio::time::timeout(
            deadline,
            tokio_tungstenite::connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
        let (mut writer, mut reader) = socket.split();
        let (outbound, mut output) = mpsc::channel::<Value>(queue);
        let (input, inbound) = mpsc::channel(queue);
        let writer_task = tokio::spawn(async move {
            while let Some(value) = output.recv().await {
                let text = value.to_string();
                if text.len() > max_message
                    || writer.send(Message::Text(text.into())).await.is_err()
                {
                    break;
                }
            }
            let _ = writer.close().await;
        });
        let reader_task = tokio::spawn(async move {
            while let Some(Ok(message)) = reader.next().await {
                let bytes = match message {
                    Message::Text(text) => text.as_bytes().to_vec(),
                    Message::Binary(bytes) => bytes.to_vec(),
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                };
                let Ok(value) = serde_json::from_slice(&bytes) else {
                    break;
                };
                if input.send(value).await.is_err() {
                    break;
                }
            }
        });
        Self::start(
            outbound,
            inbound,
            queue,
            deadline,
            vec![writer_task.abort_handle(), reader_task.abort_handle()],
        )
    }

    /// Own the parent ends of Chromium's private fd 3/4 NUL-delimited transport.
    pub fn pipe(
        input: UnixStream,
        output: UnixStream,
        max_message: usize,
        queue: usize,
        deadline: Duration,
    ) -> Result<Self, PlatformError> {
        if max_message == 0 || queue == 0 {
            return Err(unavailable());
        }
        if deadline.is_zero() {
            return Err(unavailable());
        }
        let (outbound, mut messages) = mpsc::channel::<Value>(queue);
        let (sender, inbound) = mpsc::channel(queue);
        let writer_task = tokio::spawn(async move {
            let mut input = input;
            while let Some(value) = messages.recv().await {
                let bytes = value.to_string();
                if bytes.len() > max_message
                    || input.write_all(bytes.as_bytes()).await.is_err()
                    || input.write_all(&[0]).await.is_err()
                {
                    break;
                }
            }
        });
        let reader_task = tokio::spawn(async move {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let result = (&mut output)
                    .take(max_message as u64 + 1)
                    .read_until(0, &mut bytes)
                    .await;
                if !matches!(result, Ok(1..)) || bytes.pop() != Some(0) || bytes.len() > max_message
                {
                    break;
                }
                let Ok(value) = serde_json::from_slice(&bytes) else {
                    break;
                };
                if sender.send(value).await.is_err() {
                    break;
                }
            }
        });
        Self::start(
            outbound,
            inbound,
            queue,
            deadline,
            vec![writer_task.abort_handle(), reader_task.abort_handle()],
        )
    }

    pub(super) fn start(
        outbound: mpsc::Sender<Value>,
        mut inbound: mpsc::Receiver<Value>,
        queue: usize,
        deadline: Duration,
        mut tasks: Vec<tokio::task::AbortHandle>,
    ) -> Result<Self, PlatformError> {
        if queue == 0 || deadline.is_zero() {
            return Err(unavailable());
        }
        let (commands, mut requests) = mpsc::channel::<Command>(queue);
        let (events, _) = broadcast::channel(queue);
        let alive = Arc::new(AtomicBool::new(true));
        let actor_events = events.clone();
        let actor_alive = alive.clone();
        let actor = tokio::spawn(async move {
            let mut next_id = 0u64;
            let mut pending = BTreeMap::<u64, oneshot::Sender<Reply>>::new();
            let mut expiry = tokio::time::interval(deadline);
            expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                pending.retain(|_, reply| !reply.is_closed());
                tokio::select! {
                    request = requests.recv(), if pending.len() < queue => {
                        let Some(command) = request else { break };
                        let Some(id) = next_id.checked_add(1) else { break };
                        next_id = id;
                        let mut value = json!({"id": id, "method": command.method, "params": command.params});
                        if let Some(session) = command.session { value["sessionId"] = session.into(); }
                        pending.insert(id, command.reply);
                        if !matches!(tokio::time::timeout(deadline, outbound.send(value)).await, Ok(Ok(()))) { break; }
                    }
                    response = inbound.recv() => {
                        let Some(value) = response else { break };
                        if let Some(id) = value.get("id").and_then(Value::as_u64) {
                            if let Some(reply) = pending.remove(&id) { let _ = reply.send(Ok(value)); }
                        } else if value.get("method").and_then(Value::as_str).is_some() {
                            let _ = actor_events.send(value);
                        } else { break; }
                    }
                    _ = expiry.tick() => {}
                }
            }
            actor_alive.store(false, Ordering::Release);
            for reply in pending.into_values() {
                let _ = reply.send(Err(unavailable()));
            }
        });
        tasks.push(actor.abort_handle());
        Ok(Self {
            commands,
            events,
            alive,
            deadline,
            _tasks: Arc::new(TransportTasks(tasks)),
        })
    }

    pub(super) fn invalidate(&self) {
        self.alive.store(false, Ordering::Release);
        for task in &self._tasks.0 {
            task.abort();
        }
    }

    /// Issue a command under the connection deadline; raw protocol errors remain structured.
    pub async fn command(&self, method: &str, params: Value, session: Option<&str>) -> Reply {
        let (reply, response) = oneshot::channel();
        let command = Command {
            method: method.to_owned(),
            params,
            session: session.map(str::to_owned),
            reply,
        };
        tokio::time::timeout(self.deadline, async {
            self.commands
                .send(command)
                .await
                .map_err(|_| unavailable())?;
            response.await.map_err(|_| unavailable())?
        })
        .await
        .map_err(|_| unavailable())?
    }

    /// Subscribe before issuing commands to avoid missing their resulting events.
    #[must_use]
    pub fn subscribe(&self) -> BrowserCdpEvents {
        self.events.subscribe()
    }

    /// Whether the transport owner has observed closure or malformed input.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeUnavailable,
        "browser CDP transport unavailable",
    )
}

#[cfg(test)]
#[path = "cdp_tests.rs"]
mod tests;
