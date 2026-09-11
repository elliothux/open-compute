//! Version-scoped Cloudflare Artifacts Worker-binding adapter.

use super::{VERSION_HEADER, parse_digest, parse_header};
use crate::artifact_api::{
    ArtifactApiState, CreateRepositoryRequest, ForkRepositoryRequest, ImportRepositoryRequest,
    IssuedArtifactToken,
};
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use open_compute_core::{BindingId, ErrorCode, PlatformError, VersionId};
use open_compute_storage::{
    ArtifactRepositoryRecord, ArtifactRepositoryState, ArtifactTokenRecord, ArtifactTokenScope,
    CloudflareArtifactsRepository, PlatformStorage,
};
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_BODY: usize = 64 * 1024;
const INVALID_INPUT: u16 = 10_100;
const INVALID_REPO_NAME: u16 = 10_101;
const INVALID_TTL: u16 = 10_103;
const INVALID_URL: u16 = 10_104;
const REMOTE_AUTH_REQUIRED: u16 = 10_106;
const NOT_FOUND: u16 = 10_200;
const ALREADY_EXISTS: u16 = 10_201;
const IMPORT_IN_PROGRESS: u16 = 10_302;
const FORK_IN_PROGRESS: u16 = 10_303;
const INTERNAL_ERROR: u16 = 10_400;
const UPSTREAM_UNAVAILABLE: u16 = 10_401;
const MEMORY_LIMIT: u16 = 10_402;

pub(super) async fn handle(
    api: &Arc<ArtifactApiState>,
    storage: &Arc<PlatformStorage>,
    request: Request,
) -> Response {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/json")
    {
        return artifact_error("INVALID_INPUT", INVALID_INPUT, StatusCode::BAD_REQUEST);
    }
    let Some((binding_id, operation)) = parse_path(request.uri().path()) else {
        return artifact_error("INVALID_INPUT", INVALID_INPUT, StatusCode::NOT_FOUND);
    };
    let operation = operation.to_owned();
    let Ok(version) = parse_header::<VersionId>(request.headers(), VERSION_HEADER) else {
        return artifact_error("INVALID_INPUT", INVALID_INPUT, StatusCode::BAD_REQUEST);
    };
    let Ok(digest) = parse_digest(request.headers()) else {
        return artifact_error("INVALID_INPUT", INVALID_INPUT, StatusCode::BAD_REQUEST);
    };
    let authorized = match CloudflareArtifactsRepository::new(storage.db())
        .authorize_binding(binding_id, version, &digest)
    {
        Ok(value) => value,
        Err(error) => return from_platform_error(&error),
    };
    let body = match to_bytes(request.into_body(), MAX_BODY).await {
        Ok(value) => match serde_json::from_slice::<Value>(&value) {
            Ok(Value::Object(value)) => value,
            _ => return artifact_error("INVALID_INPUT", INVALID_INPUT, StatusCode::BAD_REQUEST),
        },
        Err(_) => {
            return artifact_error("MEMORY_LIMIT", MEMORY_LIMIT, StatusCode::PAYLOAD_TOO_LARGE);
        }
    };
    let write = matches!(
        operation.as_str(),
        "create" | "delete" | "create-token" | "revoke-token" | "fork" | "import"
    );
    if write && !authorized.binding.permissions.write
        || !write && !authorized.binding.permissions.read
    {
        return artifact_error("NOT_FOUND", NOT_FOUND, StatusCode::FORBIDDEN);
    }
    dispatch(
        api,
        authorized.namespace.account_id,
        &authorized.namespace.name,
        &operation,
        &body,
    )
    .await
}

async fn dispatch(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    operation: &str,
    body: &serde_json::Map<String, Value>,
) -> Response {
    let now = open_compute_core::wall_time_ms();
    let result = match operation {
        "create" => create(api, account, namespace, body, now),
        "get" => get(api, account, namespace, body),
        "list" => list(api, account, namespace, body),
        "delete" => delete(api, account, namespace, body, now),
        "create-token" => create_token(api, account, namespace, body, now),
        "list-tokens" => list_tokens(api, account, namespace, body, now),
        "revoke-token" => revoke_token(api, account, namespace, body, now),
        "fork" => fork(api, account, namespace, body, now),
        "import" => import(api, account, namespace, body, now).await,
        _ => Err(ArtifactBindingError::new(
            "INVALID_INPUT",
            INVALID_INPUT,
            StatusCode::NOT_FOUND,
        )),
    };
    match result {
        Ok(value) => json_response(&value),
        Err(error) => artifact_error(error.code, error.numeric, error.status),
    }
}

