//! AI Search namespace lifecycle and multi-instance query routes.

use super::*;
use crate::cloudflare_v4::storage::{iso_timestamp, json, now_ms, require_no_query};
use axum::extract::{Path, State};
use open_compute_core::{BindingKind, ResourceState};
use open_compute_storage::{AI_SEARCH_SCHEMA_VERSION, AiSearchCatalog, ResourceRepository};
use open_compute_workers::{
    AiSearchNamespaceResourceDriver, CreateResourceOutcome, CreateResourceRequest,
    ResourceController,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateNamespace {
    name: String,
    description: Option<String>,
}

pub(super) async fn create(
    State(state): State<HttpState>,
    Path(public_account): Path<String>,
    request: Request,
) -> Response {
    let (context, account_id, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let raw = match json::<Value>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if raw
        .as_object()
        .is_some_and(|object| object.contains_key("public_endpoint_params"))
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let body: CreateNamespace = match serde_json::from_value(raw) {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    if !valid_namespace(&body.name)
        || body
            .description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 256)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let driver =
            AiSearchNamespaceResourceDriver::new(api.storage()).with_description(body.description);
        match ResourceController::new(api.storage(), api.pins().clone(), driver).create(
            &CreateResourceRequest {
                account_id,
                kind: BindingKind::AiSearchNamespace,
                name: body.name,
                idempotency_key: request_id.to_string(),
                driver_schema_version: AI_SEARCH_SCHEMA_VERSION,
                request_id,
                now_ms: now_ms()?,
            },
        ) {
            Ok(CreateResourceOutcome::Applied(result)) => AiSearchCatalog::new(api.storage().db())
                .get_namespace(account_id, result.resource_id)
                .map_err(|error| V4Error::from(&error)),
            Ok(CreateResourceOutcome::Replay(_)) => Err(V4Error::Conflict),
            Err(error) => Err(V4Error::from(&error)),
        }
    })
    .await;
    match result {
        Ok(Ok(record)) => match value(&record) {
            Ok(value) => success_response(context, value),
            Err(error) => error_response(error, request_id),
        },
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(V4Error::Internal, request_id),
    }
}

