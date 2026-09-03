//! Cloudflare D1 export, import, and time-travel adapters.

use super::{
    V4Error, V4Permission, V4RequestContext, V4ResourceKind, error_response, request_context,
    success_response,
};
use crate::d1_backend::{D1TimeTravelTarget, D1TransferGrant};
use crate::http::{HttpState, REQUEST_ID_HEADER};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header, uri::Authority};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_core::{AccountId, BindingKind, ErrorCode, PlatformError, RequestId, ResourceId};
use open_compute_storage::{D1ExportOptions, D1TransferState, ResourceRepository};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use url::form_urlencoded;

const MAX_JSON_BODY: usize = 4096;

#[path = "d1_signed.rs"]
mod signed;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/export",
            post(export_database),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/import",
            post(import_database),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/time_travel/bookmark",
            get(time_travel_bookmark),
        )
        .route(
            "/accounts/{account_id}/d1/database/{database_id}/time_travel/restore",
            post(time_travel_restore),
        )
}

pub(super) fn signed_router() -> Router<HttpState> {
    signed::router()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportRequest {
    output_format: ExportFormat,
    #[serde(default)]
    current_bookmark: Option<String>,
    #[serde(default)]
    dump_options: ExportDumpOptions,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ExportFormat {
    Polling,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportDumpOptions {
    #[serde(default)]
    no_schema: bool,
    #[serde(default)]
    no_data: bool,
    #[serde(default)]
    tables: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
enum ImportRequest {
    Init { etag: String },
    Ingest { filename: String, etag: String },
    Poll { current_bookmark: String },
}

async fn export_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authenticated_context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let host = match request_host(request.headers()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match json_body::<ExportRequest>(request, context).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let ExportFormat::Polling = body.output_format;
    let (account, database) = match resolve_database(&state, &account_id, &database_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let grant = if let Some(session) = body.current_bookmark {
        match api
            .backend()
            .export_transfer(account, database, session)
            .await
        {
            Ok(value) => value,
            Err(error) => return platform_error(&error, context),
        }
    } else {
        let tables = match validate_tables(body.dump_options.tables) {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        };
        match api
            .backend()
            .begin_export(
                account,
                database,
                D1ExportOptions {
                    no_schema: body.dump_options.no_schema,
                    no_data: body.dump_options.no_data,
                    tables,
                },
            )
            .await
        {
            Ok(value) => value,
            Err(error) => return platform_error(&error, context),
        }
    };
    export_response(context, &host, &account_id, &database_id, grant)
}

async fn import_database(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authenticated_context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let host = match request_host(request.headers()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match json_body::<ImportRequest>(request, context).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (account, database) = match resolve_database(&state, &account_id, &database_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let outcome = match body {
        ImportRequest::Init { etag } => {
            let etag = match parse_md5(&etag) {
                Ok(value) => value,
                Err(error) => return error_response(error, context.request_id()),
            };
            let grant = match api.backend().begin_import(account, database, etag).await {
                Ok(value) => value,
                Err(error) => return platform_error(&error, context),
            };
            if matches!(
                grant.transfer.state,
                D1TransferState::Uploaded | D1TransferState::Ingesting
            ) && let Err(error) = api
                .backend()
                .ingest_import(account, database, grant.transfer.id.clone())
                .await
            {
                return platform_error(&error, context);
            }
            import_response(
                context,
                &host,
                &account_id,
                &database_id,
                grant,
                api.backend(),
                account,
                database,
            )
            .await
        }
        ImportRequest::Ingest { filename, etag } => {
            let etag = match parse_md5(&etag) {
                Ok(value) => value,
                Err(error) => return error_response(error, context.request_id()),
            };
            let grant = match api.backend().begin_import(account, database, etag).await {
                Ok(value) if value.transfer.filename == filename => value,
                Ok(_) => return error_response(V4Error::Conflict, context.request_id()),
                Err(error) => return platform_error(&error, context),
            };
            if let Err(error) = api
                .backend()
                .ingest_import(account, database, grant.transfer.id.clone())
                .await
            {
                return platform_error(&error, context);
            }
            import_response(
                context,
                &host,
                &account_id,
                &database_id,
                grant,
                api.backend(),
                account,
                database,
            )
            .await
        }
        ImportRequest::Poll { current_bookmark } => {
            let transfer = match api
                .backend()
                .transfer(account, database, current_bookmark)
                .await
            {
                Ok(value) => value,
                Err(error) => return platform_error(&error, context),
            };
            let grant = D1TransferGrant {
                transfer,
                token: String::new(),
            };
            import_response(
                context,
                &host,
                &account_id,
                &database_id,
                grant,
                api.backend(),
                account,
                database,
            )
            .await
        }
    };
    outcome
}

async fn time_travel_bookmark(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authenticated_context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let timestamp = match one_query(request.uri().query(), "timestamp", false) {
        Ok(value) => match value.as_deref().map(parse_timestamp_ms).transpose() {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        },
        Err(error) => return error_response(error, context.request_id()),
    };
    let (account, database) = match resolve_database(&state, &account_id, &database_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    match api
        .backend()
        .time_travel_bookmark(account, database, timestamp)
        .await
    {
        Ok(bookmark) => success_response(context, BookmarkResponse { bookmark }),
        Err(error) => platform_error(&error, context),
    }
}

async fn time_travel_restore(
    State(state): State<HttpState>,
    Path((account_id, database_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match authenticated_context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match restore_query(request.uri().query()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if let Err(response) = bodyless(request, context).await {
        return response;
    }
    let (account, database) = match resolve_database(&state, &account_id, &database_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.d1_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let previous = match api
        .backend()
        .time_travel_bookmark(account, database, None)
        .await
    {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    let bookmark = match api
        .backend()
        .time_travel_restore(account, database, query)
        .await
    {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    success_response(
        context,
        RestoreResponse {
            bookmark,
            message: "Database restored successfully.",
            previous_bookmark: previous,
        },
    )
}

#[derive(Serialize)]
struct ExportOperation {
    at_bookmark: String,
    messages: Vec<String>,
    result: ExportResult,
    status: &'static str,
    success: bool,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct ExportResult {
    filename: String,
    signed_url: String,
}

fn export_response(
    context: V4RequestContext,
    host: &Authority,
    account: &str,
    database: &str,
    grant: D1TransferGrant,
) -> Response {
    if grant.transfer.state != D1TransferState::Complete {
        return error_response(V4Error::Conflict, context.request_id());
    }
    let path = format!(
        "/accounts/{account}/d1/database/{database}/transfer/{}/download",
        grant.transfer.id
    );
    success_response(
        context,
        ExportOperation {
            at_bookmark: grant.transfer.id,
            messages: Vec::new(),
            result: ExportResult {
                filename: grant.transfer.filename,
                signed_url: signed_url(host, &path, &grant.token),
            },
            status: "complete",
            success: true,
            kind: "export",
        },
    )
}

#[derive(Serialize)]
struct ImportOperation {
    at_bookmark: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    messages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ImportResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    success: bool,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    upload_url: Option<String>,
}

#[derive(Serialize)]
struct ImportResult {
    final_bookmark: String,
    meta: ImportMeta,
    num_queries: u64,
}

#[derive(Serialize)]
struct ImportMeta {
    changed_db: bool,
    duration: f64,
    rows_read: u64,
    rows_written: u64,
    size_after: u64,
}

#[allow(clippy::too_many_arguments)]
async fn import_response(
    context: V4RequestContext,
    host: &Authority,
    account_public: &str,
    database_public: &str,
    grant: D1TransferGrant,
    backend: &crate::d1_backend::D1BindingService,
    account: AccountId,
    database: ResourceId,
) -> Response {
    let transfer = match backend
        .transfer(account, database, grant.transfer.id.clone())
        .await
    {
        Ok(value) => value,
        Err(error) => return platform_error(&error, context),
    };
    let (status, result, upload_url) = match transfer.state {
        D1TransferState::Uploading => {
            let path = format!(
                "/accounts/{account_public}/d1/database/{database_public}/transfer/{}/upload",
                transfer.id
            );
            (None, None, Some(signed_url(host, &path, &grant.token)))
        }
        D1TransferState::Complete => {
            let Some(result_version) = transfer.result_session_version else {
                return error_response(V4Error::Internal, context.request_id());
            };
            let bookmark = match backend
                .bookmark_at_version(account, database, result_version)
                .await
            {
                Ok(value) => value,
                Err(error) => return platform_error(&error, context),
            };
            let (
                Some(num_queries),
                Some(duration),
                Some(rows_read),
                Some(rows_written),
                Some(size_after),
            ) = (
                transfer.num_queries,
                transfer.duration_ms,
                transfer.rows_read,
                transfer.rows_written,
                transfer.result_size_after,
            )
            else {
                return error_response(V4Error::Internal, context.request_id());
            };
            (
                Some("complete"),
                Some(ImportResult {
                    final_bookmark: bookmark,
                    meta: ImportMeta {
                        changed_db: true,
                        duration,
                        rows_read,
                        rows_written,
                        size_after,
                    },
                    num_queries,
                }),
                None,
            )
        }
        D1TransferState::Uploaded | D1TransferState::Ingesting => (None, None, None),
        D1TransferState::Preparing | D1TransferState::Failed | D1TransferState::Expired => {
            return error_response(V4Error::Conflict, context.request_id());
        }
    };
    success_response(
        context,
        ImportOperation {
            at_bookmark: transfer.id,
            filename: Some(transfer.filename),
            messages: Vec::new(),
            result,
            status,
            success: true,
            kind: "import",
            upload_url,
        },
    )
}

#[derive(Serialize)]
struct BookmarkResponse {
    bookmark: String,
}

#[derive(Serialize)]
struct RestoreResponse {
    bookmark: String,
    message: &'static str,
    previous_bookmark: String,
}

fn authenticated_context(
    request: &Request,
    permission: V4Permission,
) -> Result<V4RequestContext, Response> {
    let context = request_context(request)?;
    context
        .require(permission)
        .map_err(|error| error_response(error, context.request_id()))?;
    Ok(context)
}

async fn json_body<T: for<'de> Deserialize<'de>>(
    request: Request,
    context: V4RequestContext,
) -> Result<T, Response> {
    match one_header(request.headers(), header::CONTENT_TYPE.as_str(), true) {
        Ok(Some(value))
            if value
                .split(';')
                .next()
                .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json")) => {}
        _ => {
            return Err(error_response(
                V4Error::InvalidRequest,
                context.request_id(),
            ));
        }
    }
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))
}

async fn bodyless(request: Request, context: V4RequestContext) -> Result<(), Response> {
    if request
        .headers()
        .get_all(header::CONTENT_TYPE)
        .iter()
        .count()
        != 0
    {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    let bytes = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    if !bytes.is_empty() {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    Ok(())
}

fn resolve_database(
    state: &HttpState,
    public_account: &str,
    public_database: &str,
) -> Result<(AccountId, ResourceId), V4Error> {
    let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
    let account = authority.resolve(public_account)?;
    let storage = state.platform_storage().ok_or(V4Error::Unavailable)?;
    let database = ResourceRepository::new(storage.db())
        .list(account, Some(BindingKind::D1Database))
        .map_err(|error| V4Error::from(&error))?
        .into_iter()
        .find(|resource| {
            authority.matches_public_resource_id(
                V4ResourceKind::D1Database,
                resource.id,
                public_database,
            )
        })
        .map(|resource| resource.id)
        .ok_or(V4Error::NotFound)?;
    Ok((account, database))
}

fn restore_query(query: Option<&str>) -> Result<D1TimeTravelTarget, V4Error> {
    let values = parse_query(query)?;
    match (
        values.get("bookmark"),
        values.get("timestamp"),
        values.len(),
    ) {
        (Some(bookmark), None, 1) if !bookmark.is_empty() => {
            Ok(D1TimeTravelTarget::Bookmark(bookmark.clone()))
        }
        (None, Some(timestamp), 1) => Ok(D1TimeTravelTarget::TimestampMs(parse_timestamp_ms(
            timestamp,
        )?)),
        _ => Err(V4Error::InvalidRequest),
    }
}

fn one_query(query: Option<&str>, name: &str, required: bool) -> Result<Option<String>, V4Error> {
    let values = parse_query(query)?;
    if values.len() > usize::from(values.contains_key(name)) {
        return Err(V4Error::InvalidRequest);
    }
    match values.get(name) {
        Some(value) if !value.is_empty() => Ok(Some(value.clone())),
        None if !required => Ok(None),
        _ => Err(V4Error::InvalidRequest),
    }
}

fn parse_query(query: Option<&str>) -> Result<BTreeMap<String, String>, V4Error> {
    let mut values = BTreeMap::new();
    for (name, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if values
            .insert(name.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(V4Error::InvalidRequest);
        }
    }
    Ok(values)
}

fn parse_timestamp_ms(value: &str) -> Result<i64, V4Error> {
    value
        .parse::<jiff::Timestamp>()
        .map_err(|_| V4Error::InvalidRequest)?
        .as_millisecond()
        .try_into()
        .map_err(|_| V4Error::InvalidRequest)
}

fn validate_tables(tables: Vec<String>) -> Result<BTreeSet<String>, V4Error> {
    let mut selected = BTreeSet::new();
    for table in tables {
        if table.is_empty() || table.len() > 255 || !selected.insert(table) {
            return Err(V4Error::InvalidRequest);
        }
    }
    Ok(selected)
}

fn parse_md5(value: &str) -> Result<[u8; 16], V4Error> {
    if value.len() != 32 {
        return Err(V4Error::InvalidRequest);
    }
    hex::decode(value)
        .map_err(|_| V4Error::InvalidRequest)?
        .try_into()
        .map_err(|_| V4Error::InvalidRequest)
}

fn request_host(headers: &HeaderMap) -> Result<Authority, V4Error> {
    one_header(headers, header::HOST.as_str(), true)?
        .ok_or(V4Error::InvalidRequest)?
        .parse()
        .map_err(|_| V4Error::InvalidRequest)
}

fn one_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    required: bool,
) -> Result<Option<&'a str>, V4Error> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(V4Error::InvalidRequest);
    }
    match first {
        Some(value) => value
            .to_str()
            .map(Some)
            .map_err(|_| V4Error::InvalidRequest),
        None if !required => Ok(None),
        None => Err(V4Error::InvalidRequest),
    }
}

fn signed_url(host: &Authority, path: &str, token: &str) -> String {
    let token: String = form_urlencoded::byte_serialize(token.as_bytes()).collect();
    format!("http://{host}/client/v4{path}?token={token}")
}

fn platform_error(error: &PlatformError, context: V4RequestContext) -> Response {
    error_response(platform_v4_error(error), context.request_id())
}

fn platform_v4_error(error: &PlatformError) -> V4Error {
    match error.code() {
        ErrorCode::ArtifactIntegrityError
        | ErrorCode::D1DatabaseCorrupt
        | ErrorCode::D1IdentityMismatch => V4Error::IntegrityFailure,
        ErrorCode::D1LimitError | ErrorCode::LimitInvalid => V4Error::InvalidRequest,
        ErrorCode::D1Overloaded | ErrorCode::AdmissionBusy => V4Error::RateLimited,
        ErrorCode::D1Timeout | ErrorCode::D1ResultUnknown | ErrorCode::ResourceUnavailable => {
            V4Error::Unavailable
        }
        ErrorCode::IdempotencyConflict | ErrorCode::ResourceInvariantViolation => V4Error::Conflict,
        ErrorCode::ResourceNotFound => V4Error::NotFound,
        ErrorCode::D1SqlInvalid | ErrorCode::D1AuthorizerDenied => V4Error::InvalidRequest,
        _ => V4Error::from(error),
    }
}

fn platform_status(error: &PlatformError) -> StatusCode {
    error_status(platform_v4_error(error))
}

fn error_status(error: V4Error) -> StatusCode {
    match error {
        V4Error::AuthenticationRequired => StatusCode::UNAUTHORIZED,
        V4Error::PermissionDenied => StatusCode::FORBIDDEN,
        V4Error::InvalidRequest | V4Error::InvalidField(_) => StatusCode::BAD_REQUEST,
        V4Error::NotFound => StatusCode::NOT_FOUND,
        V4Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        V4Error::Conflict => StatusCode::CONFLICT,
        V4Error::IntegrityFailure => StatusCode::UNPROCESSABLE_ENTITY,
        V4Error::Unsupported => StatusCode::NOT_IMPLEMENTED,
        V4Error::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        V4Error::Official(_) | V4Error::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

fn raw_error(status: StatusCode, request_id: RequestId) -> Response {
    let mut response = status.into_response();
    attach_request_id(&mut response, request_id);
    response
}

fn attach_request_id(response: &mut Response, request_id: RequestId) {
    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
}
