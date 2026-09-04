//! AI Search built-in instance lifecycle and query routes.

use super::*;
use crate::cloudflare_v4::storage::{json, require_no_query};
use axum::extract::{Path, State};
use serde_json::json;

const UNSUPPORTED_INSTANCE_FIELDS: &[&str] = &[
    "ai_gateway_id",
    "cache",
    "cache_threshold",
    "cache_ttl",
    "hybrid_search_enabled",
    "public_endpoint_params",
    "source",
    "source_params",
    "summarization",
    "summarization_model",
    "sync_interval",
    "system_prompt_ai_search",
    "system_prompt_index_summarization",
    "system_prompt_rewrite_query",
    "token_id",
];

pub(super) async fn list(
    State(state): State<HttpState>,
    Path((public_account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if !valid_namespace(&namespace) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    let query = match query(
        &request,
        &[
            "page",
            "per_page",
            "search",
            "namespace",
            "order_by",
            "order_by_direction",
        ],
    ) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (page_number, per_page) = match page(&query, 20, 1, 100) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if query
        .get("namespace")
        .is_some_and(|requested| requested != &namespace)
    {
        return result_info_response(
            context,
            Vec::<Value>::new(),
            json!({"page": page_number, "per_page": per_page, "count": 0, "total_count": 0}),
        );
    }
    if query
        .get("search")
        .is_some_and(|value| value.chars().count() > 64)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let order_by = query.get("order_by").map_or("created_at", String::as_str);
    let direction = query
        .get("order_by_direction")
        .map_or("desc", String::as_str);
    if order_by != "created_at" || !matches!(direction, "asc" | "desc") {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let payload = json!({
        "page": page_number,
        "per_page": per_page,
        "search": query.get("search"),
        "order_by": order_by,
        "order_by_direction": direction,
    });
    match call(
        &api,
        account,
        &namespace,
        context.request_id(),
        "namespace.list",
        None,
        payload,
    )
    .await
    {
        Ok(value) => respond(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

pub(super) async fn create(
    State(state): State<HttpState>,
    Path((public_account, namespace)): Path<(String, String)>,
    request: Request,
) -> Response {
    mutation(
        state,
        public_account,
        namespace,
        None,
        request,
        "namespace.create",
        false,
    )
    .await
}

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    read(
        state,
        public_account,
        namespace,
        instance,
        request,
        "instance.info",
    )
    .await
}

pub(super) async fn stats(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    read(
        state,
        public_account,
        namespace,
        instance,
        request,
        "instance.stats",
    )
    .await
}

pub(super) async fn update(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    mutation(
        state,
        public_account,
        namespace,
        Some(instance),
        request,
        "instance.update",
        false,
    )
    .await
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    mutation(
        state,
        public_account,
        namespace,
        Some(instance.clone()),
        request,
        "namespace.delete",
        true,
    )
    .await
}

pub(super) async fn search(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    query_call(
        state,
        public_account,
        namespace,
        instance,
        request,
        "instance.search",
    )
    .await
}

pub(super) async fn chat(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    query_call(
        state,
        public_account,
        namespace,
        instance,
        request,
        "instance.chatCompletions",
    )
    .await
}

async fn read(
    state: HttpState,
    public_account: String,
    namespace: String,
    instance: String,
    request: Request,
    operation: &'static str,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    match call(
        &api,
        account,
        &namespace,
        context.request_id(),
        operation,
        Some(&instance),
        json!({}),
    )
    .await
    {
        Ok(value) => respond(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn mutation(
    state: HttpState,
    public_account: String,
    namespace: String,
    instance: Option<String>,
    request: Request,
    operation: &'static str,
    delete: bool,
) -> Response {
    let (context, account, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if !valid_namespace(&namespace)
        || instance
            .as_ref()
            .is_some_and(|instance| !valid_instance(instance))
    {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let payload = if delete {
        json!({"instance": instance.as_deref()})
    } else {
        match json::<Value>(request, context.request_id()).await {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        }
    };
    if !delete && let Err(error) = reject_unsupported_fields(&payload, UNSUPPORTED_INSTANCE_FIELDS)
    {
        return error_response(error, context.request_id());
    }
    match call(
        &api,
        account,
        &namespace,
        context.request_id(),
        operation,
        if delete { None } else { instance.as_deref() },
        payload,
    )
    .await
    {
        Ok(Value::Null) if delete => success_response(context, json!({})),
        Ok(value) => respond(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn query_call(
    state: HttpState,
    public_account: String,
    namespace: String,
    instance: String,
    request: Request,
    operation: &'static str,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let payload = match json::<Value>(request, context.request_id()).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = namespaces::reject_query_features(&payload) {
        return error_response(error, context.request_id());
    }
    if operation.ends_with("chatCompletions")
        && payload.get("stream").and_then(Value::as_bool) == Some(true)
    {
        return match stream(
            &api,
            account,
            &namespace,
            context.request_id(),
            operation,
            Some(&instance),
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
        account,
        &namespace,
        context.request_id(),
        operation,
        Some(&instance),
        payload,
    )
    .await
    {
        Ok(value) => respond(context, value),
        Err(error) => error_response(error, context.request_id()),
    }
}