fn create(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    let name = string(body, "name")?;
    let opts = body.get("opts").and_then(Value::as_object);
    let description = opts
        .and_then(|value| value.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let branch = opts
        .and_then(|value| value.get("setDefaultBranch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let read_only = opts
        .and_then(|value| value.get("readOnly"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let repository = api
        .create_repository(
            account,
            namespace,
            CreateRepositoryRequest {
                name,
                description,
                default_branch: branch,
                read_only,
            },
            now,
        )
        .map_err(ArtifactBindingError::from)?;
    let token = api
        .issue_initial_token(account, namespace, name, now)
        .map_err(ArtifactBindingError::from)?;
    created_repo(api, namespace, &repository, &token)
}

fn get(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
) -> Result<Value, ArtifactBindingError> {
    let repository = api
        .repository_for_binding(account, namespace, string(body, "name")?)
        .map_err(ArtifactBindingError::from)?;
    repo_info(api, namespace, &repository)
}

fn list(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
) -> Result<Value, ArtifactBindingError> {
    let limit = body.get("limit").and_then(Value::as_u64).unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(invalid());
    }
    let offset = body
        .get("cursor")
        .map(|value| value.as_str().ok_or_else(invalid).and_then(decode_cursor))
        .transpose()?
        .unwrap_or(0);
    let repositories = api
        .list_repositories(account, namespace)
        .map_err(ArtifactBindingError::from)?;
    let total = repositories.len();
    if offset > total {
        return Err(invalid());
    }
    let limit = usize::try_from(limit).map_err(|_| invalid())?;
    let repos = repositories
        .iter()
        .skip(offset)
        .take(limit)
        .map(repo_list_info)
        .collect::<Result<Vec<_>, _>>()?;
    let next = offset.saturating_add(repos.len());
    let mut result = json!({ "repos": repos, "total": total });
    if next < total {
        result
            .as_object_mut()
            .ok_or_else(ArtifactBindingError::internal)?
            .insert("cursor".to_owned(), Value::String(encode_cursor(next)));
    }
    Ok(result)
}

fn delete(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    match api.delete_repository(account, namespace, string(body, "name")?, now) {
        Ok(_) => Ok(Value::Bool(true)),
        Err(error) if error.code() == ErrorCode::ResourceNotFound => Ok(Value::Bool(false)),
        Err(error) => Err(error.into()),
    }
}

fn create_token(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    let scope = match body.get("scope").and_then(Value::as_str).unwrap_or("write") {
        "read" => ArtifactTokenScope::Read,
        "write" => ArtifactTokenScope::Write,
        _ => return Err(invalid()),
    };
    let ttl = match body.get("ttl") {
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(invalid)?,
        ),
        None => None,
    };
    let token = api
        .issue_token(
            account,
            namespace,
            string(body, "repository")?,
            scope,
            ttl,
            now,
        )
        .map_err(ArtifactBindingError::from)?;
    Ok(json!({
        "id": token.record.id,
        "plaintext": token.plaintext,
        "scope": token.record.scope,
        "expiresAt": timestamp(token.record.expires_at_ms)?,
    }))
}

fn list_tokens(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    let tokens = api
        .list_tokens(account, namespace, string(body, "repository")?)
        .map_err(ArtifactBindingError::from)?;
    let total = tokens.len();
    let tokens = tokens
        .iter()
        .map(|token| token_info(token, now))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "tokens": tokens, "total": total }))
}

fn revoke_token(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    let revoked = api
        .revoke_token_value(
            account,
            namespace,
            string(body, "repository")?,
            string(body, "tokenOrId")?,
            now,
        )
        .map_err(ArtifactBindingError::from)?;
    Ok(Value::Bool(revoked))
}

