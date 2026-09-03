//! Scoped raw SQL transfer URLs authenticated by persisted capability fingerprints.

use super::*;
use axum::body::Body;
use axum::routing::put;
use md5::Digest as _;
use open_compute_storage::D1_MAX_TRANSFER_SQL_BYTES;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/transfer/{session_id}/upload",
            put(upload_import),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/transfer/{session_id}/download",
            get(download_export),
        )
}

async fn upload_import(
    State(state): State<HttpState>,
    Path((account_id, database_id, session_id)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Ok(Some(token)) = one_query(request.uri().query(), "token", true) else {
        return raw_error(StatusCode::UNAUTHORIZED, request_id);
    };
    let (account, database) = match resolve_database(&state, &account_id, &database_id) {
        Ok(value) => value,
        Err(error) => return raw_error(error_status(error), request_id),
    };
    let Some(api) = state.d1_api() else {
        return raw_error(StatusCode::SERVICE_UNAVAILABLE, request_id);
    };
    if let Err(error) = api
        .backend()
        .authorize_import_upload(account, database, session_id.clone(), token.clone())
        .await
    {
        return raw_error(platform_status(&error), request_id);
    }
    let bytes = match to_bytes(
        request.into_body(),
        D1_MAX_TRANSFER_SQL_BYTES.saturating_add(1),
    )
    .await
    {
        Ok(value) if !value.is_empty() && value.len() <= D1_MAX_TRANSFER_SQL_BYTES => value,
        _ => return raw_error(StatusCode::PAYLOAD_TOO_LARGE, request_id),
    };
    match api
        .backend()
        .upload_import(account, database, session_id, token, bytes.to_vec())
        .await
    {
        Ok(_) => {
            let mut response = StatusCode::OK.into_response();
            response.headers_mut().insert(
                header::ETAG,
                HeaderValue::from_str(&format!("\"{}\"", hex::encode(md5::Md5::digest(&bytes))))
                    .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
            );
            attach_request_id(&mut response, request_id);
            response
        }
        Err(error) => raw_error(platform_status(&error), request_id),
    }
}

async fn download_export(
    State(state): State<HttpState>,
    Path((account_id, database_id, session_id)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Ok(Some(token)) = one_query(request.uri().query(), "token", true) else {
        return raw_error(StatusCode::UNAUTHORIZED, request_id);
    };
    let (account, database) = match resolve_database(&state, &account_id, &database_id) {
        Ok(value) => value,
        Err(error) => return raw_error(error_status(error), request_id),
    };
    let Some(api) = state.d1_api() else {
        return raw_error(StatusCode::SERVICE_UNAVAILABLE, request_id);
    };
    match api
        .backend()
        .download_export(account, database, session_id, token)
        .await
    {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/sql; charset=utf-8"),
            );
            attach_request_id(&mut response, request_id);
            response
        }
        Err(error) => raw_error(platform_status(&error), request_id),
    }
}
