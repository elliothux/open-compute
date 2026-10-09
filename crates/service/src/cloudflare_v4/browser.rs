//! Browser Run management authority; native `DevTools` responses retain their raw wire shape.

use super::storage::{account, context, strict_query};
use super::{V4Error, V4Permission, error_response};
use crate::browser::BrowserService;
use crate::http::HttpState;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use std::sync::Arc;

pub(super) fn router() -> Router<HttpState> {
    let mut router = Router::new()
        .route("/devtools/session", get(list))
        .route("/devtools/browser", axum::routing::post(acquire))
        .route(
            "/devtools/browser/{session_id}",
            get(connect).delete(connect),
        )
        .route(
            "/devtools/browser/{session_id}/{*operation}",
            get(devtools).put(devtools),
        );
    router = router.route(
        "/devtools/browser/{session_id}/live_view",
        axum::routing::post(live_view),
    );
    for action in [
        "content",
        "screenshot",
        "pdf",
        "scrape",
        "links",
        "snapshot",
        "markdown",
        "json",
        "accessibilityTree",
    ] {
        router = router.route(&format!("/{action}"), axum::routing::post(quick_action));
    }
    Router::new()
        .nest("/accounts/{account_id}/browser-rendering", router.clone())
        .nest("/accounts/{account_id}/browser-run", router)
}

async fn devtools(
    State(state): State<HttpState>,
    Path((account_id, session_id, operation)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let write = request.method() == Method::PUT
        || operation.starts_with("json/close/")
        || operation.starts_with("json/activate/")
        || operation.starts_with("page/");
    let permission = if write {
        V4Permission::ProductWrite
    } else {
        V4Permission::Read
    };
    let service = match authority(&state, &account_id, &request, permission) {
        Ok(service) => service,
        Err(response) => return response,
    };
    if request.method() == Method::GET
        && let Some(target) = operation.strip_prefix("page/")
    {
        return service
            .page(&session_id, target, request)
            .await
            .unwrap_or_else(|error| crate::browser::http::error(&error));
    }
    let Ok(query) = strict_query(&request) else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    let readonly = context(&request, V4Permission::ProductWrite).is_err();
    let method = request.method().clone();
    if !matches!(to_bytes(request.into_body(), service.config().max_body_bytes as usize).await, Ok(body) if body.is_empty())
    {
        return crate::browser::http::error(&crate::browser::invalid());
    }
    match service
        .devtools(&session_id, &operation, &method, &query)
        .await
    {
        Ok(mut value) => {
            if let Ok((base, origin)) =
                service.public_urls(&session_id, state.control_origin_port())
            {
                if operation == "json/version" {
                    value["webSocketDebuggerUrl"] = base.into();
                } else if let Some(rows) = value.as_array_mut() {
                    for row in rows {
                        if let Err(error) =
                            target_url(row, &base, &origin, &service, &session_id, readonly)
                        {
                            return crate::browser::http::error(&error);
                        }
                    }
                } else if value.get("id").is_some()
                    && let Err(error) =
                        target_url(&mut value, &base, &origin, &service, &session_id, readonly)
                {
                    return crate::browser::http::error(&error);
                }
            } else if operation == "json/version" {
                return crate::browser::http::error(&crate::browser::http::unsupported());
            }
            Json(value).into_response()
        }
        Err(error) => crate::browser::http::error(&error),
    }
}

fn target_url(
    value: &mut serde_json::Value,
    base: &str,
    origin: &str,
    service: &BrowserService,
    session: &str,
    readonly: bool,
) -> Result<(), open_compute_core::PlatformError> {
    if let Some(id) = value.get("id").and_then(serde_json::Value::as_str) {
        if value["type"] == "page" {
            let view = service.default_view(session, id, origin, readonly)?;
            value["webSocketDebuggerUrl"] = view["webSocketDebuggerUrl"].clone();
            value["devtoolsFrontendUrl"] = view["devtoolsFrontendUrl"].clone();
            value["description"] = "".into();
            return Ok(());
        }
        let mut url = url::Url::parse(base).ok();
        if let Some(url) = &mut url {
            if let Ok(mut path) = url.path_segments_mut() {
                path.push("page").push(id);
            }
            value["webSocketDebuggerUrl"] = url.as_str().into();
        }
    }
    Ok(())
}

fn authority(
    state: &HttpState,
    account_id: &str,
    request: &Request,
    permission: V4Permission,
) -> Result<Arc<BrowserService>, Response> {
    let context = context(request, permission).map_err(super::HttpError::into_response)?;
    let id =
        account(state, account_id).map_err(|error| error_response(error, context.request_id()))?;
    let service = state
        .browser_service()
        .filter(|service| service.instance_id() == id && service.is_available())
        .cloned();
    service.ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))
}

