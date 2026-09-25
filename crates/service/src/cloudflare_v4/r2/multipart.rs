//! Authenticated R2 multipart management operations backed by the runtime authority.

use super::super::storage::{json, require_no_query};
use super::{bucket, error_response, success_response};
use crate::cloudflare_v4::V4Error;
use crate::http::HttpState;
use crate::r2_protocol::{
    AbortMultipartRequest, CompleteMultipartRequest, CreateMultipartRequest,
    MultipartCreateWireOptions, UploadPartHeader,
};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::{delete, post, put};
use open_compute_artifacts::{R2HttpMetadata, R2UploadedPart};
use serde::Deserialize;
use std::collections::BTreeMap;

const RESPONSE_LIMIT: usize = 1024 * 1024;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/open-compute/r2/buckets/{bucket_name}/multipart-uploads",
            post(create),
        )
        .route(
            "/accounts/{account_id}/open-compute/r2/buckets/{bucket_name}/multipart-uploads/{upload_id}/parts/{part_number}/{object_key}",
            put(upload_part),
        )
        .route(
            "/accounts/{account_id}/open-compute/r2/buckets/{bucket_name}/multipart-uploads/{upload_id}/complete/{object_key}",
            post(complete),
        )
        .route(
            "/accounts/{account_id}/open-compute/r2/buckets/{bucket_name}/multipart-uploads/{upload_id}/abort/{object_key}",
            delete(abort),
        )
}

async fn create(
    State(state): State<HttpState>,
    Path((account, bucket_name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, bucket) = match bucket(&state, &request, &account, &bucket_name, true) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<CreateBody>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    wrap_json(
        context,
        binding
            .management_multipart_create(account, bucket.resource.id, body.into())
            .await,
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateBody {
    key: String,
    #[serde(default)]
    options: CreateOptions,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateOptions {
    #[serde(default)]
    http_metadata: R2HttpMetadata,
    #[serde(default)]
    custom_metadata: BTreeMap<String, String>,
    storage_class: Option<String>,
}

impl From<CreateBody> for CreateMultipartRequest {
    fn from(value: CreateBody) -> Self {
        Self {
            key: value.key,
            options: MultipartCreateWireOptions {
                http_metadata: value.options.http_metadata,
                custom_metadata: value.options.custom_metadata,
                storage_class: value.options.storage_class,
                ssec_key: None,
            },
        }
    }
}

async fn upload_part(
    State(state): State<HttpState>,
    Path((account, bucket_name, upload_id, part_number, object_key)): Path<(
        String,
        String,
        String,
        i32,
        String,
    )>,
    request: Request,
) -> Response {
    let (context, account, bucket) = match bucket(&state, &request, &account, &bucket_name, true) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let request_id = context.request_id();
    wrap_json(
        context,
        binding
            .management_multipart_part(
                account,
                bucket.resource.id,
                request_id,
                UploadPartHeader {
                    key: object_key,
                    upload_id,
                    part_number,
                    ssec_key: None,
                },
                request.into_body(),
            )
            .await,
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteBody {
    parts: Vec<R2UploadedPart>,
}

async fn complete(
    State(state): State<HttpState>,
    Path((account, bucket_name, upload_id, object_key)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, bucket) = match bucket(&state, &request, &account, &bucket_name, true) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<CompleteBody>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    wrap_json(
        context,
        binding
            .management_multipart_complete(
                account,
                bucket.resource.id,
                CompleteMultipartRequest {
                    key: object_key,
                    upload_id,
                    parts: body.parts,
                },
            )
            .await,
    )
    .await
}

async fn abort(
    State(state): State<HttpState>,
    Path((account, bucket_name, upload_id, object_key)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, bucket) = match bucket(&state, &request, &account, &bucket_name, true) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    match to_bytes(request.into_body(), 1).await {
        Ok(bytes) if bytes.is_empty() => {}
        _ => return error_response(V4Error::InvalidRequest, context.request_id()),
    }
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    match binding
        .management_multipart_abort(
            account,
            bucket.resource.id,
            AbortMultipartRequest {
                key: object_key,
                upload_id,
            },
        )
        .await
    {
        Ok(_) => success_response(context, ()),
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

async fn wrap_json(
    context: crate::cloudflare_v4::V4RequestContext,
    result: Result<Response, open_compute_core::PlatformError>,
) -> Response {
    let response = match result {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let Ok(bytes) = to_bytes(response.into_body(), RESPONSE_LIMIT).await else {
        return error_response(V4Error::Internal, context.request_id());
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) => success_response(context, value),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}
