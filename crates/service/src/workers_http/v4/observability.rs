//! Cloudflare Script Tails and Workers Observability Telemetry adapters.

use super::{domain, handlers};
use crate::cloudflare_v4::{V4Error, V4Permission, error_response};
use crate::http::HttpState;
use crate::observability::{ObservabilityService, TailFilter};
use crate::observability_filter::{Combination, FilterNode, validate as validate_filter_ast};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_core::VersionId;
use serde::Deserialize;
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::Duration;

const MAX_TAIL_BODY: usize = 16 * 1024;
#[path = "observability_query.rs"]
mod query;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account}/workers/scripts/{script}/tails",
            get(list_tails).post(create_tail),
        )
        .route(
            "/accounts/{account}/workers/scripts/{script}/tails/{tail}",
            axum::routing::delete(delete_tail),
        )
        .route(
            "/accounts/{account}/workers/observability/telemetry/keys",
            post(query::telemetry_keys),
        )
        .route(
            "/accounts/{account}/workers/observability/telemetry/values",
            post(query::telemetry_values),
        )
        .route(
            "/accounts/{account}/workers/observability/telemetry/query",
            post(query::telemetry_query),
        )
        .route(
            "/accounts/{account}/workers/observability/telemetry/live-tail",
            post(prepare_live_tail),
        )
        .route(
            "/accounts/{account}/workers/observability/telemetry/live-tail/heartbeat",
            post(live_tail_heartbeat),
        )
}

