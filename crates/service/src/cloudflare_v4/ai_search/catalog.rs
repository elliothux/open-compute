//! AI Search item and reindex-job catalog routes.

use super::*;
use crate::cloudflare_v4::storage::{json, now_ms, require_no_query};
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::json;

pub(super) async fn list_jobs(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    page_call(
        state,
        public_account,
        namespace,
        instance,
        request,
        "jobs.list",
        50,
    )
    .await
}

pub(super) async fn create_job(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let payload = match json::<Value>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if payload
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|value| value.chars().count() > 255)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    domain_response(
        context,
        call(
            &api,
            account,
            &namespace,
            context.request_id(),
            "jobs.create",
            Some(&instance),
            payload,
        )
        .await,
        None,
    )
}

pub(super) async fn get_job(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, job_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    id_call(
        state,
        public_account,
        namespace,
        instance,
        job_id,
        request,
        "job.info",
        V4Permission::Read,
    )
    .await
}

pub(super) async fn cancel_job(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, job_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&job_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Cancel {
        action: String,
    }
    match json::<Cancel>(request, context.request_id()).await {
        Ok(value) if value.action == "cancel" => {}
        Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        Err(response) => return response,
    }
    domain_response(
        context,
        call(
            &api,
            account,
            &namespace,
            context.request_id(),
            "job.cancel",
            Some(&instance),
            json!({"jobId": job_id}),
        )
        .await,
        None,
    )
}

pub(super) async fn job_logs(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, job_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&job_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    let query = match query(&request, &["page", "per_page"]) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (page_number, per_page) = match page(&query, 20, 0, 500) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    domain_response(
        context,
        call(
            &api,
            account,
            &namespace,
            context.request_id(),
            "job.logs",
            Some(&instance),
            json!({"jobId": job_id, "params": {"page": page_number, "per_page": per_page}}),
        )
        .await,
        None,
    )
}

