//! Official Cloudflare Workflows management adapter.

mod cursor;
mod definitions;
mod instances;
mod value;

use super::storage::{account, context};
use super::{V4Error, V4OfficialError, V4Permission, V4RequestContext, error_response};
use crate::http::HttpState;
use crate::workflow_http::WorkflowApiState;
use axum::Router;
use axum::extract::Request;
use axum::response::Response;
use axum::routing::{get, post};
use open_compute_core::{AccountId, WorkflowId};
use open_compute_storage::scheduler::WorkflowState;
use open_compute_storage::{CatalogDirection, CatalogSort, WorkflowDefinition, WorkflowRepository};
use std::sync::Arc;

const CURSOR_LIFETIME_MS: i64 = 15 * 60 * 1_000;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/workflows",
            get(definitions::list),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}",
            get(definitions::get)
                .put(definitions::put)
                .delete(definitions::delete),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/versions",
            get(definitions::list_versions),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/versions/{version_id}",
            get(definitions::get_version),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/instances",
            get(instances::list).post(instances::create),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/instances/batch",
            post(instances::batch),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/instances/{instance_id}",
            get(instances::get),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/instances/{instance_id}/status",
            axum::routing::patch(instances::status),
        )
        .route(
            "/accounts/{account_id}/workflows/{workflow_name}/instances/{instance_id}/events/{event_type}",
            post(instances::event),
        )
}

fn authenticated(
    state: &HttpState,
    request: &Request,
    permission: V4Permission,
    public_account: &str,
) -> Result<(V4RequestContext, AccountId, Arc<WorkflowApiState>), Response> {
    let context = context(request, permission)?;
    let account = account(state, public_account)
        .map_err(|error| error_response(error, context.request_id()))?;
    let api = state
        .workflow_api()
        .cloned()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    Ok((context, account, api))
}

fn definition(
    api: &WorkflowApiState,
    account: AccountId,
    name: &str,
) -> Result<WorkflowDefinition, V4Error> {
    valid_name(name)?;
    let page = WorkflowRepository::new(api.storage().db())
        .definitions(
            account,
            Some(name),
            None,
            CatalogSort::Name,
            CatalogDirection::Asc,
            None,
            100,
        )
        .map_err(|error| V4Error::from(&error))?;
    page.items
        .into_iter()
        .find(|definition| definition.name == name)
        .ok_or(V4Error::Official(V4OfficialError::WorkflowNotFound))
}

fn valid_name(value: &str) -> Result<(), V4Error> {
    open_compute_core::workflow::validate_workflow_name(value).map_err(|_| V4Error::InvalidRequest)
}

fn valid_instance_id(value: &str) -> Result<(), V4Error> {
    open_compute_core::workflow::validate_workflow_instance_id(value)
        .map_err(|_| V4Error::InvalidRequest)
}

fn status_name(
    state: WorkflowState,
    rollback_requested: bool,
    pause_requested: bool,
) -> &'static str {
    if rollback_requested {
        return "rollingBack";
    }
    match state {
        WorkflowState::Queued => "queued",
        WorkflowState::Running if pause_requested => "waitingForPause",
        WorkflowState::Running => "running",
        WorkflowState::Waiting => "waiting",
        WorkflowState::Paused => "paused",
        WorkflowState::Complete => "complete",
        WorkflowState::Errored => "errored",
        WorkflowState::Terminated => "terminated",
    }
}

fn workflow_id(id: WorkflowId) -> String {
    id.to_string()
}

#[cfg(test)]
#[path = "../workflows_tests.rs"]
mod tests;
