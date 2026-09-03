//! Read-only account subdomain prerequisite used by fixed Wrangler Workflow deploys.

use super::handlers::authorize;
use crate::cloudflare_v4::{V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::get;
use serde::Serialize;

#[derive(Serialize)]
struct AccountSubdomain {
    subdomain: String,
}

pub(super) fn router() -> Router<HttpState> {
    Router::new().route(
        "/accounts/{account}/workers/subdomain",
        get(get_account_subdomain),
    )
}

async fn get_account_subdomain(
    State(state): State<HttpState>,
    Path(public_account): Path<String>,
    request: Request,
) -> Response {
    let context = match authorize(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let result = (|| {
        let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
        authority.resolve(&public_account)?;
        Ok(AccountSubdomain {
            subdomain: authority.workers_dev_prerequisite_label(),
        })
    })();
    match result {
        Ok(value) => success_response(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}
