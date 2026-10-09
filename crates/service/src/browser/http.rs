//! Fixed-package binding routes and immutable binding authority.

use super::*;
use axum::Json;
use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use open_compute_core::{DeploymentId, VersionId, WorkerId};
use open_compute_storage::runtime_features::{BuiltinBindingKind, version_runtime_features};
use open_compute_storage::worker_repository::WorkerRepository;
use std::str::FromStr;

pub(super) fn authorize(
    service: &BrowserService,
    headers: &HeaderMap,
) -> Result<(), PlatformError> {
    let instance = parse_header::<InstanceId>(headers, "x-open-compute-instance-id")?;
    let worker = parse_header::<WorkerId>(headers, "x-open-compute-worker-id")?;
    let version = parse_header::<VersionId>(headers, "x-open-compute-version-id")?;
    let name = text(headers, "x-open-compute-binding-name")?;
    let digest: [u8; 32] = hex::decode(text(headers, "x-open-compute-descriptor-sha256")?)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(denied)?;
    if instance != service.instance || text(headers, "x-open-compute-capability-version")? != "1" {
        return Err(denied());
    }
    let repository = WorkerRepository::new(service.storage.db());
    repository
        .authorize_runtime_version(instance, worker, version)
        .map_err(|_| denied())?;
    if headers.contains_key("x-open-compute-deployment-id") {
        let deployment = parse_header::<DeploymentId>(headers, "x-open-compute-deployment-id")?;
        if repository
            .get_worker_deployment(instance, worker, deployment)
            .map_err(|_| denied())?
            .version_id
            != version
        {
            return Err(denied());
        }
    }
    let (_, bindings) = version_runtime_features(service.storage.db(), version)?;
    if !bindings.iter().any(|binding| {
        binding.kind == BuiltinBindingKind::Browser
            && binding.name == name
            && binding.descriptor_sha256 == digest
    }) {
        return Err(denied());
    }
    Ok(())
}

pub(super) async fn dispatch(
    service: Arc<BrowserService>,
    request: Request,
) -> Result<Response, PlatformError> {
    if request.headers().contains_key("cf-brapi-guardrails") {
        return Err(unsupported());
    }
    if !service.is_available() {
        return Err(backend::unavailable());
    }
    let path = request
        .uri()
        .path()
        .strip_prefix("/internal/browser")
        .ok_or_else(invalid)?
        .to_owned();
    let method = request.method().clone();
    let query = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .into_owned()
        .collect::<Vec<_>>();
    let mut parameters = BTreeMap::new();
    for (key, value) in query {
        if parameters.insert(key, value).is_some() {
            return Err(invalid());
        }
    }
    match (method, path.as_str()) {
        (Method::POST, path) if path.strip_prefix("/v1/").is_some_and(actions::is_action) => {
            if !parameters.is_empty() {
                return Err(invalid());
            }
            let action = path.strip_prefix("/v1/").ok_or_else(invalid)?.to_owned();
            let bytes = to_bytes(request.into_body(), service.config.max_body_bytes as usize)
                .await
                .map_err(|_| invalid())?;
            let options = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            service.action(&action, options).await
        }
        (Method::POST, "/v1/devtools/browser") => {
            if parameters.keys().any(|name| name != "keep_alive") {
                return Err(unsupported());
            }
            let keep_alive = parameters
                .get("keep_alive")
                .map(|value| value.parse::<u64>().map_err(|_| invalid()))
                .transpose()?
                .unwrap_or(60_000);
            if !to_bytes(request.into_body(), service.config.max_body_bytes as usize)
                .await
                .map_err(|_| invalid())?
                .is_empty()
            {
                return Err(unsupported());
            }
            Ok(Json(json!({"sessionId":service.acquire(keep_alive).await?})).into_response())
        }
        (Method::GET, "/v1/sessions" | "/v1/history") => {
            if parameters
                .keys()
                .any(|name| !matches!(name.as_str(), "limit" | "offset"))
            {
                return Err(invalid());
            }
            let number = |name: &str, default| {
                parameters
                    .get(name)
                    .map(|value| value.parse::<u32>().map_err(|_| invalid()))
                    .transpose()
                    .map(|value| value.unwrap_or(default))
            };
            Ok(Json(service.list(
                path == "/v1/history",
                number("limit", 100)?,
                number("offset", 0)?,
            )?)
            .into_response())
        }
        (Method::GET, "/v1/limits") if parameters.is_empty() => {
            Ok(Json(service.limits()?).into_response())
        }
        (method, _) => {
            let id = path
                .strip_prefix("/v1/devtools/browser/")
                .filter(|id| !id.contains('/'))
                .ok_or_else(unsupported)?;
            if !parameters.is_empty() {
                return Err(invalid());
            }
            service.session(id)?;
            match method {
                Method::GET => websocket::upgrade(service, id, request).await,
                Method::DELETE => {
                    service.close(id, false).await?;
                    Ok(StatusCode::NO_CONTENT.into_response())
                }
                _ => Err(unsupported()),
            }
        }
    }
}

pub(crate) fn error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BrowserInputInvalid => StatusCode::BAD_REQUEST,
        ErrorCode::BrowserSessionNotFound | ErrorCode::BindingPermissionDenied => {
            StatusCode::NOT_FOUND
        }
        ErrorCode::BrowserLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::BrowserTimeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::BrowserUnsupported => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(json!({"success":false,"errors":[{"message":error.code().as_str()}]})),
    )
        .into_response()
}
fn text<'a>(headers: &'a HeaderMap, key: &str) -> Result<&'a str, PlatformError> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(denied)
}
fn parse_header<T: FromStr>(headers: &HeaderMap, key: &str) -> Result<T, PlatformError> {
    text(headers, key)?.parse().map_err(|_| denied())
}
fn denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingPermissionDenied,
        "browser binding is denied",
    )
}
pub(crate) fn unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserUnsupported,
        "browser operation is unsupported",
    )
}
