//! Authenticated bounded operator surface for the P0.8 scheduler.

use crate::http::{HttpState, authorize};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

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
        Ok(repaired) => axum::Json(serde_json::json!({ "repaired": repaired })).into_response(),
        Err(error) => scheduler_error(&error),
    }
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

fn scheduler_error(error: &open_compute_core::PlatformError) -> Response {
    let status = if error.code() == open_compute_core::ErrorCode::SchedulerKindNotEnabled {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        axum::Json(serde_json::json!({ "code": error.code().as_str() })),
    )
        .into_response()
}

fn requested_kind(
    request: &Request,
) -> Result<Option<open_compute_core::SchedulerKind>, open_compute_core::PlatformError> {
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

fn kind_not_enabled() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::SchedulerKindNotEnabled,
        "scheduler workload kind is not enabled in this release",
    )
}
