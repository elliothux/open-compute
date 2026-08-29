//! Authenticated Workflow definition/version lifecycle and secret-free inspection.

use crate::http::{HttpState, ProductErrorCode, authorize};
use crate::runtime_bridge::WorkerdTransport;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use open_compute_core::{
    AccountId, DeploymentId, ErrorCode, PlatformError, RequestId, SchedulerClock as _,
    SystemSchedulerClock, WorkflowId, WorkflowInstanceId,
};
use open_compute_storage::{
    DeploymentState, PlatformStorage, SchedulerStore, WorkflowRepository, WorkflowVersion,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

mod instances;

/// Workflow control composition; tenant create/status remains on the private binding backend.
#[derive(Clone, Debug)]
pub struct WorkflowApiState {
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    transport: WorkerdTransport,
    limits: open_compute_core::WorkflowsConfig,
}

impl WorkflowApiState {
    /// Bind the catalog, scheduler authority, and verified runtime class probe.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        scheduler: Arc<SchedulerStore>,
        transport: WorkerdTransport,
        limits: open_compute_core::WorkflowsConfig,
    ) -> Self {
        Self {
            storage,
            scheduler,
            transport,
            limits,
        }
    }

    /// Freeze a target before probing; an Unknown result retains the validating version for recovery.
    pub async fn create_version(
        &self,
        account: AccountId,
        definition: WorkflowId,
        deployment: DeploymentId,
        class_name: String,
    ) -> Result<WorkflowVersion, PlatformError> {
        let storage = self.storage.clone();
        let version = tokio::task::spawn_blocking(move || {
            let _admission = storage.reserve_mutation(64 * 1024)?;
            WorkflowRepository::new(storage.db()).stage_version(
                account,
                definition,
                deployment,
                &class_name,
                now_ms(),
            )
        })
        .await
        .map_err(|_| unavailable())??;
        validate_version(self.storage.clone(), &self.transport, version).await
    }
}

/// Validate only a frozen class, then atomically select the newest proven version.
pub(crate) async fn validate_version(
    storage: Arc<PlatformStorage>,
    transport: &WorkerdTransport,
    version: WorkflowVersion,
) -> Result<WorkflowVersion, PlatformError> {
    let probe = transport.probe_workflow(&version.target).await;
    let accepted = match probe {
        Ok(()) => true,
        Err(error)
            if matches!(
                error.code(),
                ErrorCode::WorkflowVersionNotReady
                    | ErrorCode::ArtifactIntegrityError
                    | ErrorCode::WorkflowInvariantViolation
            ) =>
        {
            false
        }
        Err(_) => return Ok(version),
    };
    tokio::task::spawn_blocking(move || {
        WorkflowRepository::new(storage.db()).finish_version(
            version.target.account_id,
            version.target.version_id,
            accepted,
            now_ms(),
        )
    })
    .await
    .map_err(|_| unavailable())?
}