async fn list(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let service = match authority(&state, &account_id, &request, V4Permission::Read) {
        Ok(service) => service,
        Err(response) => return response,
    };
    let Ok(query) = strict_query(&request) else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    if query
        .keys()
        .any(|name| !matches!(name.as_str(), "limit" | "offset"))
    {
        return crate::browser::http::error(&crate::browser::invalid());
    }
    let number = |name: &str, default| {
        query
            .get(name)
            .map(|value| value.parse::<u32>())
            .transpose()
            .map(|value| value.unwrap_or(default))
    };
    let (Ok(limit), Ok(offset)) = (number("limit", 100), number("offset", 0)) else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    if !(1..=200).contains(&limit) {
        return crate::browser::http::error(&crate::browser::invalid());
    }
    match service.list(false, limit, offset) {
        Ok(mut value) => {
            let Some(mut sessions) = value
                .get_mut("sessions")
                .and_then(serde_json::Value::as_array_mut)
                .map(std::mem::take)
            else {
                return crate::browser::http::error(&crate::browser::invalid());
            };
            for session in &mut sessions {
                if let Some(timestamp) = session
                    .get("connectionStartTime")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                {
                    session["connectionStartTime"] = timestamp.into();
                }
            }
            Json(sessions).into_response()
        }
        Err(error) => crate::browser::http::error(&error),
    }
}

async fn acquire(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let service = match authority(&state, &account_id, &request, V4Permission::ProductWrite) {
        Ok(service) => service,
        Err(response) => return response,
    };
    let Ok(query) = strict_query(&request) else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    if query.iter().any(|(key, value)| {
        key != "keep_alive"
            && !(matches!(key.as_str(), "lab" | "recording") && value == "false")
            && !(key == "targets" && matches!(value.as_str(), "true" | "false"))
    }) {
        return crate::browser::http::error(&crate::browser::http::unsupported());
    }
    let body = request.into_body();
    let Ok(body) = to_bytes(body, service.config().max_body_bytes as usize).await else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    if !body.is_empty() {
        let empty = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .is_some_and(|value| value.as_object().is_some_and(serde_json::Map::is_empty));
        if !empty {
            return crate::browser::http::error(&crate::browser::http::unsupported());
        }
    }
    let keep_alive = match query
        .get("keep_alive")
        .map(|value| value.parse::<u64>())
        .transpose()
    {
        Ok(value) => value.unwrap_or(60_000),
        Err(_) => return crate::browser::http::error(&crate::browser::invalid()),
    };
    if let Err(error) = service.public_urls("", state.control_origin_port()) {
        return crate::browser::http::error(&error);
    }
    let id = match service.acquire(keep_alive).await {
        Ok(id) => id,
        Err(error) => return crate::browser::http::error(&error),
    };
    let (base, origin) = match service.public_urls(&id, state.control_origin_port()) {
        Ok(urls) => urls,
        Err(error) => return crate::browser::http::error(&error),
    };
    let mut value = serde_json::json!({"sessionId":id,"webSocketDebuggerUrl":base});
    if query.get("targets").is_some_and(|value| value == "true") {
        let result = async {
            let mut targets = service
                .devtools(&id, "json/list", &Method::GET, &Default::default())
                .await?;
            for row in targets.as_array_mut().ok_or_else(crate::browser::invalid)? {
                target_url(row, &base, &origin, &service, &id, false)?;
            }
            Ok::<_, open_compute_core::PlatformError>(targets)
        }
        .await;
        match result {
            Ok(targets) => value["targets"] = targets,
            Err(error) => {
                if let Err(cleanup) = service.close(&id, false).await {
                    return crate::browser::http::error(&cleanup);
                }
                return crate::browser::http::error(&error);
            }
        }
    }
    Json(value).into_response()
}

