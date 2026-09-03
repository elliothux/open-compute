//! Official Cloudflare v4 R2 bucket and raw-object adapter.

mod idempotency;
mod objects;

use super::storage::{
    account, context, iso_timestamp, json, now_ms, require_no_query, strict_query,
};
use super::{V4Error, V4Permission, error_response, success_response};
use crate::http::{HttpState, REQUEST_ID_HEADER};
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use idempotency::{create_fingerprint, put_idempotency_key};
use open_compute_core::{BindingKind, ErrorCode, RequestId, ResourceId, ResourceState};
use open_compute_storage::{
    R2_SCHEMA_VERSION, R2BucketRecord, R2BucketRepository, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRecord, ResourceRepository,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
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
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    if let Err(error) = jurisdiction(request.headers()) {
        return error_response(error, context.request_id());
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match json::<CreateBucket>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
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
        Err(response) => return response,
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
    account_id: open_compute_core::AccountId,
    name: String,
    put_by_name: bool,
) -> Response {
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let fingerprint = match create_fingerprint(api, account_id, &name) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let idempotency_key = if put_by_name {
        match put_idempotency_key(api, account_id, &name) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        }
    } else {
        request_id.to_string()
    };
    let expires_at_ms = match now.checked_add(IDEMPOTENCY_TTL_MS) {
        Some(value) => value,
        None => return error_response(V4Error::Internal, request_id),
    };
    let reservation_input = ReserveResourceCreate {
        account_id,
        kind: BindingKind::R2Bucket,
        name: &name,
        idempotency_key: &idempotency_key,
        fingerprint_key_id: api.storage().crypto().fingerprint_key_id(),
        request_fingerprint: &fingerprint,
        resource_id: ResourceId::generate(),
        driver_schema_version: R2_SCHEMA_VERSION,
        request_id,
        now_ms: now,
        expires_at_ms,
    };
    let max_resources = api.storage().hardening().max_resources_per_kind_per_account;
    let reservation = ResourceRepository::new(api.storage().db())
        .reserve_create(&reservation_input, max_resources);
    let resource = match reservation {
        Ok(ResourceCreateReservation::Reserved(value))
        | Ok(ResourceCreateReservation::Continue(value)) => value,
        Ok(ResourceCreateReservation::Complete(response)) => {
            if put_by_name {
                return match reconcile_named_bucket(api, account_id, &name, now).await {
                    Ok(Some(record)) => bucket_success(context, &record),
                    Ok(None) => error_response(V4Error::Conflict, request_id),
                    Err(error) => error_response(error, request_id),
                };
            }
            return persisted_bucket_response(context, request_id, &response);
        }
        Ok(ResourceCreateReservation::Failed(_)) => {
            return error_response(V4Error::Conflict, request_id);
        }
        Err(error) if put_by_name && error.code() == ErrorCode::ResourceNameConflict => {
            return match reconcile_named_bucket(api, account_id, &name, now).await {
                Ok(Some(record)) => bucket_success(context, &record),
                Ok(None) => error_response(V4Error::Conflict, request_id),
                Err(error) => error_response(error, request_id),
            };
        }
        Err(error) => return error_response(V4Error::from(&error), request_id),
    };
    let driver = api.resource_driver();
    let reconciled = match driver.reconcile(&resource).await {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), request_id),
    };
    if resource.state == ResourceState::Creating
        && let Err(error) = ResourceRepository::new(api.storage().db()).mark_ready(resource.id, now)
    {
        let current = ResourceRepository::new(api.storage().db()).get(account_id, resource.id);
        if !matches!(current, Ok(current) if current.state == ResourceState::Ready) {
            return error_response(V4Error::from(&error), request_id);
        }
    }
    let record =
        match R2BucketRepository::new(api.storage().db()).get(account_id, reconciled.resource.id) {
            Ok(value) => value,
            Err(error) => return error_response(V4Error::from(&error), request_id),
        };
    let result = match Bucket::from_record(&record) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let persisted = match serde_json::to_vec(&result) {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::Internal, request_id),
    };
    if let Err(error) = ResourceRepository::new(api.storage().db()).complete_create(
        account_id,
        &idempotency_key,
        &fingerprint,
        record.resource.id,
        &persisted,
    ) {
        if error.code() != ErrorCode::IdempotencyConflict {
            return error_response(V4Error::from(&error), request_id);
        }
        return match ResourceRepository::new(api.storage().db())
            .reserve_create(&reservation_input, max_resources)
        {
            Ok(ResourceCreateReservation::Complete(response)) => {
                persisted_bucket_response(context, request_id, &response)
            }
            Ok(_) | Err(_) => error_response(V4Error::Conflict, request_id),
        };
    }
    success_response(context, result)
}

