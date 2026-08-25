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
}

async fn inspect(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    match scheduler.summary() {
        Ok(summary) => axum::Json(serde_json::json!({
            "paused": scheduler.is_paused(),
            "summary": summary,
        }))
        .into_response(),
        Err(error) => scheduler_error(&error),
    }
}

async fn pause(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    scheduler.pause();
    StatusCode::NO_CONTENT.into_response()
}

async fn resume(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    scheduler.resume();
    StatusCode::NO_CONTENT.into_response()
}

async fn repair(State(state): State<HttpState>, request: Request) -> Response {
    let Some(scheduler) = authorized_scheduler(&state, &request) else {
        return unavailable(&state, &request);
    };
    match scheduler.repair_once().await {
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
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({ "code": error.code().as_str() })),
    )
        .into_response()
}
