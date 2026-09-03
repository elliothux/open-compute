//! Cloudflare v4-compatible control-plane boundary.

pub(crate) mod accounts;
mod d1;
mod kv;
mod r2;
mod storage;
mod vendor;
mod wire;

pub(crate) use accounts::V4ResourceKind;
pub(crate) use wire::{
    V4Error, V4OfficialError, V4Permission, V4RequestContext, V4ResultInfo, V4Role, error_response,
    paginated_response, request_context, success_response,
};
/// Official Cloudflare storage routes implemented by the local product authorities.
pub(crate) fn storage_router() -> Router<HttpState> {
    kv::router().merge(d1::router()).merge(r2::router())
}

use crate::http::HttpState;
use axum::Router;
use axum::middleware;

/// Compose the common/account/vendor v4 surface with independently owned product routes.
pub(crate) fn router(
    auth_state: HttpState,
    additional_routes: Router<HttpState>,
) -> Router<HttpState> {
    Router::new()
        .merge(accounts::router())
        .merge(vendor::router())
        .merge(additional_routes)
        .fallback(wire::not_found)
        .layer(middleware::from_fn_with_state(
            auth_state,
            wire::authentication_boundary,
        ))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
