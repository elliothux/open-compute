//! Official Cloudflare v4 Workers KV catalog and key-list adapter.

mod bulk;
mod values;

use super::storage::{
    account, context, now_ms, require_no_query, resolve_resource_id, strict_query,
};
use super::{
    V4Error, V4Permission, V4ResourceKind, error_response, paginated_response, success_response,
};
use crate::binding_backend::KvBindingExecutor;
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::kv_backend::{KvCommand, KvCommandResult};
use crate::resource_binding::management_binding;
use axum::extract::{Path, Request, State};
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use open_compute_core::{BindingKind, RequestId};
use open_compute_storage::{KV_MAX_LIST_LIMIT, KvNamespaceRecord, KvNamespaceRepository};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, KvResourceDriver, ResourceController,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const KIND: V4ResourceKind = V4ResourceKind::KvNamespace;
const MAX_VALUE_BODY: usize = 25 * 1024 * 1024;
const MAX_BULK_KEYS: usize = 10_000;
const MAX_BULK_BODY: usize = 100 * 1024 * 1024 - 1;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/storage/kv/namespaces",
            post(create_namespace).get(list_namespaces),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}",
            get(get_namespace)
                .put(rename_namespace)
                .delete(delete_namespace),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/keys",
            get(list_keys),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/values/{key_name}",
            get(values::get).put(values::put).delete(values::delete),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/metadata/{key_name}",
            get(values::metadata),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk",
            axum::routing::put(bulk::update),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk/get",
            post(bulk::get),
        )
        .route(
            "/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk/delete",
            post(bulk::delete),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamespaceBody {
    title: String,
}

#[derive(Serialize)]
struct Namespace {
    id: String,
    title: String,
    supports_url_encoding: bool,
}

impl Namespace {
    fn from_record(
        authority: &super::accounts::AccountAuthority,
        record: &KvNamespaceRecord,
    ) -> Self {
        Self {
            id: authority.public_resource_id(KIND, record.resource.id),
            title: record.resource.name.clone(),
            supports_url_encoding: true,
        }
    }
}

struct NamespaceListQuery {
    page: usize,
    per_page: usize,
    direction: Option<String>,
    order: Option<String>,
}

async fn create_namespace(
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
    let body = match super::storage::json::<NamespaceBody>(request, context.request_id()).await {
        Ok(value) if valid_title(&value.title) => value,
        Ok(_) => return error_response(V4Error::InvalidField("/title"), context.request_id()),
        Err(response) => return response,
    };
    let Some(api) = state.kv_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let created = tokio::task::spawn_blocking(move || {
        let driver = KvResourceDriver::new(api.storage(), api.config().namespace_quota_bytes);
        let outcome = ResourceController::new(api.storage(), api.pins().clone(), driver)
            .create(&CreateResourceRequest {
                account_id,
                kind: BindingKind::KvNamespace,
                name: body.title,
                idempotency_key: request_id.to_string(),
                driver_schema_version: open_compute_storage::KV_SCHEMA_VERSION,
                request_id,
                now_ms: now_ms()?,
            })
            .map_err(|error| V4Error::from(&error))?;
        let resource_id = match outcome {
            CreateResourceOutcome::Applied(result) => result.resource_id,
            CreateResourceOutcome::Replay(_) => return Err(V4Error::Conflict),
        };
        KvNamespaceRepository::new(api.storage().db())
            .get(account_id, resource_id)
            .map_err(|error| V4Error::from(&error))
    })
    .await;
    match created {
        Ok(Ok(record)) => match state.cloudflare_v4_account() {
            Some(authority) => {
                success_response(context, Namespace::from_record(authority, &record))
            }
            None => error_response(V4Error::Unavailable, request_id),
        },
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

async fn list_namespaces(
    State(state): State<HttpState>,
    Path(account_id): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match namespace_list_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if query.page == 0
        || query.per_page == 0
        || query.per_page > 1000
        || query
            .order
            .as_deref()
            .is_some_and(|value| !matches!(value, "id" | "title"))
        || query
            .direction
            .as_deref()
            .is_some_and(|value| !matches!(value, "asc" | "desc"))
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let account_id = match account(&state, &account_id) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.kv_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let records = match KvNamespaceRepository::new(api.storage().db()).list(account_id) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let total = records.len();
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let mut values: Vec<_> = records
        .iter()
        .map(|record| Namespace::from_record(authority, record))
        .collect();
    if query.order.as_deref() == Some("id") {
        values.sort_by(|left, right| left.id.cmp(&right.id));
    } else {
        values.sort_by(|left, right| left.title.cmp(&right.title));
    }
    if query.direction.as_deref() == Some("desc") {
        values.reverse();
    }
    let start = query.page.saturating_sub(1).saturating_mul(query.per_page);
    let values: Vec<_> = values
        .into_iter()
        .skip(start)
        .take(query.per_page)
        .collect();
    let count = values.len();
    paginated_response(
        context,
        values,
        super::V4ResultInfo {
            page: query.page,
            per_page: query.per_page,
            count,
            total_count: total,
            total_pages: total.div_ceil(query.per_page),
        },
    )
}

async fn get_namespace(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, _, record) = match namespace(&state, &request, &account_id, &namespace_id, false)
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let Some(authority) = state.cloudflare_v4_account() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    success_response(context, Namespace::from_record(authority, &record))
}

async fn rename_namespace(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match namespace(&state, &request, &account_id, &namespace_id, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match super::storage::json::<NamespaceBody>(request, context.request_id()).await {
        Ok(value) if valid_title(&value.title) => value,
        Ok(_) => return error_response(V4Error::InvalidField("/title"), context.request_id()),
        Err(response) => return response,
    };
    let Some(api) = state.kv_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let driver = KvResourceDriver::new(api.storage(), api.config().namespace_quota_bytes);
        ResourceController::new(api.storage(), api.pins().clone(), driver)
            .rename(
                account_id,
                record.resource.id,
                &body.title,
                request_id,
                now_ms()?,
            )
            .map_err(|error| V4Error::from(&error))?;
        KvNamespaceRepository::new(api.storage().db())
            .get(account_id, record.resource.id)
            .map_err(|error| V4Error::from(&error))
    })
    .await;
    match result {
        Ok(Ok(record)) => match state.cloudflare_v4_account() {
            Some(authority) => {
                success_response(context, Namespace::from_record(authority, &record))
            }
            None => error_response(V4Error::Unavailable, request_id),
        },
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

async fn delete_namespace(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match namespace(&state, &request, &account_id, &namespace_id, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let Some(api) = state.kv_api().cloned() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let request_id = context.request_id();
    let driver = KvResourceDriver::new(api.storage(), api.config().namespace_quota_bytes);
    let controller = ResourceController::new(api.storage(), api.pins().clone(), driver);
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    match controller
        .delete(
            account_id,
            record.resource.id,
            request_id,
            now,
            api.delete_drain_timeout(),
        )
        .await
    {
        Ok(()) => success_response(context, ()),
        Err(error) => error_response(V4Error::from(&error), request_id),
    }
}

struct KeyListQuery {
    prefix: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct KeyInfo {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

async fn list_keys(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match namespace(&state, &request, &account_id, &namespace_id, false) {
            Ok(value) => value,
            Err(response) => return response,
        };
    let query = match key_list_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let limit = query.limit.unwrap_or(1000);
    if limit == 0 || limit > KV_MAX_LIST_LIMIT {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let Some(api) = state.kv_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match management_binding(
        api.storage(),
        account_id,
        record.resource.id,
        BindingKind::KvNamespace,
    ) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let result = api.executor().execute(
        &binding,
        KvCommand::List {
            prefix: query.prefix.unwrap_or_default(),
            limit,
            cursor: query.cursor,
        },
    );
    match result {
        Ok(KvCommandResult::List { rows, cursor, .. }) => {
            let keys: Result<Vec<_>, V4Error> = rows
                .into_iter()
                .map(|row| {
                    Ok(KeyInfo {
                        name: String::from_utf8(row.key).map_err(|_| V4Error::Internal)?,
                        expiration: row
                            .expires_at_ms
                            .map(|value| u64::try_from(value / 1000).map_err(|_| V4Error::Internal))
                            .transpose()?,
                        metadata: row
                            .metadata_json
                            .as_deref()
                            .map(|value| {
                                serde_json::from_slice(value).map_err(|_| V4Error::Internal)
                            })
                            .transpose()?,
                    })
                })
                .collect();
            match keys {
                Ok(keys) => cursor_response(context.request_id(), keys, cursor),
                Err(error) => error_response(error, context.request_id()),
            }
        }
        Ok(_) => error_response(V4Error::Internal, context.request_id()),
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

fn namespace(
    state: &HttpState,
    request: &Request,
    account_id: &str,
    namespace_id: &str,
    write: bool,
) -> Result<
    (
        super::V4RequestContext,
        open_compute_core::AccountId,
        KvNamespaceRecord,
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
    let account_id =
        account(state, account_id).map_err(|error| error_response(error, context.request_id()))?;
    let api = state
        .kv_api()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    let records = KvNamespaceRepository::new(api.storage().db())
        .list(account_id)
        .map_err(|error| error_response(V4Error::from(&error), context.request_id()))?;
    let authority = state
        .cloudflare_v4_account()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    let resource_id = resolve_resource_id(authority, KIND, namespace_id, &records, |record| {
        record.resource.id
    })
    .map_err(|error| error_response(error, context.request_id()))?;
    let record = records
        .into_iter()
        .find(|record| record.resource.id == resource_id)
        .ok_or_else(|| error_response(V4Error::NotFound, context.request_id()))?;
    Ok((context, account_id, record))
}

#[derive(Serialize)]
struct CursorEnvelope<T> {
    success: bool,
    result: T,
    result_info: CursorInfo,
    errors: [Value; 0],
    messages: [Value; 0],
}

#[derive(Serialize)]
struct CursorInfo {
    count: usize,
    cursor: String,
}

fn cursor_response<T: Serialize>(
    request_id: RequestId,
    values: Vec<T>,
    cursor: Option<String>,
) -> Response {
    let count = values.len();
    let mut response = Json(CursorEnvelope {
        success: true,
        result: values,
        result_info: CursorInfo {
            count,
            cursor: cursor.unwrap_or_default(),
        },
        errors: [],
        messages: [],
    })
    .into_response();
    attach_request_id(&mut response, request_id);
    response
}

fn attach_request_id(response: &mut Response, request_id: RequestId) {
    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
}

fn valid_title(title: &str) -> bool {
    !title.is_empty() && title.len() <= 512
}

fn namespace_list_query(request: &Request) -> Result<NamespaceListQuery, V4Error> {
    let mut values = strict_query(request)?;
    let page = take_usize(&mut values, "page")?.unwrap_or(1);
    let per_page = take_usize(&mut values, "per_page")?.unwrap_or(20);
    let direction = values.remove("direction");
    let order = values.remove("order");
    if !values.is_empty() {
        return Err(V4Error::InvalidRequest);
    }
    Ok(NamespaceListQuery {
        page,
        per_page,
        direction,
        order,
    })
}

fn key_list_query(request: &Request) -> Result<KeyListQuery, V4Error> {
    let mut values = strict_query(request)?;
    let prefix = values.remove("prefix");
    let cursor = values.remove("cursor");
    let limit = values
        .remove("limit")
        .map(|value| value.parse().map_err(|_| V4Error::InvalidRequest))
        .transpose()?;
    if !values.is_empty() {
        return Err(V4Error::InvalidRequest);
    }
    Ok(KeyListQuery {
        prefix,
        cursor,
        limit,
    })
}

fn take_usize(values: &mut BTreeMap<String, String>, key: &str) -> Result<Option<usize>, V4Error> {
    values
        .remove(key)
        .map(|value| value.parse().map_err(|_| V4Error::InvalidRequest))
        .transpose()
}
