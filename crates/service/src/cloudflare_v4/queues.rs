//! Cloudflare Queues catalog and consumer adapters.

#[path = "queues/consumers.rs"]
mod consumers;

use super::wire::V4OfficialError;
use super::{
    V4Error, V4Permission, V4RequestContext, V4ResultInfo, error_response, paginated_response,
    request_context, success_response,
};
use crate::http::HttpState;
use crate::queue_api::{QueueApiState, now_ms};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use axum::routing::get;
use open_compute_core::{AccountId, ErrorCode, PlatformError};
use open_compute_storage::{QueueConfig, QueueConsumerRepository, QueueRecord, QueueRepository};
use open_compute_workers::{CreateQueueOutcome, CreateQueueRequest, QueueController};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::form_urlencoded;

const MAX_JSON_BODY: usize = 16 * 1024;

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/queues",
            get(list_queues).post(create_queue),
        )
        .route(
            "/accounts/{account_id}/queues/{queue_id}",
            get(get_queue).put(update_queue).delete(delete_queue),
        )
        .merge(consumers::router())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateQueueBody {
    queue_name: String,
    settings: Option<QueueSettingsBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateQueueBody {
    queue_name: Option<String>,
    settings: Option<QueueSettingsBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueSettingsBody {
    delivery_delay: Option<u32>,
    delivery_paused: Option<bool>,
    message_retention_period: Option<u32>,
}

async fn list_queues(
    State(state): State<HttpState>,
    Path(account_public): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = match ListQuery::parse(request.uri().query()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if let Err(response) = bodyless(request, context).await {
        return response;
    }
    let (api, authority, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let storage = api.storage().clone();
    let authority = authority.clone();
    let result = tokio::task::spawn_blocking(move || {
        let all = QueueRepository::new(storage.db()).list_account(account_id)?;
        let filtered = all
            .into_iter()
            .filter(|queue| query.name.as_ref().is_none_or(|name| &queue.name == name))
            .collect::<Vec<_>>();
        let total = filtered.len();
        let start = query.page.saturating_sub(1).saturating_mul(query.per_page);
        let page = if start >= total {
            Vec::new()
        } else {
            let end = start.saturating_add(query.per_page).min(total);
            filtered[start..end].to_vec()
        };
        let mut result = Vec::with_capacity(page.len());
        for queue in page {
            result.push(queue_response(&authority, &storage, queue)?);
        }
        Ok::<_, PlatformError>((result, total))
    })
    .await;
    match result {
        Ok(Ok((queues, total))) => {
            let count = queues.len();
            paginated_response(
                context,
                queues,
                V4ResultInfo {
                    page: query.page,
                    per_page: query.per_page,
                    count,
                    total_count: total,
                    total_pages: total.div_ceil(query.per_page),
                },
            )
        }
        Ok(Err(error)) => platform_error(&error, context),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

async fn create_queue(
    State(state): State<HttpState>,
    Path(account_public): Path<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let body = match json_body::<CreateQueueBody>(request, context).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (api, authority, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = api.clone();
    let authority = authority.clone();
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let now = now_ms()?;
        let settings = body.settings.unwrap_or_default();
        let outcome = QueueController::new(api.storage(), api.scheduler().clone()).create(
            &CreateQueueRequest {
                account_id,
                name: body.queue_name,
                config: QueueConfig {
                    delivery_delay_seconds: settings.delivery_delay.unwrap_or_default(),
                    retention_seconds: settings
                        .message_retention_period
                        .unwrap_or(QueueConfig::default().retention_seconds),
                    max_backlog_bytes: api.default_max_backlog_bytes(),
                    ..QueueConfig::default()
                },
                idempotency_key: request_id.to_string(),
                request_id,
                now_ms: now,
            },
        )?;
        let queue = match outcome {
            CreateQueueOutcome::Applied(value) => value.queue,
            CreateQueueOutcome::Replay(_) => return Err(internal()),
        };
        let queue = if settings.delivery_paused.unwrap_or(false) {
            api.set_delivery_paused(account_id, queue.id, true, request_id, now)?
        } else {
            queue
        };
        queue_response(&authority, api.storage(), queue)
    })
    .await;
    result_response(result, context)
}

async fn get_queue(
    State(state): State<HttpState>,
    Path((account_public, queue_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if let Err(response) = bodyless(request, context).await {
        return response;
    }
    let (api, authority, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let storage = api.storage().clone();
    let authority = authority.clone();
    let result = tokio::task::spawn_blocking(move || {
        let queue = resolve_queue(&authority, &storage, account_id, &queue_public)?;
        queue_response(&authority, &storage, queue)
    })
    .await;
    result_response(result, context)
}

async fn update_queue(
    State(state): State<HttpState>,
    Path((account_public, queue_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let body = match json_body::<UpdateQueueBody>(request, context).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (api, authority, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = api.clone();
    let authority = authority.clone();
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let mut queue = resolve_queue(&authority, api.storage(), account_id, &queue_public)?;
        let now = now_ms()?;
        let controller = QueueController::new(api.storage(), api.scheduler().clone());
        if let Some(name) = body.queue_name {
            queue = controller.rename(account_id, queue.id, &name, request_id, now)?;
        }
        if let Some(settings) = body.settings {
            let delivery_paused = settings.delivery_paused;
            let mut config = queue.config;
            if let Some(value) = settings.delivery_delay {
                config.delivery_delay_seconds = value;
            }
            if let Some(value) = settings.message_retention_period {
                config.retention_seconds = value;
            }
            if config != queue.config {
                queue = controller.update_config(
                    account_id,
                    queue.id,
                    queue.config_generation,
                    config,
                    request_id,
                    now,
                )?;
            }
            if let Some(paused) = delivery_paused {
                queue = api.set_delivery_paused(account_id, queue.id, paused, request_id, now)?;
            }
        }
        queue_response(&authority, api.storage(), queue)
    })
    .await;
    result_response(result, context)
}

async fn delete_queue(
    State(state): State<HttpState>,
    Path((account_public, queue_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if let Err(response) = bodyless(request, context).await {
        return response;
    }
    let (api, authority, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = api.clone();
    let authority = authority.clone();
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let queue = resolve_queue(&authority, api.storage(), account_id, &queue_public)?;
        QueueController::new(api.storage(), api.scheduler().clone()).delete(
            account_id,
            queue.id,
            queue.lifecycle_generation,
            true,
            request_id,
            now_ms()?,
        )?;
        Ok::<_, PlatformError>(DeleteResult { success: true })
    })
    .await;
    result_response(result, context)
}

pub(super) fn authority<'a>(
    state: &'a HttpState,
    public_account: &str,
) -> Result<
    (
        &'a std::sync::Arc<QueueApiState>,
        &'a super::accounts::AccountAuthority,
        AccountId,
    ),
    V4Error,
> {
    let api = state.queue_api().ok_or(V4Error::Unavailable)?;
    let authority = state.cloudflare_v4_account().ok_or(V4Error::Unavailable)?;
    let account = authority.resolve(public_account)?;
    Ok((api, authority, account))
}

pub(super) fn resolve_queue(
    authority: &super::accounts::AccountAuthority,
    storage: &open_compute_storage::PlatformStorage,
    account_id: AccountId,
    public_id: &str,
) -> Result<QueueRecord, PlatformError> {
    QueueRepository::new(storage.db())
        .list_account(account_id)?
        .into_iter()
        .find(|queue| authority.matches_public_queue_id(queue.id, public_id))
        .ok_or_else(not_found)
}

pub(super) fn context(
    request: &Request,
    permission: V4Permission,
) -> Result<V4RequestContext, Response> {
    let context = request_context(request)?;
    context
        .require(permission)
        .map_err(|error| error_response(error, context.request_id()))?;
    Ok(context)
}

pub(super) async fn json_body<T: DeserializeOwned>(
    request: Request,
    context: V4RequestContext,
) -> Result<T, Response> {
    validate_json_headers(request.headers())
        .map_err(|error| error_response(error, context.request_id()))?;
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| {
            error_response(
                V4Error::Official(V4OfficialError::RequestTooLarge),
                context.request_id(),
            )
        })?;
    if bytes.is_empty() {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))
}

pub(super) async fn bodyless(request: Request, context: V4RequestContext) -> Result<(), Response> {
    if request
        .headers()
        .get_all(header::CONTENT_TYPE)
        .iter()
        .count()
        != 0
    {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    let bytes = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, context.request_id()))?;
    if !bytes.is_empty() {
        return Err(error_response(
            V4Error::InvalidRequest,
            context.request_id(),
        ));
    }
    Ok(())
}

fn validate_json_headers(headers: &HeaderMap) -> Result<(), V4Error> {
    let values = headers
        .get_all(header::CONTENT_TYPE)
        .iter()
        .collect::<Vec<_>>();
    if values.len() != 1 {
        return Err(V4Error::InvalidRequest);
    }
    let value = values[0].to_str().map_err(|_| V4Error::InvalidRequest)?;
    let media = value.split(';').next().unwrap_or("").trim();
    if !media.eq_ignore_ascii_case("application/json")
        && !value.eq_ignore_ascii_case("text/plain;charset=UTF-8")
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(())
}

pub(super) fn platform_error(error: &PlatformError, context: V4RequestContext) -> Response {
    let mapped = match error.code() {
        ErrorCode::LimitInvalid => V4Error::Official(V4OfficialError::QueueSettingsInvalid),
        ErrorCode::QueueNotFound | ErrorCode::ResourceNotFound => V4Error::NotFound,
        ErrorCode::QueueNotEmpty
        | ErrorCode::QueueReferenced
        | ErrorCode::QueueConsumerConflict
        | ErrorCode::QueueConfigPending
        | ErrorCode::QueueConsumerProjectionPending => V4Error::Conflict,
        _ => V4Error::from(error),
    };
    error_response(mapped, context.request_id())
}

fn result_response<T: Serialize>(
    result: Result<Result<T, PlatformError>, tokio::task::JoinError>,
    context: V4RequestContext,
) -> Response {
    match result {
        Ok(Ok(value)) => success_response(context, value),
        Ok(Err(error)) => platform_error(&error, context),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

fn queue_response(
    authority: &super::accounts::AccountAuthority,
    storage: &open_compute_storage::PlatformStorage,
    queue: QueueRecord,
) -> Result<QueueResponse, PlatformError> {
    let consumers = QueueConsumerRepository::new(storage.db())
        .live_for_queue(queue.id)?
        .into_iter()
        .map(|record| consumers::consumer_response(authority, storage, &queue, record))
        .collect::<Result<Vec<_>, _>>()?;
    let producers = QueueRepository::new(storage.db())
        .active_producer_names(queue.account_id, queue.id)?
        .into_iter()
        .map(|script| ProducerResponse {
            kind: "worker",
            script,
        })
        .collect::<Vec<_>>();
    Ok(QueueResponse {
        consumers_total_count: consumers.len(),
        consumers,
        created_on: timestamp(queue.created_at_ms)?,
        modified_on: timestamp(queue.updated_at_ms)?,
        producers_total_count: producers.len(),
        producers,
        queue_id: authority.public_queue_id(queue.id),
        queue_name: queue.name,
        settings: QueueSettings {
            delivery_delay: queue.config.delivery_delay_seconds,
            delivery_paused: queue.delivery_paused,
            message_retention_period: queue.config.retention_seconds,
        },
    })
}

fn timestamp(value: i64) -> Result<String, PlatformError> {
    jiff::Timestamp::from_millisecond(value)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| internal())
}

fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::QueueNotFound, "Queue not found")
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "Queue operation failed")
}

#[derive(Serialize)]
struct QueueResponse {
    consumers: Vec<consumers::ConsumerResponse>,
    consumers_total_count: usize,
    created_on: String,
    modified_on: String,
    producers: Vec<ProducerResponse>,
    producers_total_count: usize,
    queue_id: String,
    queue_name: String,
    settings: QueueSettings,
}

#[derive(Serialize)]
struct ProducerResponse {
    #[serde(rename = "type")]
    kind: &'static str,
    script: String,
}

#[derive(Serialize)]
struct QueueSettings {
    delivery_delay: u32,
    delivery_paused: bool,
    message_retention_period: u32,
}

#[derive(Serialize)]
struct DeleteResult {
    success: bool,
}

#[derive(Clone, Debug)]
struct ListQuery {
    page: usize,
    per_page: usize,
    name: Option<String>,
}

impl ListQuery {
    fn parse(query: Option<&str>) -> Result<Self, V4Error> {
        let mut result = Self {
            page: 1,
            per_page: 20,
            name: None,
        };
        for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "page" => result.page = parse_page(&value, 10_000)?,
                "name" if result.name.is_none() && !value.is_empty() && value.len() <= 128 => {
                    result.name = Some(value.into_owned());
                }
                _ => return Err(V4Error::InvalidRequest),
            }
        }
        Ok(result)
    }
}

impl Default for QueueSettingsBody {
    fn default() -> Self {
        Self {
            delivery_delay: None,
            delivery_paused: None,
            message_retention_period: None,
        }
    }
}

fn parse_page(value: &str, max: usize) -> Result<usize, V4Error> {
    let value = value
        .parse::<usize>()
        .map_err(|_| V4Error::InvalidRequest)?;
    (value > 0 && value <= max)
        .then_some(value)
        .ok_or(V4Error::InvalidRequest)
}

#[cfg(test)]
#[path = "queues_tests.rs"]
mod tests;
