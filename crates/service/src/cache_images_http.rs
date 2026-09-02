//! Authenticated operator controls for response-cache lifecycle and Images capacity.

use crate::http::{HttpState, authorize, operator_error_response};
use crate::images_backend::ImageBindingService;
use crate::metrics::{CacheMetricOperation, MetricsRegistry};
use crate::run::gc_worker_artifacts;
use crate::snapshot_pins::SnapshotPins;
use axum::extract::{Path, Request, State};
#[cfg(test)]
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{AccountId, ErrorCode, PlatformError, RequestId, WorkerId, WorkersConfig};
use open_compute_storage::{CacheManager, PlatformStorage, WorkerRepository};
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Composed operator-only P3.3 authority.
#[derive(Clone)]
pub(crate) struct CacheImagesApiState {
    storage: Arc<PlatformStorage>,
    cache: Arc<CacheManager>,
    images: Arc<ImageBindingService>,
    artifacts: ArtifactStore,
    workers: WorkersConfig,
    snapshot_pins: Arc<SnapshotPins>,
    metrics: Arc<MetricsRegistry>,
}

impl std::fmt::Debug for CacheImagesApiState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheImagesApiState")
            .finish_non_exhaustive()
    }
}

impl CacheImagesApiState {
    /// Compose already-verified local authorities behind the existing admin capability.
    #[must_use]
    pub(crate) fn new(
        storage: Arc<PlatformStorage>,
        cache: Arc<CacheManager>,
        images: Arc<ImageBindingService>,
        artifacts: ArtifactStore,
        workers: WorkersConfig,
        snapshot_pins: Arc<SnapshotPins>,
        metrics: Arc<MetricsRegistry>,
    ) -> Self {
        Self {
            storage,
            cache,
            images,
            artifacts,
            workers,
            snapshot_pins,
            metrics,
        }
    }
}

/// Operator routes; every handler independently enforces configured admin auth.
pub(crate) fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/cache",
            get(inspect_worker_cache),
        )
        .route(
            "/v1/accounts/{account_id}/workers/{worker_id}/cache/purge",
            post(purge_worker_cache),
        )
        .route("/v1/cache", get(inspect_platform_cache))
        .route("/v1/cache/gc", post(run_cache_gc))
        .route("/v1/images/capacity", get(inspect_images))
}

async fn inspect_platform_cache(State(state): State<HttpState>, request: Request) -> Response {
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    match api.cache.stats(now_ms()) {
        Ok(stats) => {
            api.metrics.set_response_cache_stats(stats);
            Json(serde_json::json!({
                "entries": stats.entries,
                "bodyBytes": stats.body_bytes,
                "metadataBytes": stats.metadata_bytes,
                "activeRefreshes": stats.active_refreshes,
                "openDatabases": stats.open_databases,
            }))
            .into_response()
        }
        Err(error) => operator_error_response(&error, request_id(&request)),
    }
}

async fn inspect_worker_cache(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let result = parse_worker(api, &account, &worker)
        .and_then(|(account, worker)| api.cache.worker_stats(account, worker, now_ms()));
    match result {
        Ok(stats) => {
            if let Ok(process) = api.cache.stats(now_ms()) {
                api.metrics.set_response_cache_stats(process);
            }
            Json(serde_json::json!({
                "entries": stats.entries,
                "bodyBytes": stats.body_bytes,
                "metadataBytes": stats.metadata_bytes,
                "activeRefreshes": stats.active_refreshes,
                "openDatabases": stats.open_databases,
            }))
            .into_response()
        }
        Err(error) => operator_error_response(&error, request_id(&request)),
    }
}

async fn purge_worker_cache(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    let result = parse_worker(api, &account, &worker)
        .and_then(|(account, worker)| api.cache.purge_worker(account, worker, now_ms()));
    api.metrics
        .observe_response_cache(CacheMetricOperation::Purge, result.is_ok());
    match result {
        Ok(deleted) => {
            Json(serde_json::json!({ "success": true, "deleted": deleted })).into_response()
        }
        Err(error) => operator_error_response(&error, request_id(&request)),
    }
}

async fn run_cache_gc(State(state): State<HttpState>, request: Request) -> Response {
    let Some(api) = authorized_api(&state, &request).cloned() else {
        return unavailable(&state, &request);
    };
    match gc_worker_artifacts(
        &api.storage,
        &api.artifacts,
        &api.workers,
        &api.snapshot_pins,
        Some(api.cache.clone()),
    )
    .await
    {
        Ok(deleted) => Json(serde_json::json!({ "deleted": deleted })).into_response(),
        Err(error) => operator_error_response(&error, request_id(&request)),
    }
}

async fn inspect_images(State(state): State<HttpState>, request: Request) -> Response {
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    match api.images.capacity() {
        Ok(capacity) => Json(capacity).into_response(),
        Err(error) => operator_error_response(&error, request_id(&request)),
    }
}

fn parse_worker(
    api: &CacheImagesApiState,
    account: &str,
    worker: &str,
) -> Result<(AccountId, WorkerId), PlatformError> {
    let account = AccountId::from_str(account).map_err(|_| invalid())?;
    let worker = WorkerId::from_str(worker).map_err(|_| invalid())?;
    WorkerRepository::new(api.storage.db()).get_worker(account, worker)?;
    Ok((account, worker))
}

fn authorized_api<'a>(
    state: &'a HttpState,
    request: &Request,
) -> Option<&'a Arc<CacheImagesApiState>> {
    authorize(state, request)
        .then(|| state.cache_images_api())
        .flatten()
}

fn unavailable(state: &HttpState, request: &Request) -> Response {
    let error = if authorize(state, request) {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "cache and Images operator authority is unavailable",
        )
    } else {
        PlatformError::new(
            ErrorCode::AdminAuthRequired,
            "admin authentication is required",
        )
    };
    operator_error_response(&error, request_id(request))
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheProtocolError,
        "cache operator request is invalid",
    )
}

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "cache_images_http_tests.rs"]
mod tests;