/// Account-scoped catalog and bounded operator history; no payload/SQL mutation endpoints.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .merge(instances::routes())
        .route(
            "/v1/accounts/{account}/workflows",
            post(create_definition).get(list_definitions),
        )
        .route(
            "/v1/accounts/{account}/workflows/{definition}",
            get(inspect_definition)
                .patch(rename_definition)
                .delete(delete_definition),
        )
        .route(
            "/v1/accounts/{account}/workflows/{definition}/versions",
            post(create_version).get(list_versions),
        )
        .route(
            "/v1/accounts/{account}/workflows/{definition}/instances",
            get(list_instances),
        )
        .route(
            "/v1/accounts/{account}/workflows/{definition}/instances/{instance}/steps",
            get(list_steps),
        )
        .route("/v1/operator/workflows", get(inspect_pool))
        .route("/v1/operator/workflows/reconcile", post(reconcile))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameBody {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionBody {
    deployment_id: DeploymentId,
    class_name: String,
}

async fn create_definition(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let account = match parse(&account) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let body: NameBody = match read_json(request, 16 * 1024).await {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let result = tokio::task::spawn_blocking(move || {
        let _admission = api.storage.reserve_mutation(64 * 1024)?;
        WorkflowRepository::new(api.storage.db()).create_definition(account, &body.name, now_ms())
    })
    .await;
    response(result, id, StatusCode::CREATED)
}

async fn list_definitions(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let account = match parse(&account) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let (after, limit) = match page::<WorkflowId>(&request) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            WorkflowRepository::new(api.storage.db()).definitions(account, after, limit)
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn inspect_definition(
    State(state): State<HttpState>,
    Path((account, definition)): Path<(String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(tokio::task::spawn_blocking(move || {
        let repo = WorkflowRepository::new(api.storage.db());
        Ok(serde_json::json!({"definition":repo.definition(account,definition)?,"referrerCount":repo.referrer_count(account,definition)?}))
    }).await,id,StatusCode::OK)
}

async fn rename_definition(
    State(state): State<HttpState>,
    Path((account, definition)): Path<(String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let body: NameBody = match read_json(request, 16 * 1024).await {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            let _admission = api.storage.reserve_mutation(64 * 1024)?;
            WorkflowRepository::new(api.storage.db()).rename(
                account,
                definition,
                &body.name,
                now_ms(),
            )
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn delete_definition(
    State(state): State<HttpState>,
    Path((account, definition)): Path<(String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            let _admission = api.storage.reserve_mutation(64 * 1024)?;
            WorkflowRepository::new(api.storage.db()).delete(account, definition, now_ms())
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn create_version(
    State(state): State<HttpState>,
    Path((account, definition)): Path<(String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let body: VersionBody = match read_json(request, 16 * 1024).await {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    match api
        .create_version(account, definition, body.deployment_id, body.class_name)
        .await
    {
        Ok(version) => {
            let status = if version.state == DeploymentState::Validating {
                StatusCode::ACCEPTED
            } else {
                StatusCode::CREATED
            };
            (status, Json(version)).into_response()
        }
        Err(error) => failure(&error, id),
    }
}

async fn list_versions(
    State(state): State<HttpState>,
    Path((account, definition)): Path<(String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let (after, limit) = match page::<i64>(&request) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            WorkflowRepository::new(api.storage.db()).versions(
                account,
                definition,
                after.unwrap_or(0),
                limit,
            )
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn list_instances(
    State(state): State<HttpState>,
    Path((account, definition)): Path<(String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let (after, limit) = match page::<WorkflowInstanceId>(&request) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            WorkflowRepository::new(api.storage.db()).definition(account, definition)?;
            api.scheduler
                .inspect_workflow_instances(account, definition, after, limit, now_ms())
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn list_steps(
    State(state): State<HttpState>,
    Path((account, definition, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    let (account, definition) = match ids(&account, &definition) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let instance: WorkflowInstanceId = match parse(&instance) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    let (after, limit) = match page::<u32>(&request) {
        Ok(value) => value,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            open_compute_workers::WorkflowController::new(
                &api.storage,
                &api.scheduler,
                &api.limits,
            )
            .inspect(account, definition, instance, now_ms())?;
            api.scheduler.workflow_steps(instance, after, limit)
        })
        .await,
        id,
        StatusCode::OK,
    )
}

async fn inspect_pool(State(state): State<HttpState>, request: Request) -> Response {
    let id = request_id(&request);
    let api = match authorized(&state, &request) {
        Ok(api) => api,
        Err(error) => return failure(&error, id),
    };
    response(
        tokio::task::spawn_blocking(move || {
            Ok(PoolInspection {
                scheduler: api.scheduler.inspect_workflows(now_ms())?,
                operations: WorkflowRepository::new(api.storage.db()).inspect_operations()?,
            })
        })
        .await,
        id,
        StatusCode::OK,
    )
}

#[derive(Serialize)]
struct PoolInspection {
    #[serde(flatten)]
    scheduler: open_compute_storage::scheduler::WorkflowInspection,
    operations: open_compute_storage::WorkflowOperationInspection,
}

async fn reconcile(State(state): State<HttpState>, request: Request) -> Response {
    let id = request_id(&request);
    if let Err(error) = authorized(&state, &request) {
        return failure(&error, id);
    }
    let Some(scheduler) = state.scheduler().cloned() else {
        return failure(&unavailable(), id);
    };
    let versions = scheduler.reconcile_workflow_versions(32).await;
    if let Err(error) = versions {
        return failure(&error, id);
    }
    let result = tokio::task::spawn_blocking(move || scheduler.repair_workflows(32)).await;
    response(result, id, StatusCode::OK)
}

fn authorized(
    state: &HttpState,
    request: &Request,
) -> Result<Arc<WorkflowApiState>, PlatformError> {
    if !authorize(state, request) {
        return Err(error(ErrorCode::AdminAuthRequired));
    }
    state.workflow_api().cloned().ok_or_else(unavailable)
}

fn parse<T: std::str::FromStr>(value: &str) -> Result<T, PlatformError> {
    value.parse().map_err(|_| error(ErrorCode::ConfigInvalid))
}

fn ids(account: &str, definition: &str) -> Result<(AccountId, WorkflowId), PlatformError> {
    Ok((parse(account)?, parse(definition)?))
}

fn page<T: std::str::FromStr>(request: &Request) -> Result<(Option<T>, u32), PlatformError> {
    let mut after = None;
    let mut limit = None;
    for pair in request
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|part| !part.is_empty())
    {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| error(ErrorCode::ConfigInvalid))?;
        match key {
            "after" if after.is_none() => after = Some(parse(value)?),
            "limit" if limit.is_none() => limit = Some(parse(value)?),
            _ => return Err(error(ErrorCode::ConfigInvalid)),
        }
    }
    let limit = limit.unwrap_or(100);
    if limit == 0 || limit > 1000 {
        return Err(error(ErrorCode::LimitInvalid));
    }
    Ok((after, limit))
}

async fn read_json<T: for<'de> Deserialize<'de>>(
    request: Request,
    limit: usize,
) -> Result<T, PlatformError> {
    let bytes = tokio::time::timeout(
        Duration::from_secs(30),
        to_bytes(request.into_body(), limit),
    )
    .await
    .map_err(|_| error(ErrorCode::LimitInvalid))?
    .map_err(|_| error(ErrorCode::LimitInvalid))?;
    serde_json::from_slice(&bytes).map_err(|_| error(ErrorCode::ConfigInvalid))
}

fn response<T: Serialize>(
    result: Result<Result<T, PlatformError>, tokio::task::JoinError>,
    id: RequestId,
    status: StatusCode,
) -> Response {
    match result {
        Ok(Ok(value)) => (status, Json(value)).into_response(),
        Ok(Err(error)) => failure(&error, id),
        Err(_) => failure(&unavailable(), id),
    }
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}
fn now_ms() -> i64 {
    SystemSchedulerClock.wall_time_ms()
}
fn unavailable() -> PlatformError {
    error(ErrorCode::WorkflowRuntimeUnavailable)
}
fn error(code: ErrorCode) -> PlatformError {
    PlatformError::new(code, "Workflow operation failed")
}

fn failure(error: &PlatformError, id: RequestId) -> Response {
    let status = match error.code() {
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::WorkflowNotFound | ErrorCode::WorkflowInstanceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkflowReferenced
        | ErrorCode::WorkflowNameConflict
        | ErrorCode::WorkflowNotReady
        | ErrorCode::WorkflowVersionNotReady
        | ErrorCode::WorkflowInstanceStateConflict
        | ErrorCode::WorkflowInstanceBusy
        | ErrorCode::WorkflowInstanceCleanupPending
        | ErrorCode::WorkflowRunStale => StatusCode::CONFLICT,
        ErrorCode::ConfigInvalid
        | ErrorCode::LimitInvalid
        | ErrorCode::WorkflowMethodUnsupported
        | ErrorCode::WorkflowCapabilityMismatch
        | ErrorCode::WorkflowEventTypeInvalid => StatusCode::BAD_REQUEST,
        ErrorCode::WorkflowPayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::WorkflowSerializationUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::QuotaExceeded
        | ErrorCode::WorkflowStateQuotaExceeded
        | ErrorCode::WorkflowEventQueueFull => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let mut response = (
        status,
        Json(serde_json::json!({"code":error.code().as_str(),"requestId":id})),
    )
        .into_response();
    response
        .extensions_mut()
        .insert(ProductErrorCode(error.code()));
    response
}

#[cfg(test)]
#[path = "workflow_http_tests.rs"]
pub(crate) mod tests;