pub(super) async fn list(
    State(state): State<HttpState>,
    Path(public_account): Path<String>,
    request: Request,
) -> Response {
    let (context, account_id, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    let query = match query(&request, &["page", "per_page", "search"]) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (page_number, per_page) = match page(&query, 20, 1, 100) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let search = query.get("search").map(|value| value.to_lowercase());
    if search
        .as_ref()
        .is_some_and(|value| value.chars().count() > 256)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let resources = match ResourceRepository::new(api.storage().db())
        .list(account_id, Some(BindingKind::AiSearchNamespace))
    {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let catalog = AiSearchCatalog::new(api.storage().db());
    let mut records = match resources
        .into_iter()
        .filter(|resource| resource.state == ResourceState::Ready)
        .map(|resource| catalog.get_namespace(account_id, resource.id))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    records.retain(|record| {
        search.as_ref().is_none_or(|search| {
            record.resource.name.to_lowercase().contains(search)
                || record
                    .description
                    .as_ref()
                    .is_some_and(|description| description.to_lowercase().contains(search))
        })
    });
    records.sort_by(|left, right| left.resource.name.cmp(&right.resource.name));
    let total = records.len();
    let (start, end) = match bounds(page_number, per_page, total) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = match records[start..end]
        .iter()
        .map(value)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    result_info_response(
        context,
        result,
        json!({"page": page_number, "per_page": per_page, "count": end - start, "total_count": total}),
    )
}

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((public_account, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    namespace_read(state, public_account, name, request).await
}

pub(super) async fn update(
    State(state): State<HttpState>,
    Path((public_account, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if !valid_namespace(&name) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let raw = match json::<Value>(request, context.request_id()).await {
        Ok(Value::Object(object)) => object,
        Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        Err(response) => return response.into_response(),
    };
    if raw
        .keys()
        .any(|key| !matches!(key.as_str(), "description" | "public_endpoint_params"))
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if raw.contains_key("public_endpoint_params") {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let record = match find(&api, account_id, &name) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let description = match raw.get("description") {
        None => record.description.as_deref(),
        Some(Value::Null) => None,
        Some(Value::String(value)) if value.chars().count() <= 256 => Some(value.as_str()),
        Some(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    match AiSearchCatalog::new(api.storage().db()).update_namespace_description(
        account_id,
        record.resource.id,
        description,
    ) {
        Ok(record) => match value(&record) {
            Ok(value) => success_response(context, value),
            Err(error) => error_response(error, context.request_id()),
        },
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path((public_account, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let record = match find(&api, account_id, &name) {
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
        AiSearchNamespaceResourceDriver::new(api.storage()),
    )
    .delete(
        account_id,
        record.resource.id,
        context.request_id(),
        now,
        api.delete_drain_timeout(),
    )
    .await;
    match result {
        Ok(()) => success_response(context, json!({})),
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

pub(super) async fn search(
    State(state): State<HttpState>,
    Path((public_account, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    query_call(state, public_account, name, request, "namespace.search").await
}

pub(super) async fn chat(
    State(state): State<HttpState>,
    Path((public_account, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    query_call(
        state,
        public_account,
        name,
        request,
        "namespace.chatCompletions",
    )
    .await
}

async fn query_call(
    state: HttpState,
    public_account: String,
    name: String,
    request: Request,
    operation: &'static str,
) -> Response {
    let (context, account_id, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if !valid_namespace(&name) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let payload = match json::<Value>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = reject_query_features(&payload) {
        return error_response(error, context.request_id());
    }
    if operation.ends_with("chatCompletions")
        && payload.get("stream").and_then(Value::as_bool) == Some(true)
    {
        return match stream(
            &api,
            account_id,
            &name,
            context.request_id(),
            operation,
            None,
            payload,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => error_response(error, context.request_id()),
        };
    }
    match call(
        &api,
        account_id,
        &name,
        context.request_id(),
        operation,
        None,
        payload,
    )
    .await
    {
        Ok(value) => respond(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn namespace_read(
    state: HttpState,
    public_account: String,
    name: String,
    request: Request,
) -> Response {
    let (context, account_id, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    match find(&api, account_id, &name).and_then(|record| value(&record)) {
        Ok(value) => success_response(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

pub(super) fn find(
    api: &SearchApiState,
    account: AccountId,
    name: &str,
) -> Result<open_compute_storage::AiSearchNamespaceRecord, V4Error> {
    if !valid_namespace(name) {
        return Err(V4Error::NotFound);
    }
    let resource = ResourceRepository::new(api.storage().db())
        .list(account, Some(BindingKind::AiSearchNamespace))
        .map_err(|error| V4Error::from(&error))?
        .into_iter()
        .find(|resource| resource.name == name && resource.state == ResourceState::Ready)
        .ok_or(V4Error::NotFound)?;
    AiSearchCatalog::new(api.storage().db())
        .get_namespace(account, resource.id)
        .map_err(|error| V4Error::from(&error))
}

fn value(record: &open_compute_storage::AiSearchNamespaceRecord) -> Result<Value, V4Error> {
    Ok(json!({
        "name": record.resource.name,
        "description": record.description,
        "created_at": iso_timestamp(record.resource.created_at_ms)?,
        "public_endpoint_id": Value::Null,
        "public_endpoint_params": Value::Null,
    }))
}

fn bounds(page: u64, per_page: u32, total: usize) -> Result<(usize, usize), V4Error> {
    let start = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(u64::from(per_page)))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(V4Error::InvalidRequest)?;
    Ok((
        start.min(total),
        start
            .saturating_add(usize::try_from(per_page).map_err(|_| V4Error::InvalidRequest)?)
            .min(total),
    ))
}

pub(super) fn reject_query_features(payload: &Value) -> Result<(), V4Error> {
    let options = payload.get("ai_search_options").and_then(Value::as_object);
    if options.is_some_and(|options| options.contains_key("cache"))
        || options
            .and_then(|options| options.get("query_rewrite"))
            .and_then(Value::as_object)
            .is_some_and(|rewrite| rewrite.contains_key("rewrite_prompt"))
    {
        Err(V4Error::Unsupported)
    } else {
        Ok(())
    }
}
