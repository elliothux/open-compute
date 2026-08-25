//! P0.7 Durable Object namespace and object lifecycle control API.

use crate::http::{HttpState, authorize};
use crate::metrics::{DoFacetReloadReason, DoReconcileState, MetricsRegistry};
use crate::runtime_bridge::WorkerdTransport;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_core::{
    AccountId, BindingKind, DurableObjectId, DurableObjectState, DurableObjectsConfig, ErrorCode,
    PlatformError, RequestId, ResourceId, WorkerId,
};
use open_compute_storage::{
    AuthorizedDurableObjectDelete, DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRecord,
    DurableObjectRepository, PlatformStorage, ResourceRepository,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, DurableObjectResourceDriver, ResourceController,
    ResourcePins,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON_BODY: usize = 4096;
const IDEMPOTENCY_HEADER: &str = "idempotency-key";

trait DurableObjectDeleteTransport: Send + Sync {
    fn delete<'a>(
        &'a self,
        authority: &'a AuthorizedDurableObjectDelete,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>>;
}

impl DurableObjectDeleteTransport for WorkerdTransport {
    fn delete<'a>(
        &'a self,
        authority: &'a AuthorizedDurableObjectDelete,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>> {
        Box::pin(self.delete_durable_object(authority))
    }
}

/// Shared Durable Object control-plane composition state.
#[derive(Clone)]
pub struct DoApiState {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    transport: Arc<dyn DurableObjectDeleteTransport>,
    config: DurableObjectsConfig,
    delete_drain_timeout: Duration,
    metrics: Option<Arc<MetricsRegistry>>,
}

impl std::fmt::Debug for DoApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoApiState")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl DoApiState {
    /// Bind central authority, runtime transport, pins, and operator limits.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        pins: ResourcePins,
        transport: WorkerdTransport,
        config: DurableObjectsConfig,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            pins,
            transport: Arc::new(transport),
            config,
            delete_drain_timeout,
            metrics: None,
        }
    }

    /// Record lifecycle activity in the fixed low-cardinality metric registry.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Resume a bounded batch of native deletes fenced before a prior crash.
    pub async fn reconcile_pending(&self) -> Result<u32, PlatformError> {
        let candidates = DurableObjectRepository::new(&self.storage)
            .reconcile_candidates(self.config.reconcile_batch)?;
        let mut completed = 0_u32;
        for object in candidates {
            if object.state == DurableObjectState::Creating {
                let result = DurableObjectRepository::new(&self.storage).finish_object_create(
                    object.namespace_resource_id,
                    object.object_id,
                    object.generation,
                    now_ms(),
                );
                if let Some(metrics) = &self.metrics {
                    metrics.inc_do_reconcile(DoReconcileState::Creating, result.is_ok());
                }
                result?;
                completed = completed.saturating_add(1);
            } else if object.state == DurableObjectState::Deleting {
                let namespace = DurableObjectRepository::new(&self.storage)
                    .get_namespace_by_resource(object.namespace_resource_id)?;
                let result = self
                    .delete_fenced_object(namespace.resource.account_id, object)
                    .await;
                if let Some(metrics) = &self.metrics {
                    metrics.inc_do_reconcile(DoReconcileState::Deleting, result.is_ok());
                }
                result?;
                completed = completed.saturating_add(1);
            }
        }
        if let Some(metrics) = &self.metrics {
            metrics.set_do_active_hosts(
                DurableObjectRepository::new(&self.storage).count_live_objects()?,
            );
        }
        Ok(completed)
    }

    async fn delete_fenced_object(
        &self,
        account_id: AccountId,
        object: DurableObjectRecord,
    ) -> Result<(), PlatformError> {
        let repository = DurableObjectRepository::new(&self.storage);
        let authority = repository.deletion_authority(
            account_id,
            object.namespace_resource_id,
            object.object_id,
            object.generation,
        )?;
        self.transport.delete(&authority).await?;
        if let Some(metrics) = &self.metrics {
            metrics.inc_do_facet_reload(DoFacetReloadReason::Delete);
        }
        repository.finish_object_delete(
            object.namespace_resource_id,
            object.object_id,
            object.generation,
            now_ms(),
        )?;
        Ok(())
    }
}

