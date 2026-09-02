//! Worker management routes and authenticated account discovery.

use super::*;
use axum::Router;
use axum::routing::{delete, get, post, put};

/// Router containing the stable P0.2 management surface.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route("/v1/account", get(default_account))
        .route(
            "/v1/accounts/{account_id}/workers",
            post(create_worker).get(list_workers),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}",
            get(get_worker).delete(delete_worker),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/versions",
            post(create_version).get(list_versions),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/version-uploads",
            post(create_version_upload),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/version-uploads/{upload_id}",
            get(get_version_upload).delete(abort_version_upload),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/version-uploads/{upload_id}/objects/{sha256}",
            put(put_version_upload_object),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/version-uploads/{upload_id}/finalize",
            post(finalize_version_upload),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/versions/{version_id}",
            get(get_version).delete(delete_version),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/promotions",
            post(promote),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/rollbacks",
            post(rollback),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/routes",
            post(create_route).get(list_routes),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/routes/{route_id}",
            delete(delete_route),
        )
}

async fn default_account(State(state): State<HttpState>, request: Request) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    result_response(
        Ok(serde_json::json!({ "accountId": api.storage.identity().default_account_id })),
        request_id,
    )
}