pub(super) async fn list_items(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    let query = match query(
        &request,
        &[
            "page",
            "per_page",
            "search",
            "sort_by",
            "status",
            "source",
            "metadata_filter",
            "item_id",
            "key",
        ],
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (page_number, per_page) = match page(&query, 20, 0, 50) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if query.get("search").is_some_and(|value| value.len() > 256)
        || query
            .get("metadata_filter")
            .is_some_and(|value| value.len() > 2_048)
        || query.get("item_id").is_some_and(|value| value.len() > 64)
        || query.get("key").is_some_and(|value| value.len() > 1_024)
        || query.get("source").is_some_and(|value| value.len() > 512)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if query
        .get("sort_by")
        .is_some_and(|value| !matches!(value.as_str(), "status" | "modified_at"))
        || query.get("status").is_some_and(|value| {
            !matches!(
                value.as_str(),
                "queued" | "running" | "completed" | "error" | "skipped" | "outdated"
            )
        })
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let payload = json!({
        "page": page_number,
        "per_page": per_page,
        "search": query.get("search"),
        "sort_by": query.get("sort_by"),
        "status": query.get("status"),
        "source": query.get("source"),
        "metadata_filter": query.get("metadata_filter"),
        "item_id": query.get("item_id"),
        "key": query.get("key"),
    });
    domain_response(
        context,
        call(
            &api,
            account,
            &namespace,
            context.request_id(),
            "items.list",
            Some(&instance),
            payload,
        )
        .await,
        Some(&namespace),
    )
}

pub(super) async fn get_item(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, item_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    id_call(
        state,
        public_account,
        namespace,
        instance,
        item_id,
        request,
        "item.info",
        V4Permission::Read,
    )
    .await
}

pub(super) async fn sync_item(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, item_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    item_mutation(
        state,
        public_account,
        namespace,
        instance,
        item_id,
        request,
        "item.sync",
        true,
    )
    .await
}

pub(super) async fn delete_item(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, item_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    item_mutation(
        state,
        public_account,
        namespace,
        instance,
        item_id,
        request,
        "items.delete",
        false,
    )
    .await
}

pub(super) async fn item_logs(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, item_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&item_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    let query = match query(&request, &["limit", "cursor"]) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let limit = match parse_u32(&query, "limit", 50) {
        Ok(value @ 1..=100) => value,
        _ => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    let now = match now_ms() {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let after = match query.get("cursor") {
        Some(token) => match cursor::open(
            api.storage(),
            token,
            account,
            &namespace,
            &instance,
            &item_id,
            limit,
            now,
        ) {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, context.request_id()),
        },
        None => None,
    };
    let mut value = match call(
        &api,
        account,
        &namespace,
        context.request_id(),
        "item.logs",
        Some(&instance),
        json!({"itemId": item_id, "params": {"limit": limit, "cursor": after}}),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let raw_cursor = value
        .get_mut("result_info")
        .and_then(Value::as_object_mut)
        .and_then(|info| info.remove("cursor"));
    let sealed = match raw_cursor {
        Some(Value::String(after)) => {
            let expires = match now.checked_add(CURSOR_LIFETIME_MS) {
                Some(value) => value,
                None => return error_response(V4Error::Internal, context.request_id()),
            };
            match cursor::seal(
                api.storage(),
                account,
                &namespace,
                &instance,
                &item_id,
                limit,
                &after,
                expires,
            ) {
                Ok(value) => Value::String(value),
                Err(error) => return error_response(error, context.request_id()),
            }
        }
        Some(Value::Null) | None => Value::Null,
        _ => return error_response(V4Error::Internal, context.request_id()),
    };
    if let Some(info) = value.get_mut("result_info").and_then(Value::as_object_mut) {
        info.insert("cursor".to_owned(), sealed);
    }
    respond(context, value)
}

pub(super) async fn item_chunks(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, item_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&item_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    let query = match query(&request, &["limit", "offset"]) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let limit = match parse_u32(&query, "limit", 20) {
        Ok(value @ 1..=100) => value,
        _ => return error_response(V4Error::InvalidRequest, context.request_id()),
    };
    let offset = match parse_u64(&query, "offset", 0) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    domain_response(
        context,
        call(
            &api,
            account,
            &namespace,
            context.request_id(),
            "item.chunks",
            Some(&instance),
            json!({"itemId": item_id, "params": {"limit": limit, "offset": offset}}),
        )
        .await,
        None,
    )
}

async fn page_call(
    state: HttpState,
    public_account: String,
    namespace: String,
    instance: String,
    request: Request,
    operation: &'static str,
    maximum: u32,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    let query = match query(&request, &["page", "per_page"]) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (page_number, per_page) = match page(&query, 20, 0, maximum) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    domain_response(
        context,
        call(
            &api,
            account,
            &namespace,
            context.request_id(),
            operation,
            Some(&instance),
            json!({"page": page_number, "per_page": per_page}),
        )
        .await,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
async fn id_call(
    state: HttpState,
    public_account: String,
    namespace: String,
    instance: String,
    object_id: String,
    request: Request,
    operation: &'static str,
    permission: V4Permission,
) -> Response {
    let (context, account, api) = match authenticated(&state, &request, permission, &public_account)
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&object_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let field = if operation.starts_with("job.") {
        "jobId"
    } else {
        "itemId"
    };
    let mut payload = serde_json::Map::new();
    payload.insert(field.to_owned(), Value::String(object_id));
    match call(
        &api,
        account,
        &namespace,
        context.request_id(),
        operation,
        Some(&instance),
        Value::Object(payload),
    )
    .await
    {
        Ok(mut value) => {
            if operation.starts_with("item.") {
                item_namespace(&mut value, &namespace);
            }
            respond(context, value)
        }
        Err(error) => error_response(error, context.request_id()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn item_mutation(
    state: HttpState,
    public_account: String,
    namespace: String,
    instance: String,
    item_id: String,
    request: Request,
    operation: &'static str,
    body: bool,
) -> Response {
    let (context, account, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&item_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    if body {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Sync {
            next_action: String,
            #[serde(default)]
            wait_for_completion: bool,
        }
        match json::<Sync>(request, context.request_id()).await {
            Ok(value) if value.next_action == "INDEX" => {
                let _wait_for_completion = value.wait_for_completion;
            }
            Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
            Err(response) => return response,
        }
    }
    let result = call(
        &api,
        account,
        &namespace,
        context.request_id(),
        operation,
        Some(&instance),
        json!({"itemId": item_id}),
    )
    .await;
    domain_response(context, result, Some(&namespace))
}

fn domain_response(
    context: V4RequestContext,
    result: Result<Value, V4Error>,
    namespace: Option<&str>,
) -> Response {
    match result {
        Ok(Value::Null) => success_response(context, json!({})),
        Ok(mut value) => {
            if let Some(namespace) = namespace {
                item_namespace(&mut value, namespace);
            }
            respond(context, value)
        }
        Err(error) => error_response(error, context.request_id()),
    }
}
