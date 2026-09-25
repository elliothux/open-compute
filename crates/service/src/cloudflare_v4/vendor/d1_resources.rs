//! open-compute D1 resource operations absent from the official public API.

use super::*;
use axum::routing::{get, patch};
use open_compute_storage::D1SnapshotRepository;
use open_compute_workers::{D1ResourceDriver, ResourceController};
use serde::{Deserialize, Serialize};

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/open-compute/d1/databases/{database_id}/name",
            patch(rename),
        )
        .route(
            "/accounts/{account_id}/open-compute/d1/databases/{database_id}/time-travel/checkpoints",
            get(checkpoints),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameRequest {
    name: String,
}

#[derive(Serialize)]
struct RenameResult {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct CheckpointResult {
    checkpoints_ms: Vec<i64>,
}

async fn checkpoints(
    State(state): State<HttpState>,
    Path((account_public, database_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match request_context(&request) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = context.require(V4Permission::Read) {
        return error_response(error, context.request_id());
    }
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let (account, database) = match resolve_resource(
        &state,
        &account_public,
        &database_public,
        V4ResourceKind::D1Database,
        BindingKind::D1Database,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match D1SnapshotRepository::new(api.storage().db()).checkpoint_times(account, database) {
        Ok(checkpoints_ms) => success_response(context, CheckpointResult { checkpoints_ms }),
        Err(error) => platform_error(&error, context),
    }
}

async fn rename(
    State(state): State<HttpState>,
    Path((account_public, database_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match request_context(&request) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = context.require(V4Permission::ProductWrite) {
        return error_response(error, context.request_id());
    }
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let (account, database) = match resolve_resource(
        &state,
        &account_public,
        &database_public,
        V4ResourceKind::D1Database,
        BindingKind::D1Database,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body =
        match crate::cloudflare_v4::storage::json::<RenameRequest>(request, context.request_id())
            .await
        {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let driver = D1ResourceDriver::new(api.storage(), api.config().database_quota_bytes);
    match ResourceController::new(api.storage(), api.pins().clone(), driver).rename(
        account,
        database,
        &body.name,
        context.request_id(),
        open_compute_core::wall_time_ms(),
    ) {
        Ok(record) => success_response(
            context,
            RenameResult {
                id: database_public,
                name: record.name,
            },
        ),
        Err(error) => platform_error(&error, context),
    }
}
