//! Cloudflare worker Queue consumer adapter.

use super::{
    authority, bodyless, context, json_body, platform_error, resolve_queue, success_response,
};
use crate::cloudflare_v4::wire::V4OfficialError;
use crate::cloudflare_v4::{V4Error, V4Permission, error_response};
use crate::http::HttpState;
use crate::queue_api::now_ms;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::response::Response;
use axum::routing::get;
use open_compute_core::{AccountId, ErrorCode, PlatformError, QueueId, WorkerId};
use open_compute_storage::{
    PlatformStorage, QueueConsumerConfig, QueueConsumerRecord, QueueConsumerRepository,
    QueueRecord, WorkerRepository,
};
use serde::{Deserialize, Serialize};

pub(super) fn router() -> Router<HttpState> {
    Router::new()
        .route(
            "/accounts/{account_id}/queues/{queue_id}/consumers",
            get(list_consumers).post(create_consumer),
        )
        .route(
            "/accounts/{account_id}/queues/{queue_id}/consumers/{consumer_id}",
            get(get_consumer)
                .put(update_consumer)
                .delete(delete_consumer),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsumerBody {
    #[serde(rename = "type")]
    pub(super) kind: ConsumerKind,
    pub(super) script_name: String,
    dead_letter_queue: Option<String>,
    settings: Option<ConsumerSettingsBody>,
    #[serde(default)]
    environment_name: Option<String>,
}

#[derive(Deserialize)]
pub(super) enum ConsumerKind {
    #[serde(rename = "worker")]
    Worker,
    #[serde(rename = "http_pull")]
    HttpPull,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConsumerSettingsBody {
    pub(super) batch_size: Option<u32>,
    pub(super) max_concurrency: Option<u32>,
    pub(super) max_retries: Option<u32>,
    pub(super) max_wait_time_ms: Option<u32>,
    pub(super) retry_delay: Option<u32>,
}

async fn list_consumers(
    State(state): State<HttpState>,
    Path((account_public, queue_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if let Err(response) = bodyless(request, context).await {
        return response.into_response();
    }
    let (api, account, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let storage = api.storage().clone();
    let account = account.clone();
    let result = tokio::task::spawn_blocking(move || {
        let queue = resolve_queue(&account, &storage, account_id, &queue_public)?;
        QueueConsumerRepository::new(storage.db())
            .live_for_queue(queue.id)?
            .into_iter()
            .map(|record| consumer_response(&account, &storage, &queue, &record))
            .collect::<Result<Vec<_>, _>>()
    })
    .await;
    respond(result, context)
}

async fn create_consumer(
    State(state): State<HttpState>,
    Path((account_public, queue_public)): Path<(String, String)>,
    request: Request,
) -> Response {
    mutate_consumer(state, account_public, queue_public, None, request).await
}

async fn update_consumer(
    State(state): State<HttpState>,
    Path((account_public, queue_public, consumer_public)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    mutate_consumer(
        state,
        account_public,
        queue_public,
        Some(consumer_public),
        request,
    )
    .await
}

async fn mutate_consumer(
    state: HttpState,
    account_public: String,
    queue_public: String,
    expected_consumer: Option<String>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let body = match json_body::<ConsumerBody>(request, context).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if matches!(body.kind, ConsumerKind::HttpPull) {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    if body
        .environment_name
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        return error_response(V4Error::Unsupported, context.request_id());
    }
    let (api, account, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = api.clone();
    let account = account.clone();
    let result = tokio::task::spawn_blocking(move || {
        let queue = resolve_queue(&account, api.storage(), account_id, &queue_public)?;
        if let Some(public) = expected_consumer {
            resolve_consumer(&account, api.storage(), account_id, queue.id, &public)?;
        } else if QueueConsumerRepository::new(api.storage().db())
            .live_for_queue(queue.id)?
            .is_some()
        {
            return Err(PlatformError::new(
                ErrorCode::QueueConsumerConflict,
                "Queue already has a consumer",
            ));
        }
        let worker = resolve_worker(api.storage(), account_id, &body.script_name)?;
        let dead_letter_queue = body
            .dead_letter_queue
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(|name| resolve_queue_name(api.storage(), account_id, name))
            .transpose()?
            .map(|queue| queue.id);
        let config = settings(body.settings, api.max_consumer_concurrency())?;
        let record = api.upsert_consumer(
            account_id,
            queue.id,
            worker,
            config,
            dead_letter_queue,
            now_ms()?,
        )?;
        consumer_response(&account, api.storage(), &queue, &record)
    })
    .await;
    respond_consumer(result, context)
}

async fn get_consumer(
    State(state): State<HttpState>,
    Path((account_public, queue_public, consumer_public)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if let Err(response) = bodyless(request, context).await {
        return response.into_response();
    }
    let (api, account, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let storage = api.storage().clone();
    let account = account.clone();
    let result = tokio::task::spawn_blocking(move || {
        let queue = resolve_queue(&account, &storage, account_id, &queue_public)?;
        let record = resolve_consumer(&account, &storage, account_id, queue.id, &consumer_public)?;
        consumer_response(&account, &storage, &queue, &record)
    })
    .await;
    respond_consumer(result, context)
}

async fn delete_consumer(
    State(state): State<HttpState>,
    Path((account_public, queue_public, consumer_public)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let context = match context(&request, V4Permission::ProductWrite) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if request.uri().query().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    if let Err(response) = bodyless(request, context).await {
        return response.into_response();
    }
    let (api, account, account_id) = match authority(&state, &account_public) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let api = api.clone();
    let account = account.clone();
    let request_id = context.request_id();
    let result = tokio::task::spawn_blocking(move || {
        let queue = resolve_queue(&account, api.storage(), account_id, &queue_public)?;
        let record = resolve_consumer(
            &account,
            api.storage(),
            account_id,
            queue.id,
            &consumer_public,
        )?;
        api.delete_consumer(account_id, record.id, request_id, now_ms()?)?;
        Ok::<_, PlatformError>(DeleteResponse { success: true })
    })
    .await;
    respond(result, context)
}

pub(super) fn settings(
    body: Option<ConsumerSettingsBody>,
    local_max: u32,
) -> Result<QueueConsumerConfig, PlatformError> {
    let body = body.unwrap_or_default();
    if body
        .max_wait_time_ms
        .is_some_and(|value| value % 1_000 != 0)
    {
        return Err(PlatformError::new(
            ErrorCode::LimitInvalid,
            "Queue consumer wait time must use whole seconds",
        ));
    }
    QueueConsumerConfig {
        max_batch_size: body.batch_size.unwrap_or(10),
        max_batch_timeout_seconds: body.max_wait_time_ms.unwrap_or(5_000) / 1_000,
        max_retries: body.max_retries.unwrap_or(3),
        retry_delay_seconds: body.retry_delay.unwrap_or(0),
        max_concurrency: body.max_concurrency.unwrap_or(local_max),
    }
    .validate(local_max)
}

fn resolve_worker(
    storage: &PlatformStorage,
    account_id: AccountId,
    name: &str,
) -> Result<WorkerId, PlatformError> {
    WorkerRepository::new(storage.db())
        .list_workers(account_id)?
        .into_iter()
        .find(|worker| worker.name == name)
        .map(|worker| worker.id)
        .ok_or_else(|| PlatformError::new(ErrorCode::WorkerNotFound, "Worker not found"))
}

fn resolve_queue_name(
    storage: &PlatformStorage,
    account_id: AccountId,
    name: &str,
) -> Result<QueueRecord, PlatformError> {
    open_compute_storage::QueueRepository::new(storage.db())
        .list_account(account_id)?
        .into_iter()
        .find(|queue| queue.name == name)
        .ok_or_else(|| PlatformError::new(ErrorCode::QueueNotFound, "Queue not found"))
}

fn resolve_consumer(
    authority: &crate::cloudflare_v4::accounts::AccountAuthority,
    storage: &PlatformStorage,
    account_id: AccountId,
    queue_id: QueueId,
    public: &str,
) -> Result<QueueConsumerRecord, PlatformError> {
    QueueConsumerRepository::new(storage.db())
        .live_for_queue(queue_id)?
        .filter(|record| {
            record.account_id == account_id
                && authority.matches_public_queue_consumer_id(record.id, public)
        })
        .ok_or_else(|| PlatformError::new(ErrorCode::ResourceNotFound, "consumer not found"))
}

pub(super) fn consumer_response(
    authority: &crate::cloudflare_v4::accounts::AccountAuthority,
    storage: &PlatformStorage,
    queue: &QueueRecord,
    record: &QueueConsumerRecord,
) -> Result<ConsumerResponse, PlatformError> {
    let declaration =
        QueueConsumerRepository::new(storage.db()).declaration(record.declaration_id)?;
    let worker =
        WorkerRepository::new(storage.db()).get_worker(record.account_id, record.worker_id)?;
    let dead_letter_queue = declaration
        .dlq_queue_id
        .map(|id| {
            open_compute_storage::QueueRepository::new(storage.db()).get(record.account_id, id)
        })
        .transpose()?
        .map_or_else(String::new, |queue| queue.name);
    Ok(ConsumerResponse {
        consumer_id: authority.public_queue_consumer_id(record.id),
        created_on: timestamp(record.created_at_ms)?,
        dead_letter_queue,
        queue_name: queue.name.clone(),
        script: worker.name.clone(),
        script_name: worker.name,
        settings: ConsumerSettings {
            batch_size: declaration.config.max_batch_size,
            max_concurrency: declaration.config.max_concurrency,
            max_retries: declaration.config.max_retries,
            max_wait_time_ms: declaration
                .config
                .max_batch_timeout_seconds
                .saturating_mul(1_000),
            retry_delay: declaration.config.retry_delay_seconds,
        },
        kind: "worker",
    })
}

fn respond<T: Serialize>(
    result: Result<Result<T, PlatformError>, tokio::task::JoinError>,
    context: crate::cloudflare_v4::V4RequestContext,
) -> Response {
    match result {
        Ok(Ok(value)) => success_response(context, value),
        Ok(Err(error)) => platform_error(&error, context),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

fn respond_consumer(
    result: Result<Result<ConsumerResponse, PlatformError>, tokio::task::JoinError>,
    context: crate::cloudflare_v4::V4RequestContext,
) -> Response {
    match result {
        Ok(Ok(value)) => success_response(context, value),
        Ok(Err(error)) if error.code() == ErrorCode::LimitInvalid => error_response(
            V4Error::Official(V4OfficialError::QueueConsumerSettingsInvalid),
            context.request_id(),
        ),
        Ok(Err(error)) => platform_error(&error, context),
        Err(_) => error_response(V4Error::Internal, context.request_id()),
    }
}

fn timestamp(value: i64) -> Result<String, PlatformError> {
    jiff::Timestamp::from_millisecond(value)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| PlatformError::new(ErrorCode::Internal, "Queue consumer timestamp invalid"))
}

#[derive(Serialize)]
pub(super) struct ConsumerResponse {
    consumer_id: String,
    created_on: String,
    dead_letter_queue: String,
    queue_name: String,
    /// Wrangler 4.127.1 reads `script` while cloudflare 7.1.0 reads `script_name`.
    script: String,
    script_name: String,
    settings: ConsumerSettings,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct ConsumerSettings {
    batch_size: u32,
    max_concurrency: u32,
    max_retries: u32,
    max_wait_time_ms: u32,
    retry_delay: u32,
}

#[derive(Serialize)]
struct DeleteResponse {
    success: bool,
}

#[cfg(test)]
#[path = "consumers_tests.rs"]
mod tests;