fn persisted_bucket_response(
    context: super::V4RequestContext,
    request_id: RequestId,
    response: &[u8],
) -> Response {
    match serde_json::from_slice::<Bucket>(response) {
        Ok(bucket) => success_response(context, bucket),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

async fn reconcile_named_bucket(
    api: &crate::r2_api::R2ApiState,
    account_id: open_compute_core::AccountId,
    name: &str,
    now_ms: i64,
) -> Result<Option<R2BucketRecord>, V4Error> {
    let resource = current_named_resource(
        ResourceRepository::new(api.storage().db())
            .list(account_id, Some(BindingKind::R2Bucket))
            .map_err(|error| V4Error::from(&error))?,
        name,
    );
    let Some(resource) = resource else {
        return Ok(None);
    };
    match resource.state {
        ResourceState::Ready => R2BucketRepository::new(api.storage().db())
            .get(account_id, resource.id)
            .map(Some)
            .map_err(|error| V4Error::from(&error)),
        ResourceState::Creating => {
            let reconciled = api
                .resource_driver()
                .reconcile(&resource)
                .await
                .map_err(|error| V4Error::from(&error))?;
            ResourceRepository::new(api.storage().db())
                .mark_ready(resource.id, now_ms)
                .map_err(|error| V4Error::from(&error))?;
            R2BucketRepository::new(api.storage().db())
                .get(account_id, reconciled.resource.id)
                .map(Some)
                .map_err(|error| V4Error::from(&error))
        }
        ResourceState::Deleting => Err(V4Error::Conflict),
        ResourceState::Tombstoned => Ok(None),
    }
}

fn current_named_resource(resources: Vec<ResourceRecord>, name: &str) -> Option<ResourceRecord> {
    resources
        .into_iter()
        .find(|resource| resource.name == name && resource.state != ResourceState::Tombstoned)
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
        Err(response) => return response,
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
        (Some(_), Some(_)) => return error_response(V4Error::InvalidRequest, context.request_id()),
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
        Ok(buckets) => bucket_list_response(context.request_id(), buckets, cursor),
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
        Err(response) => return response,
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
    let (context, account_id, bucket) =
        match bucket(&state, &request, &account_id, &bucket_name, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let resources = ResourceRepository::new(api.storage().db());
    match resources.referrers(bucket.resource.id) {
        Ok(referrers) if referrers.is_empty() => {}
        Ok(_) => return error_response(V4Error::Conflict, request_id),
        Err(error) => return error_response(V4Error::from(&error), request_id),
    }
    let driver = api.resource_driver();
    if let Err(error) = driver.require_empty(&bucket).await {
        return error_response(V4Error::from(&error), request_id);
    }
    if let Err(error) = api
        .pins()
        .fence_and_wait(bucket.resource.id, api.delete_drain_timeout())
        .await
    {
        return error_response(V4Error::from(&error), request_id);
    }
    let result = async {
        let now = now_ms().map_err(|_| V4Error::Internal)?;
        resources
            .begin_delete(account_id, bucket.resource.id, now)
            .map_err(|error| V4Error::from(&error))?;
        R2BucketRepository::new(api.storage().db())
            .mark_delete_started(bucket.resource.id, now)
            .map_err(|error| V4Error::from(&error))?;
        crate::r2_backend::multipart::reconcile_bucket_multipart(
            api.storage(),
            api.objects(),
            &bucket,
            false,
            true,
            std::time::Duration::from_millis(api.config().operation_timeout_ms),
        )
        .await
        .map_err(|error| V4Error::from(&error))?;
        crate::r2_backend::objects::reconcile_bucket_objects(
            api.storage(),
            api.objects(),
            &bucket,
            std::time::Duration::from_millis(api.config().operation_timeout_ms),
        )
        .await
        .map_err(|error| V4Error::from(&error))?;
        driver
            .finalize_delete(&bucket)
            .await
            .map_err(|error| V4Error::from(&error))?;
        resources
            .mark_tombstoned(account_id, bucket.resource.id, request_id, now_ms()?)
            .map_err(|error| V4Error::from(&error))
    }
    .await;
    match result {
        Ok(()) => {
            api.pins().retire_fence(bucket.resource.id);
            success_response(context, ())
        }
        Err(error) => {
            api.pins().unfence(bucket.resource.id);
            error_response(error, request_id)
        }
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
        open_compute_core::AccountId,
        R2BucketRecord,
    ),
    Response,
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
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    if let Err(error) = jurisdiction(request.headers()) {
        return Err(error_response(error, context.request_id()));
    }
    let account_id =
        account(state, account_id).map_err(|error| error_response(error, context.request_id()))?;
    let api = state
        .r2_api()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    let record = R2BucketRepository::new(api.storage().db())
        .list(account_id)
        .map_err(|error| error_response(V4Error::from(&error), context.request_id()))?
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
    account_id: open_compute_core::AccountId,
    query: &BucketListQuery,
    last_name: &str,
) -> Result<String, V4Error> {
    let expires_at_ms = now_ms()?
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
    account_id: open_compute_core::AccountId,
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
        || payload.expires_at_ms < now_ms()?
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
}

fn bucket_list_response(request_id: RequestId, buckets: Vec<Bucket>, cursor: String) -> Response {
    let count = buckets.len();
    let mut response = Json(BucketListEnvelope {
        success: true,
        result: serde_json::json!({ "buckets": buckets }),
        result_info: BucketCursorInfo { count, cursor },
        errors: [],
        messages: [],
    })
    .into_response();
    attach_request_id(&mut response, request_id);
    response
}

fn header_text(headers: &HeaderMap, name: &'static str) -> Result<Option<String>, V4Error> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(V4Error::InvalidRequest);
    }
    value
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| V4Error::InvalidRequest)
        })
        .transpose()
}

#[cfg(test)]
mod tests;
