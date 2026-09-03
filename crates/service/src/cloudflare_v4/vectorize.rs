//! Official Cloudflare Vectorize v2 management adapter.

mod cursor;
mod indexes;

use super::storage::{
    account, context, iso_timestamp, json, now_ms, require_no_query, strict_query,
};
use super::{V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use crate::search_api::SearchApiState;
use crate::vectorize_backend::{QueryOptions, ReturnMetadata, execute_query, run_query_cpu};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::{get, post};
use open_compute_core::{AccountId, PlatformError, ResourceState};
use open_compute_storage::{
    VectorMutationInput, VectorMutationKind, VectorizeEngine, VectorizeIndexRecord,
    VectorizeIndexRepository, VectorizePaths,
};
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

const MAX_NDJSON_BODY: usize = 24 * 1024 * 1024;
const CURSOR_LIFETIME_MS: i64 = 15 * 60 * 1_000;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes",
            get(indexes::list).post(indexes::create),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}",
            get(indexes::get).delete(indexes::delete),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/insert",
            post(insert),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/upsert",
            post(upsert),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/query",
            post(query),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/get_by_ids",
            post(get_by_ids),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/delete_by_ids",
            post(delete_by_ids),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/info",
            get(info),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/list",
            get(list_vectors),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/metadata_index/create",
            post(create_metadata_index),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/metadata_index/list",
            get(list_metadata_indexes),
        )
        .route(
            "/accounts/{account_id}/vectorize/v2/indexes/{index_name}/metadata_index/delete",
            post(delete_metadata_index),
        )
}

async fn insert(
    state: State<HttpState>,
    path: Path<(String, String)>,
    request: Request,
) -> Response {
    mutate(state, path, request, VectorMutationKind::Insert).await
}

async fn upsert(
    state: State<HttpState>,
    path: Path<(String, String)>,
    request: Request,
) -> Response {
    mutate(state, path, request, VectorMutationKind::Upsert).await
}

async fn mutate(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
    kind: VectorMutationKind,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match strict_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if query.keys().any(|key| key != "unparsable-behavior") {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let behavior = query
        .get("unparsable-behavior")
        .map(String::as_str)
        .unwrap_or("error");
    if !matches!(behavior, "error" | "discard")
        || !exact_content_type(&request, "application/x-ndjson")
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let bytes = match to_bytes(request.into_body(), MAX_NDJSON_BODY).await {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    let items = match parse_ndjson(&bytes, behavior == "discard") {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let _pin = api
            .pins()
            .try_pin(record.resource.id)
            .map_err(|error| V4Error::from(&error))?;
        let engine = open_engine(&api, &record)?;
        engine
            .enqueue(kind, &items, now_ms()?)
            .map(|receipt| receipt.mutation_id)
            .map_err(|error| V4Error::from(&error))
    })
    .await;
    match result {
        Ok(Ok(mutation_id)) => {
            success_response(context, serde_json::json!({"mutationId": mutation_id}))
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NdjsonVector {
    id: String,
    values: Vec<f32>,
    namespace: Option<String>,
    metadata: Option<Value>,
}

fn parse_ndjson(bytes: &[u8], discard: bool) -> Result<Vec<VectorMutationInput>, V4Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| V4Error::InvalidRequest)?;
    let mut items = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            if discard {
                continue;
            }
            return Err(V4Error::InvalidRequest);
        }
        match serde_json::from_str::<NdjsonVector>(line) {
            Ok(value) => items.push(VectorMutationInput {
                id: value.id,
                namespace: value.namespace,
                values: Some(value.values),
                metadata: value.metadata,
            }),
            Err(_) if discard => {}
            Err(_) => return Err(V4Error::InvalidRequest),
        }
    }
    if items.is_empty() {
        Err(V4Error::InvalidRequest)
    } else {
        Ok(items)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Query {
    vector: Vec<f32>,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default)]
    return_values: bool,
    #[serde(default)]
    return_metadata: QueryMetadata,
    filter: Option<Value>,
}

