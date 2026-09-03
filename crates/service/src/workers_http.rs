//! Worker v4 management API state and public route ingress.

use crate::asset_backend::pin_response;
use crate::http::HttpState;
use crate::runtime_bridge::{DispatchTarget, WorkerdTransport};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{ErrorCode, PlatformError, RequestId, WorkerId};
use open_compute_storage::{PlatformStorage, WorkerRepository};
use open_compute_workers::{BundleLimits, ProductPromotionCoordinator, VersionPins};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) mod v4;

/// Shared Worker management and ingress authority.
#[derive(Clone)]
pub struct WorkerApiState {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    response_cache: Option<Arc<open_compute_storage::CacheManager>>,
    transport: WorkerdTransport,
    pins: VersionPins,
    bundle_limits: BundleLimits,
    delete_drain_timeout: Duration,
    max_queue_consumer_concurrency: u32,
    product_promoter: Option<Arc<dyn ProductPromotionCoordinator>>,
    traffic: Arc<WorkerTrafficRegistry>,
    upload_serial: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for WorkerApiState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerApiState")
            .field("artifacts", &self.artifacts)
            .field("pins", &self.pins)
            .finish_non_exhaustive()
    }
}

impl WorkerApiState {
    /// Bind HTTP handlers to typed storage, artifact, and runtime capabilities.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        transport: WorkerdTransport,
        pins: VersionPins,
        bundle_limits: BundleLimits,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            response_cache: None,
            transport,
            pins,
            bundle_limits,
            delete_drain_timeout,
            max_queue_consumer_concurrency: 32,
            product_promoter: None,
            traffic: Arc::new(WorkerTrafficRegistry::default()),
            upload_serial: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Attach the response-cache authority for Script deletion fencing and cleanup.
    #[must_use]
    pub fn with_response_cache(
        mut self,
        response_cache: Arc<open_compute_storage::CacheManager>,
    ) -> Self {
        self.response_cache = Some(response_cache);
        self
    }

    /// Apply the installation-local Queue consumer concurrency ceiling.
    #[must_use]
    pub fn with_queue_consumer_limit(mut self, maximum: u32) -> Self {
        self.max_queue_consumer_concurrency = maximum.max(1);
        self
    }

    /// Attach the Queue/Cron cross-database promotion owner.
    #[must_use]
    pub fn with_product_promoter(mut self, promoter: Arc<dyn ProductPromotionCoordinator>) -> Self {
        self.product_promoter = Some(promoter);
        self
    }

    /// Process-local dispatch/deletion pin registry.
    #[must_use]
    pub fn pins(&self) -> &VersionPins {
        &self.pins
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct WorkerTrafficAccumulator {
    requests: u64,
    errors: u64,
    total_latency_micros: u64,
    last_status: Option<u16>,
}

#[derive(Debug, Default)]
struct WorkerTrafficRegistry {
    entries: Mutex<HashMap<WorkerId, WorkerTrafficAccumulator>>,
}

impl WorkerTrafficRegistry {
    fn observe(&self, worker_id: WorkerId, status: u16, elapsed: Duration) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(worker_id).or_default();
        entry.requests = entry.requests.saturating_add(1);
        if status >= 500 {
            entry.errors = entry.errors.saturating_add(1);
        }
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        entry.total_latency_micros = entry.total_latency_micros.saturating_add(micros);
        entry.last_status = Some(status);
    }

    fn remove(&self, worker_id: WorkerId) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&worker_id);
    }
}

/// Resolve a persisted route, freeze its active Deployment, and dispatch through workerd.
pub async fn public_ingress(State(state): State<HttpState>, request: Request) -> Response {
    let request_id = request_id(&request);
    let Some(api) = state.worker_api() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let hostname = match request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| PlatformError::new(ErrorCode::RouteNotFound, "Host header is required"))
        .and_then(canonical_request_host)
    {
        Ok(hostname) => hostname,
        Err(error) => return crate::http::platform_error_response(&error, request_id),
    };
    let repo = WorkerRepository::new(api.storage.db());
    let snapshot = match repo.resolve_route(Some(&hostname), request.uri().path()) {
        Ok(snapshot) => snapshot,
        Err(error) if error.code() == ErrorCode::RouteNotFound => {
            match repo.resolve_route(None, request.uri().path()) {
                Ok(snapshot) => snapshot,
                Err(error) => return crate::http::platform_error_response(&error, request_id),
            }
        }
        Err(error) => return crate::http::platform_error_response(&error, request_id),
    };
    let pin = match api.pins.pin(snapshot.version.id) {
        Ok(pin) => pin,
        Err(error) => return crate::http::platform_error_response(&error, request_id),
    };
    let Ok(route_generation) = i64::try_from(snapshot.worker.route_generation) else {
        return crate::http::platform_error_response(
            &PlatformError::new(
                ErrorCode::VersionInvariantViolation,
                "route generation exceeds the runtime protocol",
            ),
            request_id,
        );
    };
    let target = DispatchTarget {
        account_id: snapshot.route.account_id,
        worker_id: snapshot.route.worker_id,
        version_id: snapshot.version.id,
        worker_code_sha256: hex::encode(snapshot.version.worker_code_sha256),
        entrypoint: snapshot.route.entrypoint,
        route_generation,
        request_id,
    };
    let worker_id = snapshot.route.worker_id;
    let started = std::time::Instant::now();
    let response = match api.transport.dispatch(target, request).await {
        Ok(response) => pin_response(response, pin),
        Err(error) => crate::http::platform_error_response(&error, request_id),
    };
    api.traffic
        .observe(worker_id, response.status().as_u16(), started.elapsed());
    response
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

fn canonical_request_host(value: &str) -> Result<String, PlatformError> {
    let authority = value.parse::<axum::http::uri::Authority>().map_err(|_| {
        PlatformError::new(ErrorCode::RouteNotFound, "public request Host is invalid")
    })?;
    canonical_hostname(authority.host())
}

fn canonical_hostname(value: &str) -> Result<String, PlatformError> {
    if value.is_empty() || value.len() > 253 || value.contains(['/', '@', '#', '?']) {
        return Err(PlatformError::new(
            ErrorCode::RouteNotFound,
            "public request Host is invalid",
        ));
    }
    url::Host::parse(value)
        .map(|host| host.to_string().trim_end_matches('.').to_ascii_lowercase())
        .map_err(|_| PlatformError::new(ErrorCode::RouteNotFound, "public request Host is invalid"))
}
