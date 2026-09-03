//! D1 export, import, and time-travel session boundaries.

use super::database;
use crate::cloudflare_v4::storage::{require_no_query, require_query_fields};
use crate::cloudflare_v4::{V4Error, error_response};
use crate::http::HttpState;
use axum::extract::{Path, Request, State};
use axum::response::Response;

pub(super) async fn export(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    unsupported(state, account_id, database_id, request, false, &[])
}

pub(super) async fn import(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    unsupported(state, account_id, database_id, request, true, &[])
}

pub(super) async fn restore(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    unsupported(
        state,
        account_id,
        database_id,
        request,
        true,
        &["bookmark", "timestamp"],
    )
}

fn unsupported(
    state: HttpState,
    account_id: String,
    database_id: String,
    request: Request,
    write: bool,
    query_fields: &[&str],
) -> Response {
    let (context, _, _) = match database(&state, &request, &account_id, &database_id, write) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = if query_fields.is_empty() {
        require_no_query(&request)
    } else {
        require_query_fields(&request, query_fields)
    };
    if let Err(error) = query {
        return error_response(error, context.request_id());
    }
    error_response(V4Error::Unsupported, context.request_id())
}