fn fork(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    let source = string(body, "repository")?;
    let target = string(body, "name")?;
    let opts = body.get("opts").and_then(Value::as_object);
    let repository = api
        .fork_repository(
            account,
            namespace,
            ForkRepositoryRequest {
                source_name: source,
                target_name: target,
                description: opts
                    .and_then(|value| value.get("description"))
                    .and_then(Value::as_str),
                read_only: opts
                    .and_then(|value| value.get("readOnly"))
                    .and_then(Value::as_bool),
                default_branch_only: opts
                    .and_then(|value| value.get("defaultBranchOnly"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            },
            now,
        )
        .map_err(ArtifactBindingError::from)?;
    let token = api
        .issue_initial_token(account, namespace, target, now)
        .map_err(ArtifactBindingError::from)?;
    created_repo(api, namespace, &repository, &token)
}

async fn import(
    api: &ArtifactApiState,
    account: open_compute_core::AccountId,
    namespace: &str,
    body: &serde_json::Map<String, Value>,
    now: i64,
) -> Result<Value, ArtifactBindingError> {
    let source = body
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let target = body
        .get("target")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let target_options = target.get("opts").and_then(Value::as_object);
    let depth = match source.get("depth") {
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(invalid)?,
        ),
        None => None,
    };
    let name = string(target, "name")?;
    let repository = api
        .import_repository(ImportRepositoryRequest {
            account,
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            remote: string(source, "url")?.to_owned(),
            branch: source
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_owned),
            depth,
            description: target_options
                .and_then(|value| value.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            read_only: target_options
                .and_then(|value| value.get("readOnly"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            now_ms: now,
        })
        .await
        .map_err(ArtifactBindingError::from_import)?;
    let token = api
        .issue_initial_token(account, namespace, name, now)
        .map_err(ArtifactBindingError::from)?;
    created_repo(api, namespace, &repository, &token)
}

fn repo_info(
    api: &ArtifactApiState,
    namespace: &str,
    repository: &ArtifactRepositoryRecord,
) -> Result<Value, ArtifactBindingError> {
    match repository.state {
        ArtifactRepositoryState::Ready => {}
        ArtifactRepositoryState::Importing => {
            return Err(ArtifactBindingError::new(
                "IMPORT_IN_PROGRESS",
                IMPORT_IN_PROGRESS,
                StatusCode::CONFLICT,
            ));
        }
        ArtifactRepositoryState::Forking => {
            return Err(ArtifactBindingError::new(
                "FORK_IN_PROGRESS",
                FORK_IN_PROGRESS,
                StatusCode::CONFLICT,
            ));
        }
        _ => {
            return Err(ArtifactBindingError::new(
                "UPSTREAM_UNAVAILABLE",
                UPSTREAM_UNAVAILABLE,
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    }
    Ok(json!({
        "id": repository.id,
        "name": repository.name,
        "description": if repository.description.is_empty() { Value::Null } else { Value::String(repository.description.clone()) },
        "defaultBranch": repository.default_branch,
        "createdAt": timestamp(repository.created_at_ms)?,
        "updatedAt": timestamp(repository.updated_at_ms)?,
        "lastPushAt": repository.last_push_at_ms.map(timestamp).transpose()?,
        "source": repository.source,
        "readOnly": repository.read_only,
        "remote": api.remote(namespace, &repository.name),
    }))
}

fn repo_list_info(repository: &ArtifactRepositoryRecord) -> Result<Value, ArtifactBindingError> {
    debug_assert_eq!(repository.state, ArtifactRepositoryState::Ready);
    Ok(json!({
        "id": repository.id,
        "name": repository.name,
        "description": if repository.description.is_empty() { Value::Null } else { Value::String(repository.description.clone()) },
        "defaultBranch": repository.default_branch,
        "createdAt": timestamp(repository.created_at_ms)?,
        "updatedAt": timestamp(repository.updated_at_ms)?,
        "lastPushAt": repository.last_push_at_ms.map(timestamp).transpose()?,
        "source": repository.source,
        "readOnly": repository.read_only,
    }))
}

fn encode_cursor(offset: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(offset.to_string())
}

fn decode_cursor(value: &str) -> Result<usize, ArtifactBindingError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(invalid)
}

fn created_repo(
    api: &ArtifactApiState,
    namespace: &str,
    repository: &ArtifactRepositoryRecord,
    token: &IssuedArtifactToken,
) -> Result<Value, ArtifactBindingError> {
    let info = repo_info(api, namespace, repository)?;
    Ok(json!({
        "id": info["id"], "name": info["name"], "description": info["description"],
        "defaultBranch": info["defaultBranch"], "remote": info["remote"],
        "token": token.plaintext, "tokenExpiresAt": timestamp(token.record.expires_at_ms)?,
    }))
}

fn token_info(token: &ArtifactTokenRecord, now: i64) -> Result<Value, ArtifactBindingError> {
    let state = if token.revoked_at_ms.is_some() {
        "revoked"
    } else if token.expires_at_ms <= now {
        "expired"
    } else {
        "active"
    };
    Ok(json!({
        "id": token.id, "scope": token.scope, "state": state,
        "createdAt": timestamp(token.created_at_ms)?, "expiresAt": timestamp(token.expires_at_ms)?,
    }))
}

fn string<'a>(
    body: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a str, ArtifactBindingError> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid)
}

fn timestamp(value: i64) -> Result<String, ArtifactBindingError> {
    crate::cloudflare_v4::iso_timestamp(value).map_err(|_| ArtifactBindingError::internal())
}

fn parse_path(path: &str) -> Option<(BindingId, &str)> {
    let rest = path.strip_prefix("/internal/bindings/v1/artifacts/")?;
    let (id, operation) = rest.split_once('/')?;
    if operation.contains('/') {
        return None;
    }
    Some((id.parse().ok()?, operation))
}

fn json_response(value: &Value) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn from_platform_error(error: &PlatformError) -> Response {
    let error = ArtifactBindingError::from(error.clone());
    artifact_error(error.code, error.numeric, error.status)
}

fn artifact_error(code: &'static str, numeric: u16, status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header("x-open-compute-error-code", code)
        .header("x-open-compute-error-numeric", numeric)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Debug)]
struct ArtifactBindingError {
    code: &'static str,
    numeric: u16,
    status: StatusCode,
}
impl ArtifactBindingError {
    const fn new(code: &'static str, numeric: u16, status: StatusCode) -> Self {
        Self {
            code,
            numeric,
            status,
        }
    }
    const fn internal() -> Self {
        Self::new(
            "INTERNAL_ERROR",
            INTERNAL_ERROR,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    }
    fn from_import(error: PlatformError) -> Self {
        match error.code() {
            ErrorCode::ArtifactUnavailable => {
                Self::new("INVALID_URL", INVALID_URL, StatusCode::BAD_REQUEST)
            }
            ErrorCode::BindingPermissionDenied => Self::new(
                "REMOTE_AUTH_REQUIRED",
                REMOTE_AUTH_REQUIRED,
                StatusCode::UNAUTHORIZED,
            ),
            _ => Self::from(error),
        }
    }
}
impl From<PlatformError> for ArtifactBindingError {
    fn from(error: PlatformError) -> Self {
        Self::from(&error)
    }
}
impl From<&PlatformError> for ArtifactBindingError {
    fn from(error: &PlatformError) -> Self {
        match error.code() {
            ErrorCode::ResourceNotFound => Self::new("NOT_FOUND", NOT_FOUND, StatusCode::NOT_FOUND),
            ErrorCode::ResourceNameConflict => {
                Self::new("ALREADY_EXISTS", ALREADY_EXISTS, StatusCode::CONFLICT)
            }
            ErrorCode::LimitInvalid => {
                Self::new("INVALID_TTL", INVALID_TTL, StatusCode::BAD_REQUEST)
            }
            ErrorCode::ConfigInvalid => Self::new(
                "INVALID_REPO_NAME",
                INVALID_REPO_NAME,
                StatusCode::BAD_REQUEST,
            ),
            ErrorCode::PathInvalid => {
                Self::new("INVALID_INPUT", INVALID_INPUT, StatusCode::BAD_REQUEST)
            }
            ErrorCode::QuotaExceeded | ErrorCode::ResourceLimitExceeded => {
                Self::new("MEMORY_LIMIT", MEMORY_LIMIT, StatusCode::PAYLOAD_TOO_LARGE)
            }
            ErrorCode::ResourceUnavailable | ErrorCode::ResourceNotReady => Self::new(
                "UPSTREAM_UNAVAILABLE",
                UPSTREAM_UNAVAILABLE,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            _ => Self::internal(),
        }
    }
}
fn invalid() -> ArtifactBindingError {
    ArtifactBindingError::new("INVALID_INPUT", INVALID_INPUT, StatusCode::BAD_REQUEST)
}

#[cfg(test)]
mod tests;
