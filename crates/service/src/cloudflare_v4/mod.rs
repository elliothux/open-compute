//! Cloudflare v4-compatible control-plane boundary.

pub(crate) mod accounts;
mod ai_search;
mod d1;
mod d1_transfer;
mod kv;
mod queues;
mod r2;
mod storage;
mod vectorize;
mod vendor;
mod wire;
mod workflows;

pub(crate) use accounts::V4ResourceKind;
pub(crate) use wire::{
    V4Error, V4OfficialError, V4Permission, V4RequestContext, V4ResultInfo, V4Role, error_response,
    paginated_response, request_context, result_info_response, success_response,
};
/// Official Cloudflare storage routes implemented by the local product authorities.
pub(crate) fn storage_router() -> Router<HttpState> {
    kv::router()
        .merge(d1::router())
        .merge(d1_transfer::router())
        .merge(r2::router())
        .merge(vectorize::router())
        .merge(ai_search::router())
        .merge(queues::router())
        .merge(workflows::router())
}

use crate::http::HttpState;
use axum::Router;
use axum::middleware;

/// Compose the common/account/vendor v4 surface with independently owned product routes.
pub(crate) fn router(
    auth_state: HttpState,
    additional_routes: Router<HttpState>,
) -> Router<HttpState> {
    let authenticated = Router::new()
        .merge(accounts::router())
        .merge(vendor::router())
        .merge(additional_routes)
        .fallback(wire::not_found)
        .layer(middleware::from_fn_with_state(
            auth_state,
            wire::authentication_boundary,
        ));
    Router::new()
        .merge(crate::workers_http::v4::signed_router())
        .merge(d1_transfer::signed_router())
        .merge(authenticated)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