/// Router for Durable Object namespace and object management.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/durable-objects/namespaces",
            post(create_namespace).get(list_namespaces),
        )
        .route(
            "/v1/accounts/{account_id}/durable-objects/namespaces/{namespace_id}",
            get(get_namespace).patch(rename_namespace).delete(delete_namespace),
        )
        .route(
            "/v1/accounts/{account_id}/durable-objects/namespaces/{namespace_id}/objects",
            get(list_objects),
        )
        .route(
            "/v1/accounts/{account_id}/durable-objects/namespaces/{namespace_id}/objects/{object_id}",
            get(get_object).delete(delete_object),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateNamespaceBody {
    name: String,
    worker_id: WorkerId,
    class_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceView {
    resource_id: ResourceId,
    name: String,
    state: open_compute_core::ResourceState,
    owner_worker_id: WorkerId,
    class_name: String,
    schema_version: u32,
    created_at_ms: i64,
}

fn namespace_view(value: open_compute_storage::DurableObjectNamespaceRecord) -> NamespaceView {
    NamespaceView {
        resource_id: value.resource.id,
        name: value.resource.name,
        state: value.resource.state,
        owner_worker_id: value.owner_worker_id,
        class_name: value.class_name,
        schema_version: value.schema_version,
        created_at_ms: value.created_at_ms,
    }
}

async fn create_namespace(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let Ok(account_id) = AccountId::from_str(&account) else {
        return error_response(invalid(), request_id);
    };
    let key = match header(&request, IDEMPOTENCY_HEADER) {
        Some(value) if !value.is_empty() && value.len() <= 128 => value.to_owned(),
        _ => return error_response(invalid(), request_id),
    };
    let body = match read_json::<CreateNamespaceBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if body.name.len() > api.config.max_namespace_name_bytes as usize {
        return error_response(invalid(), request_id);
    }
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let result = tokio::task::spawn_blocking(move || {
        let driver = DurableObjectResourceDriver::new(&storage, body.worker_id, &body.class_name);
        ResourceController::new(&storage, pins, driver).create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: body.name,
            idempotency_key: key,
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id,
            now_ms: now_ms(),
        })
    })
    .await;
    match result {
        Ok(Ok(CreateResourceOutcome::Applied(value))) => json(&value, StatusCode::CREATED),
        Ok(Ok(CreateResourceOutcome::Replay(value))) => json_bytes(value, StatusCode::OK),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn list_namespaces(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let Ok(account_id) = AccountId::from_str(&account) else {
        return error_response(invalid(), request_id);
    };
    match DurableObjectRepository::new(&api.storage).list_namespaces(account_id) {
        Ok(value) => json(
            &serde_json::json!({
                "namespaces": value.into_iter().map(namespace_view).collect::<Vec<_>>()
            }),
            StatusCode::OK,
        ),
        Err(error) => error_response(error, request_id),
    }
}

async fn get_namespace(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let (account_id, namespace_id) = match ids(&account, &namespace) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match DurableObjectRepository::new(&api.storage).get_namespace(account_id, namespace_id) {
        Ok(value) => json(
            &serde_json::json!({ "namespace": namespace_view(value) }),
            StatusCode::OK,
        ),
        Err(error) => error_response(error, request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameBody {
    name: String,
}

async fn rename_namespace(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let (account_id, namespace_id) = match ids(&account, &namespace) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<RenameBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if body.name.len() > api.config.max_namespace_name_bytes as usize {
        return error_response(invalid(), request_id);
    }
    let namespace =
        match DurableObjectRepository::new(&api.storage).get_namespace(account_id, namespace_id) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
    let driver = DurableObjectResourceDriver::new(
        &api.storage,
        namespace.owner_worker_id,
        &namespace.class_name,
    );
    match ResourceController::new(&api.storage, api.pins.clone(), driver).rename(
        account_id,
        namespace_id,
        &body.name,
        request_id,
        now_ms(),
    ) {
        Ok(value) => json(&value, StatusCode::OK),
        Err(error) => error_response(error, request_id),
    }
}

async fn list_objects(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let (account_id, namespace_id) = match ids(&account, &namespace) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match DurableObjectRepository::new(&api.storage).list_objects(account_id, namespace_id) {
        Ok(value) => json(&serde_json::json!({ "objects": value }), StatusCode::OK),
        Err(error) => error_response(error, request_id),
    }
}

async fn get_object(
    State(state): State<HttpState>,
    Path((account, namespace, object)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let (account_id, namespace_id) = match ids(&account, &namespace) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let Ok(object_id) = DurableObjectId::from_str(&object) else {
        return error_response(do_id_invalid(), request_id);
    };
    match DurableObjectRepository::new(&api.storage).list_objects(account_id, namespace_id) {
        Ok(objects) => match objects
            .into_iter()
            .rev()
            .find(|row| row.object_id == object_id)
        {
            Some(value) => json(&value, StatusCode::OK),
            None => error_response(not_found(), request_id),
        },
        Err(error) => error_response(error, request_id),
    }
}

async fn delete_object(
    State(state): State<HttpState>,
    Path((account, namespace, object)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let (account_id, namespace_id) = match ids(&account, &namespace) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let Ok(object_id) = DurableObjectId::from_str(&object) else {
        return error_response(do_id_invalid(), request_id);
    };
    let fenced = match DurableObjectRepository::new(&api.storage).begin_object_delete(
        account_id,
        namespace_id,
        object_id,
        now_ms(),
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match api.delete_fenced_object(account_id, fenced).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error, request_id),
    }
}

async fn delete_namespace(
    State(state): State<HttpState>,
    Path((account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let force = request
        .uri()
        .query()
        .is_some_and(|query| query == "force=true");
    let (account_id, namespace_id) = match ids(&account, &namespace) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let namespace =
        match DurableObjectRepository::new(&api.storage).get_namespace(account_id, namespace_id) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
    let referrers = match ResourceRepository::new(api.storage.db()).referrers(namespace_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if !referrers.is_empty() {
        return error_response(
            PlatformError::new(
                ErrorCode::ResourceReferenced,
                "namespace still has retained deployment bindings",
            ),
            request_id,
        );
    }
    let objects =
        match DurableObjectRepository::new(&api.storage).list_objects(account_id, namespace_id) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
    if !force
        && objects
            .iter()
            .any(|row| row.state != DurableObjectState::Tombstoned)
    {
        return error_response(
            PlatformError::new(ErrorCode::DoNamespaceNotEmpty, "namespace is not empty"),
            request_id,
        );
    }
    if force {
        for object in objects
            .into_iter()
            .filter(|row| row.state != DurableObjectState::Tombstoned)
        {
            let fenced = if object.state == DurableObjectState::Deleting {
                object
            } else {
                match DurableObjectRepository::new(&api.storage).begin_object_delete(
                    account_id,
                    namespace_id,
                    object.object_id,
                    now_ms(),
                ) {
                    Ok(value) => value,
                    Err(error) => return error_response(error, request_id),
                }
            };
            if let Err(error) = api.delete_fenced_object(account_id, fenced).await {
                return error_response(error, request_id);
            }
        }
    }
    let driver = DurableObjectResourceDriver::new(
        &api.storage,
        namespace.owner_worker_id,
        &namespace.class_name,
    );
    match ResourceController::new(&api.storage, api.pins.clone(), driver)
        .delete(
            account_id,
            namespace_id,
            request_id,
            now_ms(),
            api.delete_drain_timeout,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error, request_id),
    }
}

fn authorized_api<'a>(state: &'a HttpState, request: &Request) -> Option<&'a Arc<DoApiState>> {
    authorize(state, request).then(|| state.do_api()).flatten()
}

fn unavailable(state: &HttpState, request: &Request) -> Response {
    if authorize(state, request) {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

async fn read_json<T: for<'de> Deserialize<'de>>(request: Request) -> Result<T, PlatformError> {
    if request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        != Some("application/json")
    {
        return Err(invalid());
    }
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

fn ids(account: &str, namespace: &str) -> Result<(AccountId, ResourceId), PlatformError> {
    Ok((
        AccountId::from_str(account).map_err(|_| invalid())?,
        ResourceId::from_str(namespace).map_err(|_| invalid())?,
    ))
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers().get(name)?.to_str().ok()
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
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn json(value: &impl Serialize, status: StatusCode) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => json_bytes(bytes, status),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn json_bytes(bytes: Vec<u8>, status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn error_response(error: impl Into<PlatformError>, request_id: RequestId) -> Response {
    let error = error.into();
    let code = error.code();
    let status = match code {
        ErrorCode::DoNamespaceNotFound | ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::DoIdInvalid | ErrorCode::DoInternalProtocolError => StatusCode::BAD_REQUEST,
        ErrorCode::DoNamespaceNotEmpty
        | ErrorCode::ResourceReferenced
        | ErrorCode::DoObjectDeleting
        | ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ErrorCode::DoStorageUnavailable | ErrorCode::DoDispatchTimeout => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::ResourceNameConflict => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    json(
        &serde_json::json!({
            "ok": false,
            "error": { "code": code.as_str(), "message": "Durable Object operation failed", "requestId": request_id }
        }),
        status,
    )
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoInternalProtocolError,
        "invalid Durable Object request",
    )
}
fn internal() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoStorageUnavailable,
        "Durable Object task failed",
    )
}
fn do_id_invalid() -> PlatformError {
    PlatformError::new(ErrorCode::DoIdInvalid, "Durable Object identity is invalid")
}
fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "Durable Object was not found")
}

#[cfg(test)]
#[path = "do_http_tests.rs"]
mod tests;