pub(crate) fn signed_router() -> Router<HttpState> {
    Router::new()
        .route("/open-compute/tails/{tail}/{ticket}", get(connect_tail))
        .route(
            "/open-compute/live-tails/{tail}/{ticket}",
            get(connect_live_tail),
        )
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TailCreateBody {
    Wrangler(Vec<TailFilterWire>),
    Sdk(TailCreateSdkBody),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TailCreateSdkBody {
    #[serde(default)]
    filters: Vec<TailFilterWire>,
}

impl TailCreateBody {
    fn filters(self) -> Vec<TailFilterWire> {
        match self {
            Self::Wrangler(filters) => filters,
            Self::Sdk(body) => body.filters,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TailFilterWire {
    Sampling(SamplingFilter),
    Outcome(OutcomeFilter),
    Method(MethodFilter),
    Header(HeaderFilter),
    ClientIp(ClientIpFilter),
    Query(QueryFilter),
    ScriptVersion(ScriptVersionFilter),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SamplingFilter {
    sampling_rate: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeFilter {
    outcome: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MethodFilter {
    method: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderFilter {
    header: HeaderQuery,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderQuery {
    key: String,
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientIpFilter {
    client_ip: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryFilter {
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptVersionFilter {
    #[serde(rename = "scriptVersion")]
    script_version: String,
}

async fn list_tails(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let result = (|| {
        let account_id = domain::resolve_account(&state, &account)?;
        let api = handlers::worker_api(&state)?;
        let worker = domain::worker_by_name(api, account_id, &script)
            .map_err(|error| V4Error::from(&error))?;
        api.observability()
            .map_err(|error| V4Error::from(&error))?
            .list_tails(account_id, worker.id)
            .map_err(|error| V4Error::from(&error))
    })();
    handlers::respond(context, result)
}

async fn create_tail(
    State(state): State<HttpState>,
    Path((account, script)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0.ip());
    let Some(body) = to_bytes(request.into_body(), MAX_TAIL_BODY)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TailCreateBody>(&bytes).ok())
    else {
        return error_response(V4Error::InvalidRequest, context.request_id());
    };
    let result = (|| {
        let account_id = domain::resolve_account(&state, &account)?;
        let api = handlers::worker_api(&state)?;
        let worker = domain::worker_by_name(api, account_id, &script)
            .map_err(|error| V4Error::from(&error))?;
        let filters = body
            .filters()
            .into_iter()
            .map(|filter| tail_filter(filter, peer))
            .collect::<Result<Vec<_>, _>>()?;
        api.observability()
            .map_err(|error| V4Error::from(&error))?
            .create_tail(account_id, &worker, filters, context.request_id())
            .map_err(|error| V4Error::from(&error))
    })();
    handlers::respond(context, result)
}

async fn delete_tail(
    State(state): State<HttpState>,
    Path((account, script, tail)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let result = (|| {
        let account_id = domain::resolve_account(&state, &account)?;
        let api = handlers::worker_api(&state)?;
        let worker = domain::worker_by_name(api, account_id, &script)
            .map_err(|error| V4Error::from(&error))?;
        api.observability()
            .map_err(|error| V4Error::from(&error))?
            .delete_tail(account_id, worker.id, &tail, context.request_id())
            .map_err(|error| V4Error::from(&error))
    })();
    handlers::respond(context, result)
}

fn tail_filter(value: TailFilterWire, peer: Option<IpAddr>) -> Result<TailFilter, V4Error> {
    match value {
        TailFilterWire::Sampling(value) => Ok(TailFilter::Sampling(value.sampling_rate)),
        TailFilterWire::Outcome(value) => Ok(TailFilter::Outcome(value.outcome)),
        TailFilterWire::Method(mut value) => {
            for method in &mut value.method {
                *method = method.to_ascii_uppercase();
            }
            Ok(TailFilter::Method(value.method))
        }
        TailFilterWire::Header(value) => Ok(TailFilter::Header {
            key: value.header.key.to_ascii_lowercase(),
            query: value.header.query,
        }),
        TailFilterWire::ClientIp(value) => {
            let mut addresses = Vec::with_capacity(value.client_ip.len());
            for value in value.client_ip {
                if value == "self" {
                    addresses.push(peer.ok_or(V4Error::InvalidField("/filters/client_ip"))?);
                } else {
                    addresses.push(
                        value
                            .parse()
                            .map_err(|_| V4Error::InvalidField("/filters/client_ip"))?,
                    );
                }
            }
            Ok(TailFilter::ClientIp(addresses))
        }
        TailFilterWire::Query(value) => Ok(TailFilter::Query(value.query)),
        TailFilterWire::ScriptVersion(value) => Ok(TailFilter::ScriptVersion(
            VersionId::from_str(&value.script_version)
                .map_err(|_| V4Error::InvalidField("/filters/scriptVersion"))?,
        )),
    }
}

async fn connect_tail(
    State(state): State<HttpState>,
    Path((tail, ticket)): Path<(String, String)>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let requested_protocol = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|item| item.trim() == "trace-v1"));
    if !requested_protocol {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(api) = state.worker_api() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(service) = api.observability() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(connection) = service.connect_tail(&tail, &ticket) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let service = service.clone();
    ws.protocols(["trace-v1"])
        .accept_unmasked_frames(true)
        .max_message_size(1_024)
        .max_frame_size(1_024)
        .on_upgrade(move |socket| tail_socket(socket, service, tail, connection))
}

async fn tail_socket(
    mut socket: WebSocket,
    service: Arc<ObservabilityService>,
    tail: String,
    mut connection: crate::observability::TailConnection,
) {
    let accepted = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str::<TailControl>(text.as_str())
                        .is_ok_and(|control| !control.debug);
                }
                Some(Ok(Message::Ping(value))) => {
                    if socket.send(Message::Pong(value)).await.is_err() {
                        return false;
                    }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Binary(_) | Message::Close(_))) | Some(Err(_)) | None => {
                    return false;
                }
            }
        }
    })
    .await
    .unwrap_or(false);
    if !accepted {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: close_code::POLICY,
                reason: "debug and control frames are unsupported".into(),
            })))
            .await;
        service.disconnect_tail(&tail);
        return;
    }
    let mut expiry = tokio::time::interval(Duration::from_secs(1));
    expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = expiry.tick() => {
                if service.tail_expired(&tail) {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::NORMAL,
                        reason: "tail session expired".into(),
                    }))).await;
                    break;
                }
            }
            frame = connection.receiver.recv() => {
                let Some(frame) = frame else {
                    if service.tail_overloaded(&tail) {
                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: close_code::AGAIN,
                            reason: "tail client is too slow".into(),
                        }))).await;
                    }
                    break;
                };
                if socket.send(Message::Text(frame.text.clone().into())).await.is_err() { break; }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Ping(value))) => {
                    if socket.send(Message::Pong(value)).await.is_err() { break; }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: "unexpected client frame".into(),
                    }))).await;
                    break;
                }
            }
        }
    }
    service.disconnect_tail(&tail);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TailControl {
    #[serde(default)]
    debug: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LiveTailBody {
    script_id: Option<String>,
    #[serde(default)]
    filter_combination: Combination,
    #[serde(default)]
    filters: Vec<FilterNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LiveTailHeartbeatBody {
    script_id: Option<String>,
}

async fn prepare_live_tail(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body: LiveTailBody = match super::json::json_body(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = (|| {
        validate_filter_ast(&body.filters).map_err(|error| V4Error::from(&error))?;
        let script = body.script_id.ok_or(V4Error::Unsupported)?;
        let account_id = domain::resolve_account(&state, &account)?;
        let api = handlers::worker_api(&state)?;
        let worker = domain::worker_by_name(api, account_id, &script)
            .map_err(|error| V4Error::from(&error))?;
        api.observability()
            .map_err(|error| V4Error::from(&error))?
            .create_live_tail(
                account_id,
                &worker,
                body.filter_combination,
                body.filters,
                context.request_id(),
            )
            .map_err(|error| V4Error::from(&error))
    })();
    handlers::respond(context, result)
}

async fn live_tail_heartbeat(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let context = match handlers::authorize(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body: LiveTailHeartbeatBody = match super::json::json_body(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = (|| {
        let script = body.script_id.ok_or(V4Error::Unsupported)?;
        let account_id = domain::resolve_account(&state, &account)?;
        let api = handlers::worker_api(&state)?;
        let worker = domain::worker_by_name(api, account_id, &script)
            .map_err(|error| V4Error::from(&error))?;
        api.observability()
            .map_err(|error| V4Error::from(&error))?
            .heartbeat_live_tail(account_id, worker.id)
            .map_err(|error| V4Error::from(&error))?;
        Ok(json!({}))
    })();
    handlers::respond(context, result)
}

async fn connect_live_tail(
    State(state): State<HttpState>,
    Path((tail, ticket)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(api) = state.worker_api() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(service) = api.observability() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(connection) = service.connect_live_tail(&tail, &ticket) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let service = service.clone();
    ws.accept_unmasked_frames(true)
        .max_message_size(1_024)
        .max_frame_size(1_024)
        .on_upgrade(move |socket| live_tail_socket(socket, service, tail, connection))
}

async fn live_tail_socket(
    mut socket: WebSocket,
    service: Arc<ObservabilityService>,
    tail: String,
    mut connection: crate::observability::TailConnection,
) {
    let mut expiry = tokio::time::interval(Duration::from_secs(1));
    expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = expiry.tick() => {
                if service.tail_expired(&tail) {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::NORMAL,
                        reason: "live tail session expired".into(),
                    }))).await;
                    break;
                }
            }
            frame = connection.receiver.recv() => {
                let Some(frame) = frame else {
                    if service.tail_overloaded(&tail) {
                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: close_code::AGAIN,
                            reason: "tail client is too slow".into(),
                        }))).await;
                    }
                    break;
                };
                if socket.send(Message::Text(frame.text.clone().into())).await.is_err() { break; }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Ping(value))) => {
                    if socket.send(Message::Pong(value)).await.is_err() { break; }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: "unexpected client frame".into(),
                    }))).await;
                    break;
                }
            }
        }
    }
    service.close_live_tail(&tail);
}

#[cfg(test)]
#[path = "observability_tests.rs"]
mod tests;
