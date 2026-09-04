//! Official Cloudflare AI Search management adapter.

mod catalog;
mod cursor;
mod instances;
mod items;
mod namespaces;
mod tokens;

use super::storage::{account, context, strict_query};
use super::{
    HttpError, V4Error, V4Permission, V4RequestContext, error_response, result_info_response,
    success_response,
};
use crate::http::HttpState;
use crate::search_api::SearchApiState;
use axum::Router;
use axum::extract::Request;
use axum::response::Response;
use axum::routing::{get, post};
use open_compute_core::{AccountId, RequestId};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

const CURSOR_LIFETIME_MS: i64 = 15 * 60 * 1_000;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/ai-search/tokens",
            get(tokens::list),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces",
            get(namespaces::list).post(namespaces::create),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}",
            get(namespaces::get)
                .put(namespaces::update)
                .delete(namespaces::delete),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/search",
            post(namespaces::search),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/chat/completions",
            post(namespaces::chat),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances",
            get(instances::list).post(instances::create),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}",
            get(instances::get)
                .put(instances::update)
                .delete(instances::delete),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/stats",
            get(instances::stats),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/search",
            post(instances::search),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/chat/completions",
            post(instances::chat),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/jobs",
            get(catalog::list_jobs).post(catalog::create_job),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/jobs/{job_id}",
            get(catalog::get_job).patch(catalog::cancel_job),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/jobs/{job_id}/logs",
            get(catalog::job_logs),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/items",
            get(catalog::list_items)
                .post(items::upload)
                .put(items::index_by_key),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/items/{item_id}",
            get(catalog::get_item)
                .patch(catalog::sync_item)
                .delete(catalog::delete_item),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/items/{item_id}/download",
            get(items::download),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/items/{item_id}/logs",
            get(catalog::item_logs),
        )
        .route(
            "/accounts/{account_id}/ai-search/namespaces/{name}/instances/{id}/items/{item_id}/chunks",
            get(catalog::item_chunks),
        )
}

fn authenticated(
    state: &HttpState,
    request: &Request,
    permission: V4Permission,
    public_account: &str,
) -> Result<(V4RequestContext, AccountId, Arc<SearchApiState>), HttpError> {
    let context = context(request, permission)?;
    let account = account(state, public_account)
        .map_err(|error| error_response(error, context.request_id()))?;
    let api = state
        .search_api()
        .cloned()
        .ok_or_else(|| error_response(V4Error::Unavailable, context.request_id()))?;
    Ok((context, account, api))
}

async fn call(
    api: &SearchApiState,
    account: AccountId,
    namespace: &str,
    request_id: RequestId,
    operation: &str,
    instance: Option<&str>,
    payload: Value,
) -> Result<Value, V4Error> {
    let service = api.ai_search().ok_or(V4Error::Unavailable)?;
    service
        .official_call(account, namespace, request_id, operation, instance, payload)
        .await
        .map_err(|error| V4Error::from(&error))
}

async fn stream(
    api: &SearchApiState,
    account: AccountId,
    namespace: &str,
    request_id: RequestId,
    operation: &str,
    instance: Option<&str>,
    payload: Value,
) -> Result<Response, V4Error> {
    let service = api.ai_search().ok_or(V4Error::Unavailable)?;
    let mut response = service
        .official_stream(account, namespace, request_id, operation, instance, payload)
        .await
        .map_err(|error| V4Error::from(&error))?;
    let request_id = axum::http::HeaderValue::from_str(&request_id.to_string())
        .map_err(|_| V4Error::Internal)?;
    response
        .headers_mut()
        .insert(crate::http::REQUEST_ID_HEADER, request_id);
    Ok(response)
}

fn respond(context: V4RequestContext, value: Value) -> Response {
    match value {
        Value::Object(mut object)
            if object.contains_key("result") && object.contains_key("result_info") =>
        {
            match (object.remove("result"), object.remove("result_info")) {
                (Some(result), Some(result_info)) => {
                    result_info_response(context, result, result_info)
                }
                _ => error_response(V4Error::Internal, context.request_id()),
            }
        }
        value => success_response(context, value),
    }
}

fn query(request: &Request, allowed: &[&str]) -> Result<BTreeMap<String, String>, V4Error> {
    let query = strict_query(request)?;
    if query
        .keys()
        .all(|key| allowed.iter().any(|allowed| key == allowed))
    {
        Ok(query)
    } else {
        Err(V4Error::InvalidRequest)
    }
}

fn page(
    query: &BTreeMap<String, String>,
    default_per_page: u32,
    minimum_per_page: u32,
    maximum_per_page: u32,
) -> Result<(u64, u32), V4Error> {
    let page = parse_u64(query, "page", 1)?;
    let per_page = parse_u32(query, "per_page", default_per_page)?;
    if page == 0 || !(minimum_per_page..=maximum_per_page).contains(&per_page) {
        return Err(V4Error::InvalidRequest);
    }
    Ok((page, per_page))
}

fn parse_u32(query: &BTreeMap<String, String>, name: &str, default: u32) -> Result<u32, V4Error> {
    query.get(name).map_or(Ok(default), |value| {
        value.parse().map_err(|_| V4Error::InvalidRequest)
    })
}

fn parse_u64(query: &BTreeMap<String, String>, name: &str, default: u64) -> Result<u64, V4Error> {
    query.get(name).map_or(Ok(default), |value| {
        value.parse().map_err(|_| V4Error::InvalidRequest)
    })
}

fn valid_namespace(value: &str) -> bool {
    (1..=28).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn valid_instance(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
        && value.as_bytes().first().is_some_and(|byte| *byte != b'-')
        && value.as_bytes().last().is_some_and(|byte| *byte != b'-')
        && !value.as_bytes().windows(2).any(|pair| pair == b"--")
}

fn valid_object_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !matches!(value, "." | "..")
        && !value.chars().any(char::is_control)
}

fn reject_unsupported_fields(payload: &Value, fields: &[&str]) -> Result<(), V4Error> {
    let object = payload.as_object().ok_or(V4Error::InvalidRequest)?;
    if object.keys().any(|key| fields.contains(&key.as_str())) {
        Err(V4Error::Unsupported)
    } else {
        Ok(())
    }
}

fn item_namespace(value: &mut Value, namespace: &str) {
    match value {
        Value::Object(object) => {
            if object.contains_key("id")
                && object.contains_key("key")
                && object.contains_key("status")
            {
                object.insert("namespace".to_owned(), Value::String(namespace.to_owned()));
            }
            if let Some(result) = object.get_mut("result") {
                item_namespace(result, namespace);
            }
        }
        Value::Array(values) => {
            for value in values {
                item_namespace(value, namespace);
            }
        }
        _ => {}
    }
}
