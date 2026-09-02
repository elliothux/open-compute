//! Operator control surface for Vectorize indexes and AI Search namespaces.

use crate::http::{HttpState, ProductErrorCode, authorize};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, RequestId, ResourceId, ResourceState,
};
use open_compute_storage::{
    AI_SEARCH_SCHEMA_VERSION, AiSearchCatalog, PlatformStorage, ResourceRepository,
    VECTORIZE_SCHEMA_VERSION, VectorizeEngine, VectorizeIndexRepository, VectorizePaths,
};
use open_compute_workers::{
    AiSearchNamespaceResourceDriver, CreateResourceOutcome, CreateResourceRequest,
    ResourceController, ResourcePins, VectorizeIndexSpec, VectorizeResourceDriver,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON_BODY: usize = 16 * 1024;
const DEFAULT_VECTOR_QUOTA: u64 = 100_000;
const DEFAULT_VECTOR_BYTES: u64 = 1024 * 1024 * 1024;

/// Shared Vectorize and AI Search operator authority.
#[derive(Clone, Debug)]
pub struct SearchApiState {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    busy_timeout_ms: u64,
    delete_drain_timeout: Duration,
}

impl SearchApiState {
    /// Bind product storage and lifecycle authority.
    #[must_use]
    pub const fn new(
        storage: Arc<PlatformStorage>,
        pins: ResourcePins,
        busy_timeout_ms: u64,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            pins,
            busy_timeout_ms,
            delete_drain_timeout,
        }
    }
}

/// Router for product resource management that is not part of Cloudflare's Worker binding API.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/vectorize/indexes",
            post(create_vector_index).get(list_vector_indexes),
        )
        .route(
            "/v1/accounts/{account_id}/vectorize/indexes/{resource_id}",
            get(get_vector_index).delete(delete_vector_index),
        )
        .route(
            "/v1/accounts/{account_id}/vectorize/indexes/{resource_id}/metadata-indexes",
            post(create_vector_metadata_index),
        )
        .route(
            "/v1/accounts/{account_id}/ai-search/namespaces",
            post(create_ai_namespace).get(list_ai_namespaces),
        )
        .route(
            "/v1/accounts/{account_id}/ai-search/namespaces/{resource_id}",
            get(get_ai_namespace).delete(delete_ai_namespace),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateVectorIndexBody {
    name: String,
    dimensions: u32,
    metric: String,
    #[serde(default = "default_vector_quota")]
    quota_vectors: u64,
    #[serde(default = "default_vector_bytes")]
    quota_bytes: u64,
}

const fn default_vector_quota() -> u64 {
    DEFAULT_VECTOR_QUOTA
}

const fn default_vector_bytes() -> u64 {
    DEFAULT_VECTOR_BYTES
}

async fn create_vector_index(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let body = match read_json::<CreateVectorIndexBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    if !(32..=1536).contains(&body.dimensions)
        || !matches!(body.metric.as_str(), "cosine" | "euclidean" | "dot-product")
        || body.quota_vectors == 0
        || body.quota_vectors > DEFAULT_VECTOR_QUOTA
        || body.quota_bytes < 1024 * 1024
        || body.quota_bytes > DEFAULT_VECTOR_BYTES
    {
        return error_response(&invalid(), request_id);
    }
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let busy_timeout_ms = api.busy_timeout_ms;
    let result = tokio::task::spawn_blocking(move || {
        let spec = VectorizeIndexSpec {
            dimensions: body.dimensions,
            metric: body.metric,
            quota_vectors: body.quota_vectors,
            quota_bytes: body.quota_bytes,
        };
        ResourceController::new(
            &storage,
            pins,
            VectorizeResourceDriver::new(&storage, spec, busy_timeout_ms),
        )
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::VectorizeIndex,
            name: body.name,
            idempotency_key: key,
            driver_schema_version: VECTORIZE_SCHEMA_VERSION,
            request_id,
            now_ms: now_ms(),
        })
    })
    .await;
    create_response(result, request_id)
}

