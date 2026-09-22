//! Worker local and public origin control-plane routes.

use super::{bodyless_context, platform_error, read_context, resolve_account};
use crate::cloudflare_v4::{V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::get;
use open_compute_core::{AccountId, ErrorCode, PlatformError};
use open_compute_storage::{RouteRecord, WorkerOriginExposure, WorkerOwnership, WorkerRepository};
use serde::{Deserialize, Serialize};

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/open-compute/workers/{script_name}/endpoints",
            get(worker_endpoints),
        )
        .route(
            "/accounts/{account_id}/open-compute/workers/{script_name}/public-origin",
            get(worker_public_origin)
                .put(set_worker_public_origin)
                .delete(delete_worker_public_origin),
        )
}

#[derive(Serialize)]
struct PublicOriginBinding {
    name: String,
    url: String,
}

pub(super) async fn worker_public_origin(
    State(state): State<HttpState>,
    Path((account, script_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let workers = WorkerRepository::new(storage.db());
    let worker_id = match tenant_worker_id(workers, account, &script_name) {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    match workers.list_routes(account, worker_id) {
        Ok(routes) => match routes
            .into_iter()
            .find(|route| route.exposure == WorkerOriginExposure::Public)
            .map(|route| public_origin_binding(&route))
            .transpose()
        {
            Ok(binding) => success_response(context, binding),
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => platform_error(&error, context),
    }
}

fn public_origin_binding(route: &RouteRecord) -> Result<PublicOriginBinding, V4Error> {
    let (name, _) = route
        .hostname_ascii
        .split_once('.')
        .ok_or(V4Error::Internal)?;
    Ok(PublicOriginBinding {
        name: name.to_owned(),
        url: format!("https://{}/", route.hostname_ascii),
    })
}

pub(super) async fn worker_endpoints(
    State(state): State<HttpState>,
    Path((account, script_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let workers = WorkerRepository::new(storage.db());
    let worker_id = match tenant_worker_id(workers, account, &script_name) {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    match workers.list_routes(account, worker_id) {
        Ok(routes) => {
            let result = routes
                .into_iter()
                .filter_map(|route| project_worker_endpoint(route, &state))
                .collect::<Result<Vec<_>, V4Error>>();
            match result {
                Ok(result) => success_response(context, result),
                Err(error) => error_response(error, context.request_id()),
            }
        }
        Err(error) => platform_error(&error, context),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicOriginInput {
    name: String,
}

pub(super) async fn set_worker_public_origin(
    State(state): State<HttpState>,
    Path((account, script_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match read_context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let input: PublicOriginInput =
        match crate::cloudflare_v4::storage::json_with_limit(request, context.request_id(), 512)
            .await
        {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let workers = WorkerRepository::new(storage.db());
    let worker_id = match tenant_worker_id(workers, account, &script_name) {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    if !state.public_gateway_serving() {
        return error_response(V4Error::Unavailable, context.request_id());
    }
    let now = crate::cloudflare_v4::storage::now_ms();
    match workers.set_public_origin(
        account,
        worker_id,
        Some(&input.name),
        context.request_id(),
        now,
    ) {
        Ok(Some(route)) => match public_origin_endpoint(route) {
            Ok(endpoint) => success_response(context, endpoint),
            Err(error) => error_response(error, context.request_id()),
        },
        Ok(None) => error_response(V4Error::Internal, context.request_id()),
        Err(error) => platform_error(&error, context),
    }
}

pub(super) async fn delete_worker_public_origin(
    State(state): State<HttpState>,
    Path((account, script_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match bodyless_context(request, V4Permission::ProductWrite).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let account = match resolve_account(&state, &account) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(storage) = state.platform_storage() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let workers = WorkerRepository::new(storage.db());
    let worker_id = match tenant_worker_id(workers, account, &script_name) {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    match workers.set_public_origin(
        account,
        worker_id,
        None,
        context.request_id(),
        crate::cloudflare_v4::storage::now_ms(),
    ) {
        Ok(_) => success_response(context, serde_json::Value::Null),
        Err(error) => platform_error(&error, context),
    }
}

fn tenant_worker_id(
    workers: WorkerRepository<'_>,
    account: AccountId,
    name: &str,
) -> Result<open_compute_core::WorkerId, PlatformError> {
    workers
        .list_workers(account)?
        .into_iter()
        .find(|worker| worker.name == name && worker.ownership == WorkerOwnership::Tenant)
        .map(|worker| worker.id)
        .ok_or_else(|| PlatformError::new(ErrorCode::WorkerNotFound, "Worker not found"))
}

fn public_origin_endpoint(route: RouteRecord) -> Result<WorkerEndpoint, V4Error> {
    Ok(WorkerEndpoint {
        id: route.id,
        kind: WorkerEndpointKind::PublicOrigin,
        url: format!("https://{}/", route.hostname_ascii),
        scope: WorkerEndpointScope::PublicNetwork,
        created_on: crate::cloudflare_v4::iso_timestamp(route.created_at_ms)?,
    })
}

pub(super) fn project_worker_endpoint(
    route: RouteRecord,
    state: &HttpState,
) -> Option<Result<WorkerEndpoint, V4Error>> {
    match route.exposure {
        WorkerOriginExposure::Local => {
            let port = state.local_origin_port()?;
            Some(
                crate::cloudflare_v4::iso_timestamp(route.created_at_ms).map(|created_on| {
                    WorkerEndpoint {
                        id: route.id,
                        kind: WorkerEndpointKind::LocalOrigin,
                        url: format!("http://{}:{port}/", route.hostname_ascii),
                        scope: WorkerEndpointScope::LocalMachine,
                        created_on,
                    }
                }),
            )
        }
        WorkerOriginExposure::Public => state
            .public_gateway_serving()
            .then(|| public_origin_endpoint(route)),
    }
}

#[derive(Serialize)]
pub(super) struct WorkerEndpoint {
    id: String,
    kind: WorkerEndpointKind,
    url: String,
    scope: WorkerEndpointScope,
    created_on: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkerEndpointKind {
    LocalOrigin,
    PublicOrigin,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkerEndpointScope {
    LocalMachine,
    PublicNetwork,
}
