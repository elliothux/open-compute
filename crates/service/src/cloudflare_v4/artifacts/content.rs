//! Cloudflare Artifacts object, file, raw, and commit-log reads.

use super::{parse_query, prepare};
use crate::http::HttpState;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use open_compute_artifacts::GitObjectKind;
use serde::Deserialize;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/log",
            get(read_log),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/blob/{oid}",
            get(read_blob),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/commit/{oid}",
            get(read_commit),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/tree/{oid}",
            get(read_tree),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/file",
            get(read_file),
        )
        .route(
            "/accounts/{account_id}/artifacts/namespaces/{namespace}/repos/{repository}/raw/{reference}/{*path}",
            get(read_raw),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    #[serde(rename = "ref")]
    reference: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogQuery {
    #[serde(rename = "ref")]
    reference: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn read_blob(
    State(state): State<HttpState>,
    Path(path): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    read_object(state, path, request, GitObjectKind::Blob, true).await
}

async fn read_commit(
    State(state): State<HttpState>,
    Path(path): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    read_object(state, path, request, GitObjectKind::Commit, false).await
}

async fn read_tree(
    State(state): State<HttpState>,
    Path(path): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    read_object(state, path, request, GitObjectKind::Tree, false).await
}

async fn read_object(
    state: HttpState,
    (account_id, namespace, repository, oid): (String, String, String, String),
    request: Request,
    expected: GitObjectKind,
    raw: bool,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        super::V4Permission::Read,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match api.read_object(account, &namespace, &repository, &oid) {
        Ok(object) if object.kind == expected && raw => (
            [(header::CONTENT_TYPE, "application/octet-stream")],
            object.bytes,
        )
            .into_response(),
        Ok(object) if object.kind == expected => super::success_response(
            context,
            serde_json::json!({
                "id": object.oid,
                "type": object.kind,
                "content": String::from_utf8_lossy(&object.bytes),
            }),
        ),
        Ok(_) => super::artifact_error_response(
            super::V4OfficialError::ArtifactNotFound,
            context.request_id(),
        ),
        Err(error) => super::platform_error_response(&error, context.request_id()),
    }
}

async fn read_file(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        super::V4Permission::Read,
        true,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(query) = parse_query::<FileQuery>(&request) else {
        return super::invalid_response(context.request_id());
    };
    raw_file(
        api,
        account,
        (&namespace, &repository, &query.reference, &query.path),
        false,
        context.request_id(),
    )
}

async fn read_raw(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository, reference, path)): Path<(
        String,
        String,
        String,
        String,
        String,
    )>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        super::V4Permission::Read,
        false,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    raw_file(
        api,
        account,
        (&namespace, &repository, &reference, &path),
        true,
        context.request_id(),
    )
}

async fn read_log(
    State(state): State<HttpState>,
    Path((account_id, namespace, repository)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match prepare(
        &state,
        &account_id,
        &request,
        super::V4Permission::Read,
        true,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(query) = parse_query::<LogQuery>(&request) else {
        return super::invalid_response(context.request_id());
    };
    match api.commit_log(
        account,
        &namespace,
        &repository,
        query.reference.as_deref(),
        query.offset.unwrap_or(0),
        query.limit.unwrap_or(50),
    ) {
        Ok(commits) => super::success_response(
            context,
            commits
                .into_iter()
                .map(|commit| {
                    serde_json::json!({
                        "id": commit.oid,
                        "content": String::from_utf8_lossy(&commit.bytes),
                    })
                })
                .collect::<Vec<_>>(),
        ),
        Err(error) => super::platform_error_response(&error, context.request_id()),
    }
}

fn raw_file(
    api: &crate::artifact_api::ArtifactApiState,
    account: open_compute_core::AccountId,
    location: (&str, &str, &str, &str),
    sniff: bool,
    request_id: open_compute_core::RequestId,
) -> Response {
    let (namespace, repository, reference, path) = location;
    match api.read_file(account, namespace, repository, reference, path) {
        Ok(object) => Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                if sniff {
                    content_type(path, &object.bytes)
                } else {
                    "application/octet-stream"
                },
            )
            .body(axum::body::Body::from(object.bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(error) => super::platform_error_response(&error, request_id),
    }
}

fn content_type(path: &str, bytes: &[u8]) -> &'static str {
    if bytes.iter().take(512).any(|byte| *byte == 0) {
        return "application/octet-stream";
    }
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