async fn list_vector_indexes(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        VectorizeIndexRepository::new(storage.db()).list(account_id)
    })
    .await
    {
        Ok(Ok(indexes)) => {
            json_response(&serde_json::json!({ "indexes": indexes }), StatusCode::OK)
        }
        Ok(Err(error)) => error_response(&error, request_id),
        Err(_) => error_response(&internal(), request_id),
    }
}

async fn get_vector_index(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        VectorizeIndexRepository::new(storage.db()).get(account_id, resource_id)
    })
    .await
    {
        Ok(Ok(index)) => json_response(&serde_json::json!({ "index": index }), StatusCode::OK),
        Ok(Err(error)) => error_response(&error, request_id),
        Err(_) => error_response(&internal(), request_id),
    }
}

async fn delete_vector_index(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    delete_resource(
        state,
        request,
        account,
        resource,
        BindingKind::VectorizeIndex,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateMetadataIndexBody {
    property_name: String,
    property_type: String,
}

async fn create_vector_metadata_index(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let body = match read_json::<CreateMetadataIndexBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let storage = api.storage.clone();
    let busy_timeout_ms = api.busy_timeout_ms;
    match tokio::task::spawn_blocking(move || {
        let record = VectorizeIndexRepository::new(storage.db()).get(account_id, resource_id)?;
        if record.resource.state != ResourceState::Ready {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "Vectorize index is not ready",
            ));
        }
        let path = VectorizePaths::open(storage.data_dir().root())?.resolve_storage_key(
            &record.storage_key,
            account_id,
            resource_id,
        )?;
        let engine = VectorizeEngine::open(
            &path,
            &resource_id.to_string(),
            record.dimensions,
            &record.metric,
            record.quota_vectors,
            record.quota_bytes,
            busy_timeout_ms,
        )?;
        engine.create_metadata_index(&body.property_name, &body.property_type, now_ms())?;
        engine.indexed_properties()
    })
    .await
    {
        Ok(Ok(properties)) => json_response(
            &serde_json::json!({ "properties": properties }),
            StatusCode::CREATED,
        ),
        Ok(Err(error)) => error_response(&error, request_id),
        Err(_) => error_response(&internal(), request_id),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateNamespaceBody {
    name: String,
}

async fn create_ai_namespace(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let body = match read_json::<CreateNamespaceBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let result = tokio::task::spawn_blocking(move || {
        ResourceController::new(
            &storage,
            pins,
            AiSearchNamespaceResourceDriver::new(&storage),
        )
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::AiSearchNamespace,
            name: body.name,
            idempotency_key: key,
            driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
            request_id,
            now_ms: now_ms(),
        })
    })
    .await;
    create_response(result, request_id)
}

async fn list_ai_namespaces(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        let resources = ResourceRepository::new(storage.db())
            .list(account_id, Some(BindingKind::AiSearchNamespace))?;
        let catalog = AiSearchCatalog::new(storage.db());
        resources
            .into_iter()
            .filter(|resource| resource.state != ResourceState::Tombstoned)
            .map(|resource| catalog.get_namespace(account_id, resource.id))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    {
        Ok(Ok(namespaces)) => json_response(
            &serde_json::json!({ "namespaces": namespaces }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(&error, request_id),
        Err(_) => error_response(&internal(), request_id),
    }
}

async fn get_ai_namespace(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        AiSearchCatalog::new(storage.db()).get_namespace(account_id, resource_id)
    })
    .await
    {
        Ok(Ok(namespace)) => json_response(
            &serde_json::json!({ "namespace": namespace }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(&error, request_id),
        Err(_) => error_response(&internal(), request_id),
    }
}

async fn delete_ai_namespace(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    delete_resource(
        state,
        request,
        account,
        resource,
        BindingKind::AiSearchNamespace,
    )
    .await
}

async fn delete_resource(
    state: HttpState,
    request: Request,
    account: String,
    resource: String,
    kind: BindingKind,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request).cloned() else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(&error, request_id),
    };
    if let Err(error) = idempotency_key(&request) {
        return error_response(&error, request_id);
    }
    let result = match kind {
        BindingKind::VectorizeIndex => {
            ResourceController::new(
                &api.storage,
                api.pins.clone(),
                VectorizeResourceDriver::recovery(&api.storage, api.busy_timeout_ms),
            )
            .delete(
                account_id,
                resource_id,
                request_id,
                now_ms(),
                api.delete_drain_timeout,
            )
            .await
        }
        BindingKind::AiSearchNamespace => {
            ResourceController::new(
                &api.storage,
                api.pins.clone(),
                AiSearchNamespaceResourceDriver::new(&api.storage),
            )
            .delete(
                account_id,
                resource_id,
                request_id,
                now_ms(),
                api.delete_drain_timeout,
            )
            .await
        }
        _ => Err(internal()),
    };
    match result {
        Ok(()) => json_response(
            &serde_json::json!({ "resourceId": resource_id, "state": "tombstoned" }),
            StatusCode::ACCEPTED,
        ),
        Err(error) => error_response(&error, request_id),
    }
}

