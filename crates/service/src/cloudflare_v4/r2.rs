//! Official Cloudflare v4 R2 bucket and raw-object adapter.

mod headers;
mod idempotency;
mod multipart;
mod objects;

use super::storage::{
    account, context, iso_timestamp, json, now_ms, require_no_query, strict_query,
};
use super::{HttpError, V4Error, V4Permission, error_response, success_response};
use crate::http::{HttpState, REQUEST_ID_HEADER};
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use headers::header_text;
use idempotency::put_idempotency_key;
use open_compute_core::{RequestId, ResourceState};
use open_compute_storage::r2::{R2BucketRecord, R2BucketRepository};
use open_compute_workers::CreateR2BucketRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .merge(multipart::router())
        .route(
            "/accounts/{account_id}/r2/buckets",
            post(create_bucket).get(list_buckets),
        )
        .route(
            "/accounts/{account_id}/r2/buckets/{bucket_name}",
            get(get_bucket)
                .put(create_bucket_by_name)
                .delete(delete_bucket),
        )
        .route(
            "/accounts/{account_id}/open-compute/r2/buckets/{bucket_name}/usage",
            get(objects::usage),
        )
        .route(
            "/accounts/{account_id}/r2/buckets/{bucket_name}/objects",
            get(objects::list),
        )
        .route(
            "/accounts/{account_id}/r2/buckets/{bucket_name}/objects/{*object_key}",
            get(objects::get).put(objects::put).delete(objects::delete),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateBucket {
    name: String,
    location_hint: Option<String>,
    storage_class: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Bucket {
    name: String,
    creation_date: String,
    jurisdiction: String,
    storage_class: String,
}

impl Bucket {
    fn from_record(record: &R2BucketRecord) -> Result<Self, V4Error> {
        Ok(Self {
            name: record.resource.name.clone(),
            creation_date: iso_timestamp(record.resource.created_at_ms)?,
            jurisdiction: "default".to_owned(),
            storage_class: "Standard".to_owned(),
        })
    }
}

async fn create_bucket(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    mut request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    if let Err(error) = jurisdiction(request.headers()) {
        return error_response(error, context.request_id());
    }
    if let Err(error) = normalize_bucket_create_content_type(&mut request) {
        return error_response(error, context.request_id());
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match json::<CreateBucket>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if !valid_bucket_name(&body.name) {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if body
        .location_hint
        .as_deref()
        .is_some_and(|value| !matches!(value, "apac" | "eeur" | "enam" | "weur" | "wnam" | "oc"))
        || body
            .storage_class
            .as_deref()
            .is_some_and(|value| !matches!(value, "Standard" | "InfrequentAccess"))
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if body.location_hint.is_some() || body.storage_class.as_deref() == Some("InfrequentAccess") {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    create(&state, context, account_id, body.name, false).await
}

async fn create_bucket_by_name(
    State(state): State<HttpState>,
    Path((account_id, bucket_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    if let Err(error) = jurisdiction(request.headers()) {
        return error_response(error, context.request_id());
    }
    if !valid_bucket_name(&bucket_name) {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let class = match header_text(request.headers(), "cf-r2-storage-class") {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if class
        .as_deref()
        .is_some_and(|value| !matches!(value, "Standard" | "InfrequentAccess"))
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if class.as_deref() == Some("InfrequentAccess") {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    match header_text(request.headers(), "content-length") {
        Ok(Some(value)) if value == "0" => {}
        Ok(None) => {}
        Ok(Some(_)) | Err(_) => {
            return error_response(V4Error::InvalidRequest, context.request_id());
        }
    }
    match to_bytes(request.into_body(), 1).await {
        Ok(bytes) if bytes.is_empty() => {}
        _ => return error_response(V4Error::InvalidRequest, context.request_id()),
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    create(&state, context, account_id, bucket_name, true).await
}

async fn create(
    state: &HttpState,
    context: super::V4RequestContext,
    account_id: open_compute_core::InstanceId,
    name: String,
    put_by_name: bool,
) -> Response {
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let now = now_ms();
    let idempotency_key = if put_by_name {
        match put_idempotency_key(api, account_id, &name) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        }
    } else {
        request_id.to_string()
    };
    match api
        .controller()
        .create(&CreateR2BucketRequest {
            instance_id: account_id,
            name,
            idempotency_key,
            request_id,
            now_ms: now,
            reconcile_by_name: put_by_name,
        })
        .await
    {
        Ok(record) => bucket_success(context, &record),
        Err(error) => error_response(V4Error::from(&error), request_id),
    }
}

fn bucket_success(context: super::V4RequestContext, record: &R2BucketRecord) -> Response {
    match Bucket::from_record(record) {
        Ok(bucket) => success_response(context, bucket),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn list_buckets(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = jurisdiction(request.headers()) {
        return error_response(error, context.request_id());
    }
    let query = match bucket_list_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let records = match R2BucketRepository::new(api.storage().db()).list(account_id) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let start_after = match (query.cursor.as_deref(), query.start_after.as_deref()) {
        (Some(_), Some(_)) => {
            return error_response(V4Error::InvalidRequest, context.request_id());
        }
        (Some(cursor), None) => match decode_cursor(api, account_id, &query, cursor) {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, context.request_id()),
        },
        (None, value) => value.map(str::to_owned),
    };
    let mut records: Vec<_> = records
        .into_iter()
        .filter(|record| {
            record.resource.state == ResourceState::Ready
                && query
                    .name_contains
                    .as_deref()
                    .is_none_or(|needle| record.resource.name.contains(needle))
        })
        .collect();
    records.sort_by(|left, right| left.resource.name.cmp(&right.resource.name));
    if query.direction.as_deref() == Some("desc") {
        records.reverse();
    }
    // Cloudflare counts every bucket that matches the filter, across all pages.
    let total_count = records.len();
    if let Some(start) = start_after {
        records.retain(|record| {
            if query.direction.as_deref() == Some("desc") {
                record.resource.name < start
            } else {
                record.resource.name > start
            }
        });
    }
    let has_more = records.len() > query.per_page;
    records.truncate(query.per_page);
    let cursor = if has_more {
        match records.last() {
            Some(record) => match encode_cursor(api, account_id, &query, &record.resource.name) {
                Ok(value) => value,
                Err(error) => return error_response(error, context.request_id()),
            },
            None => return error_response(V4Error::Internal, context.request_id()),
        }
    } else {
        String::new()
    };
    let buckets: Result<Vec<_>, _> = records.iter().map(Bucket::from_record).collect();
    match buckets {
        Ok(buckets) => bucket_list_response(
            context.request_id(),
            &buckets,
            BucketCursorInfo {
                count: buckets.len(),
                cursor,
                per_page: query.per_page,
                total_count,
            },
        ),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn get_bucket(
    State(state): State<HttpState>,
    Path((account_id, bucket_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, _, bucket) = match bucket(&state, &request, &account_id, &bucket_name, false) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    match Bucket::from_record(&bucket) {
        Ok(bucket) => success_response(context, bucket),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn delete_bucket(
    State(state): State<HttpState>,
    Path((account_id, bucket_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, _account_id, bucket) =
        match bucket(&state, &request, &account_id, &bucket_name, true) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    match api.delete_bucket(&bucket, request_id, now_ms()).await {
        Ok(()) => success_response(context, ()),
        Err(error) => error_response(V4Error::from(&error), request_id),
    }
}

fn bucket(
    state: &HttpState,
    request: &Request,
    account_id: &str,
    bucket_name: &str,
    write: bool,
) -> Result<
    (
        super::V4RequestContext,
        open_compute_core::InstanceId,
        R2BucketRecord,
    ),
    HttpError,
> {
    let context = context(
        request,
        if write {
            V4Permission::ProductWrite
        } else {
            V4Permission::Read
        },
    )?;
    if !valid_bucket_name(bucket_name) {
        return Err(HttpError::from_response(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        )));
    }
    if let Err(error) = jurisdiction(request.headers()) {
        return Err(HttpError::from_response(error_response(
            error,
            context.request_id(),
        )));
    }
    let account_id = account(state, account_id)
        .map_err(|error| HttpError::from_response(error_response(error, context.request_id())))?;
    let api = state
        .r2_api()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    let record = R2BucketRepository::new(api.storage().db())
        .list(account_id)
        .map_err(|error| {
            HttpError::from_response(error_response(V4Error::from(&error), context.request_id()))
        })?
        .into_iter()
        .find(|record| {
            record.resource.name == bucket_name && record.resource.state == ResourceState::Ready
        })
        .ok_or_else(|| error_response(V4Error::NotFound, context.request_id()))?;
    Ok((context, account_id, record))
}

fn valid_bucket_name(name: &str) -> bool {
    (3..=63).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn jurisdiction(headers: &HeaderMap) -> Result<(), V4Error> {
    match header_text(headers, "cf-r2-jurisdiction")?.as_deref() {
        None | Some("default") => Ok(()),
        Some("eu" | "fedramp") => Err(V4Error::Unsupported),
        Some(_) => Err(V4Error::InvalidRequest),
    }
}

fn normalize_bucket_create_content_type(request: &mut Request) -> Result<(), V4Error> {
    let mut values = request.headers().get_all(header::CONTENT_TYPE).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(V4Error::InvalidRequest);
    }
    if value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("text/plain;charset=UTF-8"))
    {
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    Ok(())
}

fn attach_request_id(response: &mut Response, request_id: RequestId) {
    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
}

struct BucketListQuery {
    name_contains: Option<String>,
    start_after: Option<String>,
    cursor: Option<String>,
    per_page: usize,
    direction: Option<String>,
}

fn bucket_list_query(request: &Request) -> Result<BucketListQuery, V4Error> {
    let mut values = strict_query(request)?;
    let name_contains = values.remove("name_contains");
    let start_after = values.remove("start_after");
    let cursor = values.remove("cursor");
    let per_page = values
        .remove("per_page")
        .map(|value| value.parse().map_err(|_| V4Error::InvalidRequest))
        .transpose()?
        .unwrap_or(20);
    if !(1..=1000).contains(&per_page) {
        return Err(V4Error::InvalidRequest);
    }
    if values
        .remove("order")
        .as_deref()
        .is_some_and(|value| value != "name")
    {
        return Err(V4Error::InvalidRequest);
    }
    let direction = values.remove("direction");
    if direction
        .as_deref()
        .is_some_and(|value| !matches!(value, "asc" | "desc"))
        || !values.is_empty()
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(BucketListQuery {
        name_contains,
        start_after,
        cursor,
        per_page,
        direction,
    })
}

const BUCKET_CURSOR_TTL_MS: i64 = 15 * 60 * 1000;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BucketCursor {
    version: u8,
    account_id: String,
    name_contains: Option<String>,
    per_page: usize,
    direction: Option<String>,
    last_name: String,
    expires_at_ms: i64,
}

fn encode_cursor(
    api: &crate::r2_api::R2ApiState,
    account_id: open_compute_core::InstanceId,
    query: &BucketListQuery,
    last_name: &str,
) -> Result<String, V4Error> {
    let expires_at_ms = now_ms()
        .checked_add(BUCKET_CURSOR_TTL_MS)
        .ok_or(V4Error::Internal)?;
    let payload = serde_json::to_vec(&BucketCursor {
        version: 1,
        account_id: account_id.to_string(),
        name_contains: query.name_contains.clone(),
        per_page: query.per_page,
        direction: query.direction.clone(),
        last_name: last_name.to_owned(),
        expires_at_ms,
    })
    .map_err(|_| V4Error::Internal)?;
    let signature = api.storage().crypto().sign_r2_cursor(&payload);
    let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    Ok(format!(
        "{}.{}",
        base64.encode(payload),
        base64.encode(signature)
    ))
}

fn decode_cursor(
    api: &crate::r2_api::R2ApiState,
    account_id: open_compute_core::InstanceId,
    query: &BucketListQuery,
    cursor: &str,
) -> Result<String, V4Error> {
    let (payload, signature) = cursor.split_once('.').ok_or(V4Error::InvalidRequest)?;
    if signature.contains('.') {
        return Err(V4Error::InvalidRequest);
    }
    let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload = base64
        .decode(payload)
        .map_err(|_| V4Error::InvalidRequest)?;
    let signature = base64
        .decode(signature)
        .map_err(|_| V4Error::InvalidRequest)?;
    if !api
        .storage()
        .crypto()
        .verify_r2_cursor(&payload, &signature)
    {
        return Err(V4Error::InvalidRequest);
    }
    let payload: BucketCursor =
        serde_json::from_slice(&payload).map_err(|_| V4Error::InvalidRequest)?;
    if payload.version != 1
        || payload.account_id != account_id.to_string()
        || payload.name_contains != query.name_contains
        || payload.per_page != query.per_page
        || payload.direction != query.direction
        || payload.expires_at_ms < now_ms()
        || !valid_bucket_name(&payload.last_name)
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(payload.last_name)
}

#[derive(Serialize)]
struct BucketListEnvelope {
    success: bool,
    result: Value,
    result_info: BucketCursorInfo,
    errors: [Value; 0],
    messages: [Value; 0],
}

#[derive(Serialize)]
struct BucketCursorInfo {
    count: usize,
    cursor: String,
    per_page: usize,
    total_count: usize,
}

fn bucket_list_response(
    request_id: RequestId,
    buckets: &[Bucket],
    result_info: BucketCursorInfo,
) -> Response {
    let mut response = Json(BucketListEnvelope {
        success: true,
        result: serde_json::json!({ "buckets": buckets }),
        result_info,
        errors: [],
        messages: [],
    })
    .into_response();
    attach_request_id(&mut response, request_id);
    response
}

#[cfg(test)]
mod tests;