async fn connect(
    State(state): State<HttpState>,
    Path((account_id, session_id)): Path<(String, String)>,
    mut request: Request,
) -> Response {
    // A DevTools connection can mutate the browser, even though its HTTP method is GET.
    let service = match authority(&state, &account_id, &request, V4Permission::ProductWrite) {
        Ok(service) => service,
        Err(response) => return response,
    };
    if request.method() == Method::DELETE && request.uri().query().is_some() {
        return crate::browser::http::error(&crate::browser::invalid());
    }
    if uuid::Uuid::parse_str(&session_id).is_err() {
        return crate::browser::http::error(&crate::browser::not_found());
    }
    if request.method() == Method::DELETE {
        if !matches!(to_bytes(request.into_body(), service.config().max_body_bytes as usize).await, Ok(bytes) if bytes.is_empty())
        {
            return crate::browser::http::error(&crate::browser::invalid());
        }
        return match service.close_browser(&session_id).await {
            Ok(()) => Json(serde_json::json!({"status":"closed"})).into_response(),
            Err(error) => crate::browser::http::error(&error),
        };
    }
    if let Err(error) = rewrite(&mut request, &format!("/v1/devtools/browser/{session_id}")) {
        return crate::browser::http::error(&error);
    }
    service.handle_transport(request).await
}

fn rewrite(request: &mut Request, path: &str) -> Result<(), open_compute_core::PlatformError> {
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    *request.uri_mut() = format!("/internal/browser{path}{query}")
        .parse()
        .map_err(|_| crate::browser::invalid())?;
    Ok(())
}

async fn quick_action(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let service = match authority(&state, &account_id, &request, V4Permission::ProductWrite) {
        Ok(service) => service,
        Err(response) => return response,
    };
    let Ok(query) = strict_query(&request) else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    if query.keys().any(|key| key != "cacheTTL")
        || request.headers().contains_key("cf-brapi-guardrails")
    {
        return crate::browser::http::error(&crate::browser::http::unsupported());
    }
    let action = request
        .uri()
        .path()
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned();
    let Ok(bytes) = to_bytes(
        request.into_body(),
        service.config().max_body_bytes as usize,
    )
    .await
    else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    let Ok(mut options) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    let Some(options_object) = options.as_object_mut() else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    // Public SDK parameters put cacheTTL in the query; the binding uses its typed options.
    if options_object.contains_key("cacheTTL") {
        return crate::browser::http::error(&crate::browser::invalid());
    }
    if let Some(ttl) = query.get("cacheTTL") {
        let Ok(ttl) = ttl.parse::<u32>() else {
            return crate::browser::http::error(&crate::browser::invalid());
        };
        if ttl > 86_400 {
            return crate::browser::http::error(&crate::browser::invalid());
        }
        options_object.insert("cacheTTL".into(), ttl.into());
    }
    service
        .action(&action, options)
        .await
        .unwrap_or_else(|error| crate::browser::http::error(&error))
}

async fn live_view(
    State(state): State<HttpState>,
    Path((account_id, id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let service = match authority(&state, &account_id, &request, V4Permission::ProductWrite) {
        Ok(service) => service,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return crate::browser::http::error(&crate::browser::invalid());
    }
    let (_, origin) = match service.public_urls(&id, state.control_origin_port()) {
        Ok(urls) => urls,
        Err(error) => return crate::browser::http::error(&error),
    };
    let Ok(bytes) = to_bytes(
        request.into_body(),
        service.config().max_body_bytes as usize,
    )
    .await
    else {
        return crate::browser::http::error(&crate::browser::invalid());
    };
    match service.live_view(&id, &bytes, &origin).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => crate::browser::http::error(&error),
    }
}
