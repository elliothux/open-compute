//! Vectorize index catalog and lifecycle routes.

use super::{ready_index, valid_index_name};
use crate::cloudflare_v4::storage::{
    account, context, iso_timestamp, json, now_ms, require_no_query,
};
use crate::cloudflare_v4::{V4Error, V4Permission, error_response, success_response};
use crate::http::HttpState;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use open_compute_core::BindingKind;
use open_compute_storage::{
    VECTORIZE_SCHEMA_VERSION, VectorizeIndexRecord, VectorizeIndexRepository,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, ResourceController, VectorizeIndexSpec,
    VectorizeResourceDriver,
};
use serde::{Deserialize, Serialize};

const DEFAULT_VECTOR_QUOTA: u64 = 100_000;
const DEFAULT_VECTOR_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateIndex {
    name: String,
    config: IndexConfig,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IndexConfig {
    Dimensions { dimensions: u32, metric: String },
    Preset { preset: String },
}

#[derive(Serialize)]
struct Index<'a> {
    name: &'a str,
    config: DimensionConfig<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    created_on: String,
    modified_on: String,
}

#[derive(Serialize)]
struct DimensionConfig<'a> {
    dimensions: u32,
    metric: &'a str,
}

pub(super) async fn create(
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
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let body = match json::<CreateIndex>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_index_name(&body.name) {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let (dimensions, metric) = match resolve_config(body.config) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.search_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let driver = VectorizeResourceDriver::new(
            api.storage(),
            VectorizeIndexSpec {
                dimensions,
                metric,
                quota_vectors: DEFAULT_VECTOR_QUOTA,
                quota_bytes: DEFAULT_VECTOR_BYTES,
            },
            api.busy_timeout_ms(),
        )
        .with_description(body.description);
        let outcome = ResourceController::new(api.storage(), api.pins().clone(), driver)
            .create(&CreateResourceRequest {
                account_id,
                kind: BindingKind::VectorizeIndex,
                name: body.name,
                idempotency_key: request_id.to_string(),
                driver_schema_version: VECTORIZE_SCHEMA_VERSION,
                request_id,
                now_ms: now_ms()?,
            })
            .map_err(|error| V4Error::from(&error))?;
        match outcome {
            CreateResourceOutcome::Applied(value) => {
                VectorizeIndexRepository::new(api.storage().db())
                    .get(account_id, value.resource_id)
                    .map_err(|error| V4Error::from(&error))
            }
            CreateResourceOutcome::Replay(_) => Err(V4Error::Conflict),
        }
    })
    .await;
    match result {
        Ok(Ok(record)) => response(context, &record),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn list(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.search_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let records = match VectorizeIndexRepository::new(api.storage().db()).list(account_id) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let indexes = match records
        .iter()
        .filter(|record| record.resource.state == open_compute_core::ResourceState::Ready)
        .map(value)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    success_response(context, indexes)
}

pub(super) async fn get(
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
    match ready_index(&state, &account_id, &index_name) {
        Ok((_, _, record)) => response(context, &record),
        Err(error) => error_response(error, context.request_id()),
    }
}

pub(super) async fn delete(
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
    let (_, api, record) = match ready_index(&state, &account_id, &index_name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = ResourceController::new(
        api.storage(),
        api.pins().clone(),
        VectorizeResourceDriver::recovery(api.storage(), api.busy_timeout_ms()),
    )
    .delete(
        record.resource.account_id,
        record.resource.id,
        context.request_id(),
        now,
        api.delete_drain_timeout(),
    )
    .await;
    match result {
        Ok(()) => success_response(context, Option::<()>::None),
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

fn response(
    context: crate::cloudflare_v4::V4RequestContext,
    record: &VectorizeIndexRecord,
) -> Response {
    match value(record) {
        Ok(value) => success_response(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

fn value(record: &VectorizeIndexRecord) -> Result<Index<'_>, V4Error> {
    let timestamp = iso_timestamp(record.resource.created_at_ms)?;
    Ok(Index {
        name: &record.resource.name,
        config: DimensionConfig {
            dimensions: record.dimensions,
            metric: &record.metric,
        },
        description: record.description.as_deref(),
        created_on: timestamp.clone(),
        modified_on: timestamp,
    })
}

fn resolve_config(config: IndexConfig) -> Result<(u32, String), V4Error> {
    let (dimensions, metric) = match config {
        IndexConfig::Dimensions { dimensions, metric } => (dimensions, metric),
        IndexConfig::Preset { preset } => (
            match preset.as_str() {
                "@cf/baai/bge-small-en-v1.5" => 384,
                "@cf/baai/bge-base-en-v1.5" | "cohere/embed-multilingual-v2.0" => 768,
                "@cf/baai/bge-large-en-v1.5" => 1024,
                "openai/text-embedding-ada-002" => 1536,
                _ => return Err(V4Error::InvalidRequest),
            },
            "cosine".to_owned(),
        ),
    };
    if !(1..=1536).contains(&dimensions)
        || !matches!(metric.as_str(), "cosine" | "euclidean" | "dot-product")
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok((dimensions, metric))
}
