//! Authenticated bounded operator surface for the P0.8 scheduler.

use crate::http::{HttpState, authorize};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_core::{ErrorCode, PlatformError, QueueConsumerId, RequestId};
use serde::Deserialize;
use std::str::FromStr as _;

const MAX_OPERATOR_BODY: usize = 1024;

/// Scheduler operator routes; all handlers enforce the configured admin capability.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route("/v1/scheduler", get(inspect))
        .route("/v1/scheduler/pause", post(pause))
        .route("/v1/scheduler/resume", post(resume))
        .route("/v1/scheduler/repair", post(repair))
        .route("/v1/operator/scheduler", get(inspect))
        .route("/v1/operator/scheduler/pause", post(pause))
        .route("/v1/operator/scheduler/resume", post(resume))
        .route("/v1/operator/scheduler/repair", post(repair))
        .route("/v1/operator/queue-consumers", get(inspect))
        .route(
            "/v1/operator/queue-consumers/{consumer_id}/pause",
            post(pause_queue_consumer),
        )
        .route(
            "/v1/operator/queue-consumers/{consumer_id}/resume",
            post(resume_queue_consumer),
        )
        .route("/v1/operator/cron-activations", get(inspect))
}

async fn inspect(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    match scheduler.inspect() {
        Ok(summary) => axum::Json(summary).into_response(),
        Err(error) => scheduler_error(&error),
    }
}

async fn pause(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    match requested_kind(&request) {
        Ok(Some(kind)) => match scheduler.pause_kind(kind) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => scheduler_error(&error),
        },
        Ok(None) => {
            scheduler.pause();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => scheduler_error(&error),
    }
}

async fn resume(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    match requested_kind(&request) {
        Ok(Some(kind)) => match scheduler.resume_kind(kind) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => scheduler_error(&error),
        },
        Ok(None) => {
            scheduler.resume();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => scheduler_error(&error),
    }
}

async fn repair(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    match scheduler.repair_and_probe().await {
        Ok(alarm_repaired) => match scheduler.repair_products(1_000) {
            Ok(product_repaired) => axum::Json(serde_json::json!({
                "repaired": u64::from(alarm_repaired).saturating_add(product_repaired),
                "alarmRepaired": alarm_repaired,
                "productRepaired": product_repaired,
            }))
            .into_response(),
            Err(error) => scheduler_error(&error),
        },
        Err(error) => scheduler_error(&error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueueConsumerGenerationBody {
    consumer_generation: u64,
}

async fn pause_queue_consumer(
    State(state): State<HttpState>,
    Path(consumer_id): Path<String>,
    request: Request,
) -> Response {
    mutate_queue_consumer(state, consumer_id, request, true).await
}

async fn resume_queue_consumer(
    State(state): State<HttpState>,
    Path(consumer_id): Path<String>,
    request: Request,
) -> Response {
    mutate_queue_consumer(state, consumer_id, request, false).await
}

async fn mutate_queue_consumer(
    state: HttpState,
    consumer_id: String,
    request: Request,
    pause: bool,
) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request).cloned() else {
        return unavailable(&state, &request);
    };
    let Ok(consumer_id) = QueueConsumerId::from_str(&consumer_id) else {
        return scheduler_error(&invalid_operator_request());
    };
    let request_id = request_id(&request);
    let body = match read_generation(request).await {
        Ok(body) if body.consumer_generation > 0 => body,
        Ok(_) | Err(_) => return scheduler_error(&invalid_operator_request()),
    };
    let result = if pause {
        scheduler.pause_queue_consumer_operator(consumer_id, body.consumer_generation, request_id)
    } else {
        scheduler.resume_queue_consumer_operator(consumer_id, body.consumer_generation, request_id)
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => scheduler_error(&error),
    }
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

async fn read_generation(request: Request) -> Result<QueueConsumerGenerationBody, PlatformError> {
    let bytes = to_bytes(request.into_body(), MAX_OPERATOR_BODY)
        .await
        .map_err(|_| invalid_operator_request())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_operator_request())
}

fn authorized_scheduler<'a>(
    state: &'a HttpState,
    request: &Request,
) -> Option<&'a std::sync::Arc<crate::scheduler::SchedulerService>> {
    authorize(state, request)
        .then(|| state.scheduler())
        .flatten()
}

fn unavailable(state: &HttpState, request: &Request) -> Response {
    if authorize(state, request) {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn scheduler_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::SchedulerKindNotEnabled | ErrorCode::ConfigInvalid => StatusCode::BAD_REQUEST,
        ErrorCode::QueueConsumerGenerationStale => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        axum::Json(serde_json::json!({ "code": error.code().as_str() })),
    )
        .into_response()
}

fn requested_kind(
    request: &Request,
) -> Result<Option<open_compute_core::SchedulerKind>, PlatformError> {
    let Some(query) = request.uri().query() else {
        return Ok(None);
    };
    let mut values = query.split('&');
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(kind_not_enabled());
    }
    let Some(kind) = value.strip_prefix("kind=") else {
        return Err(kind_not_enabled());
    };
    open_compute_core::SchedulerKind::parse(kind)
        .map(Some)
        .ok_or_else(kind_not_enabled)
}

fn kind_not_enabled() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerKindNotEnabled,
        "scheduler workload kind is not enabled in this release",
    )
}

fn invalid_operator_request() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigInvalid,
        "scheduler operator request is invalid",
    )
}

#[cfg(test)]
#[path = "scheduler_http_tests.rs"]
mod tests;
