//! Authenticated operator controls for response-cache lifecycle and Images capacity.

use crate::http::{HttpState, authorize};
use crate::images_backend::ImageBindingService;
use crate::metrics::{CacheMetricOperation, MetricsRegistry};
use crate::run::gc_worker_artifacts;
use crate::snapshot_pins::SnapshotPins;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{AccountId, ErrorCode, PlatformError, WorkerId, WorkersConfig};
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
            "/v1/operator/accounts/{account_id}/workers/{worker_id}/cache",
            get(inspect_worker_cache),
        )
        .route(
            "/v1/operator/accounts/{account_id}/workers/{worker_id}/cache/purge",
            post(purge_worker_cache),
        )
        .route("/v1/operator/cache/gc", post(run_cache_gc))
        .route("/v1/operator/images/capacity", get(inspect_images))
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
        Err(error) => operator_error(&error),
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
        Err(error) => operator_error(&error),
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
        Err(error) => operator_error(&error),
    }
}

async fn inspect_images(State(state): State<HttpState>, request: Request) -> Response {
    let Some(api) = authorized_api(&state, &request) else {
        return unavailable(&state, &request);
    };
    match api.images.capacity() {
        Ok(capacity) => Json(capacity).into_response(),
        Err(error) => operator_error(&error),
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
    if authorize(state, request) {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn operator_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::AccountNotFound | ErrorCode::WorkerNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkerDeleted => StatusCode::GONE,
        ErrorCode::CacheCorrupt => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::CacheUnavailable | ErrorCode::ArtifactUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        Json(serde_json::json!({ "code": error.code().as_str() })),
    )
        .into_response()
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