fn create_response(
    result: Result<Result<CreateResourceOutcome, PlatformError>, tokio::task::JoinError>,
    request_id: RequestId,
) -> Response {
    match result {
        Ok(Ok(CreateResourceOutcome::Applied(value))) => json_response(&value, StatusCode::CREATED),
        Ok(Ok(CreateResourceOutcome::Replay(bytes))) => json_bytes(bytes, StatusCode::OK),
        Ok(Err(error)) => error_response(&error, request_id),
        Err(_) => error_response(&internal(), request_id),
    }
}

fn authorized_api<'a>(state: &'a HttpState, request: &Request) -> Option<&'a Arc<SearchApiState>> {
    if authorize(state, request) {
        state.search_api()
    } else {
        None
    }
}

fn unauthorized_or_unavailable(
    state: &HttpState,
    request: &Request,
    request_id: RequestId,
) -> Response {
    if authorize(state, request) {
        StatusCode::NOT_FOUND.into_response()
    } else {
        error_response(
            &PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "admin authentication is required",
            ),
            request_id,
        )
    }
}

async fn read_json<T: for<'de> Deserialize<'de>>(request: Request) -> Result<T, PlatformError> {
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

fn idempotency_key(request: &Request) -> Result<String, PlatformError> {
    let key = request
        .headers()
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid());
    }
    Ok(key.to_owned())
}

fn parse_account(value: &str) -> Result<AccountId, PlatformError> {
    AccountId::from_str(value).map_err(|_| invalid())
}

fn parse_ids(account: &str, resource: &str) -> Result<(AccountId, ResourceId), PlatformError> {
    Ok((
        parse_account(account)?,
        ResourceId::from_str(resource).map_err(|_| invalid())?,
    ))
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn json_response(value: &impl Serialize, status: StatusCode) -> Response {
    serde_json::to_vec(value).map_or_else(
        |_| error_response(&internal(), RequestId::generate()),
        |bytes| json_bytes(bytes, status),
    )
}

fn json_bytes(bytes: Vec<u8>, status: StatusCode) -> Response {
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

fn error_response(error: &PlatformError, request_id: RequestId) -> Response {
    let code = error.code();
    let status = match code {
        ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::ResourceNameConflict
        | ErrorCode::IdempotencyConflict
        | ErrorCode::ResourceReferenced
        | ErrorCode::ResourceNotReady => StatusCode::CONFLICT,
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::ConfigInvalid | ErrorCode::LimitInvalid | ErrorCode::BindingLimitExceeded => {
            StatusCode::BAD_REQUEST
        }
        ErrorCode::QuotaExceeded | ErrorCode::AdmissionBusy => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit => StatusCode::INSUFFICIENT_STORAGE,
        ErrorCode::PlatformUnavailable | ErrorCode::ResourceUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": code.as_str(),
                "message": "search resource control request failed",
                "requestId": request_id,
            }
        })),
    )
        .into_response();
    response.extensions_mut().insert(ProductErrorCode(code));
    response
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "search resource configuration is invalid",
    )
}

fn internal() -> PlatformError {
    PlatformError::new(
        ErrorCode::Internal,
        "search resource control operation failed",
    )
}

#[cfg(test)]
#[path = "search_http_tests.rs"]
mod tests;