const fn default_top_k() -> usize {
    5
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum QueryMetadata {
    #[default]
    None,
    Indexed,
    All,
}

async fn query(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<Query>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let result = run_query_cpu(move || {
        let _pin = api.pins().try_pin(record.resource.id)?;
        let engine = open_engine_platform(&api, &record)?;
        execute_query(
            &engine,
            &record.resource.id.to_string(),
            &body.vector,
            &QueryOptions {
                top_k: body.top_k,
                namespace: None,
                return_values: body.return_values,
                return_metadata: match body.return_metadata {
                    QueryMetadata::None => ReturnMetadata::None,
                    QueryMetadata::Indexed => ReturnMetadata::Indexed,
                    QueryMetadata::All => ReturnMetadata::All,
                },
                filter: body.filter,
            },
            Instant::now() + Duration::from_secs(15),
        )
    })
    .await;
    match result {
        Ok(value) => success_response(context, value),
        Err(error) => error_response(V4Error::from(&error), request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ids {
    ids: Vec<String>,
}

async fn get_by_ids(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    vector_ids(state, account_id, index_name, request, false).await
}

async fn delete_by_ids(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    vector_ids(state, account_id, index_name, request, true).await
}

async fn vector_ids(
    state: HttpState,
    account_id: String,
    index_name: String,
    request: Request,
    delete: bool,
) -> Response {
    let permission = if delete {
        V4Permission::ProductWrite
    } else {
        V4Permission::Read
    };
    let context = match context(&request, permission) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<Ids>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if body.ids.is_empty() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let request_id = context.request_id();
    let mutation_time = if delete {
        match now_ms() {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, request_id),
        }
    } else {
        None
    };
    let result = tokio::task::spawn_blocking(move || {
        let _pin = api.pins().try_pin(record.resource.id)?;
        let engine = open_engine_platform(&api, &record)?;
        if delete {
            let items = body
                .ids
                .into_iter()
                .map(|id| VectorMutationInput {
                    id,
                    namespace: None,
                    values: None,
                    metadata: None,
                })
                .collect::<Vec<_>>();
            engine
                .enqueue(
                    VectorMutationKind::Delete,
                    &items,
                    mutation_time.ok_or_else(internal_platform)?,
                )
                .map(|receipt| serde_json::json!({"mutationId": receipt.mutation_id}))
        } else {
            serde_json::to_value(engine.get_by_ids(&body.ids)?).map_err(|_| internal_platform())
        }
    })
    .await;
    match result {
        Ok(Ok(value)) => success_response(context, value),
        Ok(Err(error)) => error_response(V4Error::from(&error), request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

async fn info(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = open_engine(&api, &record)
        .and_then(|engine| engine.describe().map_err(|error| V4Error::from(&error)));
    match result {
        Ok(value) => {
            let processed = match value.processed_at_ms.map(iso_timestamp).transpose() {
                Ok(value) => value,
                Err(error) => return error_response(error, context.request_id()),
            };
            success_response(
                context,
                serde_json::json!({"dimensions": value.dimensions, "processedUpToDatetime": processed, "processedUpToMutation": value.processed_mutation_id, "vectorCount": value.vector_count}),
            )
        }
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn list_vectors(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match strict_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if query
        .keys()
        .any(|key| !matches!(key.as_str(), "count" | "cursor"))
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let count = match query.get("count").map_or(Ok(100), |value| {
        value.parse::<usize>().map_err(|_| V4Error::InvalidRequest)
    }) {
        Ok(value @ 1..=1000) => value,
        _ => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    let (account, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let after = match query.get("cursor") {
        Some(token) => match cursor::open(api.storage(), token, account, &index_name, count, now) {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, context.request_id()),
        },
        None => None,
    };
    let engine = match open_engine(&api, &record) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let mut ids = Vec::new();
    if let Err(error) = engine.scan_candidates(None, None, |record| {
        ids.push(record.id);
        Ok(())
    }) {
        return error_response(V4Error::from(&error), context.request_id());
    }
    ids.sort();
    let total = ids.len();
    if let Some(after) = after.as_deref() {
        ids.retain(|id| id.as_str() > after);
    }
    let truncated = ids.len() > count;
    ids.truncate(count);
    let expires = match now.checked_add(CURSOR_LIFETIME_MS) {
        Some(value) => value,
        None => return error_response(V4Error::Internal, context.request_id()),
    };
    let next = if truncated {
        match ids
            .last()
            .map(|id| cursor::seal(api.storage(), account, &index_name, count, id, expires))
            .transpose()
        {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        }
    } else {
        None
    };
    let expiration = if next.is_some() {
        match iso_timestamp(expires) {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, context.request_id()),
        }
    } else {
        None
    };
    let vectors = ids
        .iter()
        .map(|id| serde_json::json!({"id": id}))
        .collect::<Vec<_>>();
    success_response(
        context,
        serde_json::json!({"count": vectors.len(), "totalCount": total, "isTruncated": truncated, "nextCursor": next, "cursorExpirationTimestamp": expiration, "vectors": vectors}),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateMetadata {
    property_name: String,
    index_type: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteMetadata {
    property_name: String,
}

async fn create_metadata_index(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<CreateMetadata>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    match open_engine(&api, &record).and_then(|engine| {
        engine
            .create_metadata_index(&body.property_name, &body.index_type, now_ms()?)
            .map_err(|error| V4Error::from(&error))
    }) {
        Ok(()) => success_response(
            context,
            serde_json::json!({"mutationId": uuid::Uuid::now_v7().to_string()}),
        ),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn list_metadata_indexes(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    match open_engine(&api, &record).and_then(|engine| {
        engine
            .metadata_indexes()
            .map_err(|error| V4Error::from(&error))
    }) {
        Ok(value) => success_response(context, serde_json::json!({"metadataIndexes": value})),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn delete_metadata_index(
    State(state): State<HttpState>,
    Path((account_id, index_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<DeleteMetadata>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    match open_engine(&api, &record).and_then(|engine| {
        engine
            .delete_metadata_index(&body.property_name)
            .map_err(|error| V4Error::from(&error))
    }) {
        Ok(()) => success_response(
            context,
            serde_json::json!({"mutationId": uuid::Uuid::now_v7().to_string()}),
        ),
        Err(error) => error_response(error, context.request_id()),
    }
}

fn ready_index(
    state: &HttpState,
    public_account: &str,
    name: &str,
) -> Result<
    (
        AccountId,
        std::sync::Arc<SearchApiState>,
        VectorizeIndexRecord,
    ),
    V4Error,
> {
    if !valid_index_name(name) {
        return Err(V4Error::NotFound);
    }
    let account = account(state, public_account)?;
    let api = state.search_api().cloned().ok_or(V4Error::Unavailable)?;
    let record = VectorizeIndexRepository::new(api.storage().db())
        .list(account)
        .map_err(|error| V4Error::from(&error))?
        .into_iter()
        .find(|record| {
            record.resource.name == name && record.resource.state == ResourceState::Ready
        })
        .ok_or(V4Error::NotFound)?;
    Ok((account, api, record))
}

fn open_engine(
    api: &SearchApiState,
    record: &VectorizeIndexRecord,
) -> Result<VectorizeEngine, V4Error> {
    open_engine_platform(api, record).map_err(|error| V4Error::from(&error))
}
fn open_engine_platform(
    api: &SearchApiState,
    record: &VectorizeIndexRecord,
) -> Result<VectorizeEngine, PlatformError> {
    let path = VectorizePaths::open(api.storage().data_dir().root())?.resolve_storage_key(
        &record.storage_key,
        record.resource.account_id,
        record.resource.id,
    )?;
    VectorizeEngine::open(
        &path,
        &record.resource.id.to_string(),
        record.dimensions,
        &record.metric,
        record.quota_vectors,
        record.quota_bytes,
        api.busy_timeout_ms(),
    )
}

fn valid_index_name(value: &str) -> bool {
    value.len() >= 2
        && value.chars().count() <= 128
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}
fn exact_content_type(request: &Request, expected: &str) -> bool {
    let mut values = request
        .headers()
        .get_all(axum::http::header::CONTENT_TYPE)
        .iter();
    values.next().and_then(|value| value.to_str().ok()) == Some(expected) && values.next().is_none()
}

fn internal_platform() -> PlatformError {
    PlatformError::new(
        open_compute_core::ErrorCode::Internal,
        "Vectorize response serialization failed",
    )
}

#[cfg(test)]
#[path = "vectorize/tests.rs"]
mod tests;
