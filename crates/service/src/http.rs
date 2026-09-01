//! Fixed health/metrics HTTP surface.

use crate::auth::{bearer_matches, resolve_admin_auth};
use crate::cache_images_http::{self, CacheImagesApiState};
use crate::d1_http::{self, D1ApiState};
use crate::do_http::{self, DoApiState};
use crate::health::HealthCoordinator;
use crate::kv_http::{self, KvApiState};
use crate::metrics::{CONTENT_TYPE, MetricsRegistry};
use crate::queue_http::{self, QueueApiState};
use crate::r2_http::{self, R2ApiState};
use crate::scheduler::SchedulerService;
use crate::scheduler_http;
use crate::workers_http::{self, WorkerApiState};
use crate::workflow_http::{self, WorkflowApiState};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use open_compute_core::config::ServerConfig;
use open_compute_core::{ErrorCode, OperationClass, ReadinessReason, RequestId, SecretString};
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use serde::Serialize;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;

/// Response header carrying the generated request ID.
pub const REQUEST_ID_HEADER: &str = "x-open-compute-request-id";
const MAX_BODY: usize = 4096;
const MAX_HEADER_BYTES: usize = 8192;
const MAX_HEADER_TOTAL: usize = 16_384;
const MAX_DEPLOYMENT_HEADER_TOTAL: usize =
    workers_http::MAX_DEPLOYMENT_METADATA_HEADER_BYTES + MAX_HEADER_TOTAL;

/// Stable error metadata attached internally for low-cardinality product metrics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProductErrorCode(pub ErrorCode);

/// Shared HTTP state.
#[derive(Clone)]
pub struct HttpState {
    health: HealthCoordinator,
    metrics: Arc<MetricsRegistry>,
    metrics_enabled: bool,
    admin_secret: Option<Arc<SecretString>>,
    supervisor: Arc<dyn Fn() -> Option<SanitizedSupervisor> + Send + Sync>,
    #[cfg(any(test, feature = "test-support"))]
    test_runtime_restart: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    worker_api: Option<Arc<WorkerApiState>>,
    kv_api: Option<Arc<KvApiState>>,
    r2_api: Option<Arc<R2ApiState>>,
    d1_api: Option<Arc<D1ApiState>>,
    do_api: Option<Arc<DoApiState>>,
    queue_api: Option<Arc<QueueApiState>>,
    workflow_api: Option<Arc<WorkflowApiState>>,
    scheduler: Option<Arc<SchedulerService>>,
    cache_images_api: Option<Arc<CacheImagesApiState>>,
}

impl std::fmt::Debug for HttpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpState")
            .field("metrics_enabled", &self.metrics_enabled)
            .field("admin_auth", &self.admin_secret.is_some())
            .field(
                "test_runtime_restart",
                &cfg!(any(test, feature = "test-support")),
            )
            .field("worker_api", &self.worker_api.is_some())
            .field("kv_api", &self.kv_api.is_some())
            .field("r2_api", &self.r2_api.is_some())
            .field("d1_api", &self.d1_api.is_some())
            .field("do_api", &self.do_api.is_some())
            .field("queue_api", &self.queue_api.is_some())
            .field("workflow_api", &self.workflow_api.is_some())
            .field("scheduler", &self.scheduler.is_some())
            .field("cache_images_api", &self.cache_images_api.is_some())
            .finish_non_exhaustive()
    }
}

/// Redacted supervisor fields allowed on `/health/status`.
#[derive(Clone, Debug, Serialize)]
pub struct SanitizedSupervisor {
    /// Supervisor state token.
    pub state: SupervisorState,
    /// Stable readiness reason.
    pub reason: ReadinessReason,
    /// Attempt counter.
    pub attempt: u32,
}

impl From<&SupervisorSnapshot> for SanitizedSupervisor {
    fn from(snap: &SupervisorSnapshot) -> Self {
        Self {
            state: snap.state,
            reason: snap.reason,
            attempt: snap.attempt,
        }
    }
}

impl HttpState {
    /// Build state. Resolves admin auth when configured.
    pub fn new(
        health: HealthCoordinator,
        metrics: Arc<MetricsRegistry>,
        metrics_enabled: bool,
        server: &ServerConfig,
        supervisor: Arc<dyn Fn() -> Option<SanitizedSupervisor> + Send + Sync>,
    ) -> Result<Self, open_compute_core::PlatformError> {
        let admin_secret = match &server.admin_auth {
            Some(reference) => Some(Arc::new(resolve_admin_auth(reference)?)),
            None => None,
        };
        Ok(Self {
            health,
            metrics,
            metrics_enabled,
            admin_secret,
            supervisor,
            #[cfg(any(test, feature = "test-support"))]
            test_runtime_restart: None,
            worker_api: None,
            kv_api: None,
            r2_api: None,
            d1_api: None,
            do_api: None,
            queue_api: None,
            workflow_api: None,
            scheduler: None,
            cache_images_api: None,
        })
    }

    /// Test helper with no admin auth.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(
        health: HealthCoordinator,
        metrics: Arc<MetricsRegistry>,
        metrics_enabled: bool,
        admin_secret: Option<SecretString>,
    ) -> Self {
        Self {
            health,
            metrics,
            metrics_enabled,
            admin_secret: admin_secret.map(Arc::new),
            supervisor: Arc::new(|| None),
            test_runtime_restart: None,
            worker_api: None,
            kv_api: None,
            r2_api: None,
            d1_api: None,
            do_api: None,
            queue_api: None,
            workflow_api: None,
            scheduler: None,
            cache_images_api: None,
        }
    }

    /// Attach the P0.2 control/data plane to this listener state.
    #[must_use]
    pub fn with_worker_api(mut self, worker_api: WorkerApiState) -> Self {
        self.worker_api = Some(Arc::new(worker_api));
        self
    }

    /// Attach a generic supervised-runtime restart hook to test-support builds.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub(crate) fn with_test_runtime_restart(
        mut self,
        restart: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        self.test_runtime_restart = Some(restart);
        self
    }

    /// Borrow the optional P0.2 API state.
    #[must_use]
    pub(crate) fn worker_api(&self) -> Option<&Arc<WorkerApiState>> {
        self.worker_api.as_ref()
    }

    /// Attach the P0.4 KV control plane to this listener state.
    #[must_use]
    pub fn with_kv_api(mut self, kv_api: KvApiState) -> Self {
        self.kv_api = Some(Arc::new(kv_api));
        self
    }

    /// Borrow the optional P0.4 API state.
    #[must_use]
    pub(crate) fn kv_api(&self) -> Option<&Arc<KvApiState>> {
        self.kv_api.as_ref()
    }

    /// Attach the P0.5 R2 logical-bucket control plane.
    #[must_use]
    pub fn with_r2_api(mut self, r2_api: R2ApiState) -> Self {
        self.r2_api = Some(Arc::new(r2_api));
        self
    }

    /// Borrow the optional P0.5 R2 control-plane state.
    #[must_use]
    pub(crate) fn r2_api(&self) -> Option<&Arc<R2ApiState>> {
        self.r2_api.as_ref()
    }

    /// Attach the P0.6 D1 control plane.
    #[must_use]
    pub fn with_d1_api(mut self, d1_api: D1ApiState) -> Self {
        self.d1_api = Some(Arc::new(d1_api));
        self
    }

    /// Borrow the optional P0.6 D1 control-plane state.
    #[must_use]
    pub(crate) fn d1_api(&self) -> Option<&Arc<D1ApiState>> {
        self.d1_api.as_ref()
    }

    /// Attach the P0.7 Durable Object control plane.
    #[must_use]
    pub fn with_do_api(mut self, do_api: DoApiState) -> Self {
        self.do_api = Some(Arc::new(do_api));
        self
    }

    /// Borrow the optional P0.7 Durable Object control-plane state.
    #[must_use]
    pub(crate) fn do_api(&self) -> Option<&Arc<DoApiState>> {
        self.do_api.as_ref()
    }

    /// Attach the P2.2 Queue catalog control plane.
    #[must_use]
    pub fn with_queue_api(mut self, queue_api: Option<QueueApiState>) -> Self {
        self.queue_api = queue_api.map(Arc::new);
        self
    }

    /// Borrow the optional P2.2 Queue control-plane state.
    #[must_use]
    pub(crate) fn queue_api(&self) -> Option<&Arc<QueueApiState>> {
        self.queue_api.as_ref()
    }

    /// Attach the Workflow catalog and bounded operator history.
    #[must_use]
    pub fn with_workflow_api(mut self, workflow_api: Option<WorkflowApiState>) -> Self {
        self.workflow_api = workflow_api.map(Arc::new);
        self
    }

    pub(crate) fn workflow_api(&self) -> Option<&Arc<WorkflowApiState>> {
        self.workflow_api.as_ref()
    }

    /// Attach the P0.8 scheduler operator surface.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: Option<Arc<SchedulerService>>) -> Self {
        self.scheduler = scheduler;
        self
    }

    /// Borrow the optional P0.8 scheduler service.
    #[must_use]
    pub(crate) fn scheduler(&self) -> Option<&Arc<SchedulerService>> {
        self.scheduler.as_ref()
    }

    /// Attach the P3.3 cache and Images operator authority.
    #[must_use]
    pub(crate) fn with_cache_images_api(mut self, api: CacheImagesApiState) -> Self {
        self.cache_images_api = Some(Arc::new(api));
        self
    }

    /// Borrow the optional P3.3 operator authority.
    #[must_use]
    pub(crate) fn cache_images_api(&self) -> Option<&Arc<CacheImagesApiState>> {
        self.cache_images_api.as_ref()
    }

    /// Borrow the fixed-series metrics registry from product control handlers.
    #[must_use]
    pub(crate) const fn metrics(&self) -> &Arc<MetricsRegistry> {
        &self.metrics
    }
}

/// Public routes only.
pub fn public_router(state: HttpState) -> Router {
    let middleware_state = state.clone();
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .fallback(workers_http::public_ingress)
        .layer(axum::middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state)
}

/// Admin routes, including public health plus status/metrics.
pub fn admin_router(state: HttpState) -> Router {
    let metrics_enabled = state.metrics_enabled;
    let middleware_state = state.clone();
    let mut router = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/health/status", get(status));
    if metrics_enabled {
        router = router.route("/metrics", get(metrics_handler));
    }
    router = router
        .merge(workers_http::control_router())
        .merge(kv_http::control_router())
        .merge(d1_http::control_router());
    router = router.merge(do_http::control_router());
    router = router.merge(r2_http::control_router());
    router = router.merge(scheduler_http::control_router());
    router = router.merge(queue_http::control_router());
    router = router.merge(workflow_http::control_router());
    router = router.merge(cache_images_http::control_router());
    router = router.merge(test_control_router());
    router
        .fallback(fallback)
        .layer(axum::middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state)
}

/// Merged public+admin on one listener.
pub fn merged_router(state: HttpState) -> Router {
    let metrics_enabled = state.metrics_enabled;
    let middleware_state = state.clone();
    let mut router = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/health/status", get(status));
    if metrics_enabled {
        router = router.route("/metrics", get(metrics_handler));
    }
    router
        .merge(workers_http::control_router())
        .merge(kv_http::control_router())
        .merge(r2_http::control_router())
        .merge(d1_http::control_router())
        .merge(do_http::control_router())
        .merge(scheduler_http::control_router())
        .merge(queue_http::control_router())
        .merge(workflow_http::control_router())
        .merge(cache_images_http::control_router())
        .merge(test_control_router())
        .fallback(workers_http::public_ingress)
        .layer(axum::middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state)
}

async fn live() -> StatusCode {
    StatusCode::OK
}

async fn ready(State(state): State<HttpState>) -> Response {
    let reason = state.health.readiness();
    if reason.is_ready() {
        StatusCode::OK.into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "code": reason.as_str() })),
        )
            .into_response()
    }
}

async fn status(State(state): State<HttpState>, request: Request) -> Response {
    if !authorize(&state, &request) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let snap = state.health.snapshot();
    let supervisor = (state.supervisor)();
    Json(serde_json::json!({
        "readiness": snap.readiness,
        "components": snap.components,
        "supervisor": supervisor,
    }))
    .into_response()
}

async fn metrics_handler(State(state): State<HttpState>, request: Request) -> Response {
    if !authorize(&state, &request) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body = state.metrics.render(&state.health.snapshot());
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE))],
        body,
    )
        .into_response()
}

#[cfg(any(test, feature = "test-support"))]
fn test_control_router() -> Router<HttpState> {
    Router::new().route(
        "/__test/runtime/restart",
        axum::routing::post(test_runtime_restart),
    )
}

#[cfg(not(any(test, feature = "test-support")))]
fn test_control_router() -> Router<HttpState> {
    Router::new()
}

#[cfg(any(test, feature = "test-support"))]
async fn test_runtime_restart(State(state): State<HttpState>, request: Request) -> StatusCode {
    if !authorize(&state, &request) {
        return StatusCode::UNAUTHORIZED;
    }
    if request
        .headers()
        .get("x-open-compute-test-ack")
        .is_none_or(|value| value != "restart-generation")
    {
        return StatusCode::BAD_REQUEST;
    }
    match &state.test_runtime_restart {
        Some(restart) if restart() => StatusCode::ACCEPTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(crate) fn authorize(state: &HttpState, request: &Request) -> bool {
    let Some(secret) = &state.admin_secret else {
        return true;
    };
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    bearer_matches(header, secret)
}

async fn fallback(method: Method) -> StatusCode {
    if matches!(
        method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::DELETE | Method::PATCH
    ) {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::METHOD_NOT_ALLOWED
    }
}

async fn bounds_middleware(
    State(state): State<HttpState>,
    mut request: Request,
    next: axum::middleware::Next,
) -> Result<Response, StatusCode> {
    let start = Instant::now();
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && matches!(
            request.uri().path(),
            "/health/live" | "/health/ready" | "/health/status" | "/metrics"
        )
    {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let direct_deployment_upload = request.uri().path().starts_with("/v1/accounts/")
        && request.uri().path().ends_with("/deployments");
    let staged_deployment_upload = is_staged_deployment_upload(request.uri().path());
    let mut header_total = 0_usize;
    for (name, value) in request.headers() {
        let value_limit = if direct_deployment_upload
            && name.as_str() == workers_http::DEPLOYMENT_METADATA_HEADER
        {
            workers_http::MAX_DEPLOYMENT_METADATA_HEADER_BYTES
        } else {
            MAX_HEADER_BYTES
        };
        if value.len() > value_limit || name.as_str().len() > 256 {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        header_total = header_total
            .saturating_add(name.as_str().len())
            .saturating_add(value.len());
        let total_limit = if direct_deployment_upload {
            MAX_DEPLOYMENT_HEADER_TOTAL
        } else {
            MAX_HEADER_TOTAL
        };
        if header_total > total_limit {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
    }
    let body_limit = if direct_deployment_upload || staged_deployment_upload {
        workers_http::HARD_MAX_BUNDLE_BODY
    } else {
        MAX_BODY
    };
    if let Some(len) = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        && len > body_limit
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let id = RequestId::generate();
    request.extensions_mut().insert(id);
    let route = request.uri().path().to_owned();
    let is_mutation = !matches!(*request.method(), Method::GET | Method::HEAD);
    let method = bound_method(request.method());
    let mut response = next.run(request).await;
    if let Some(error) = response.extensions().get::<ProductErrorCode>()
        && let Some(operation) = product_operation(&route)
    {
        state.metrics.observe_product_error(operation, error.0);
        if matches!(
            error.0,
            ErrorCode::QuotaExceeded
                | ErrorCode::AdmissionBusy
                | ErrorCode::StoragePressure
                | ErrorCode::DiskHardLimit
                | ErrorCode::PlatformUnavailable
        ) {
            state.metrics.observe_admission(operation, Some(error.0));
        }
    } else if is_mutation
        && response.status().is_success()
        && let Some(operation) = product_operation(&route)
    {
        state.metrics.observe_admission(operation, None);
    }
    let status = response.status().as_u16();
    response.headers_mut().insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&id.to_string()).unwrap_or(HeaderValue::from_static("invalid")),
    );
    tracing::info!(
        request_id = %id,
        route = %bound_route(&route),
        method,
        status,
        duration_ms = start.elapsed().as_millis() as u64,
        "http"
    );
    Ok(response)
}

fn is_staged_deployment_upload(path: &str) -> bool {
    let parts = path
        .strip_prefix('/')
        .unwrap_or(path)
        .split('/')
        .collect::<Vec<_>>();
    parts.len() >= 6
        && parts[0] == "v1"
        && parts[1] == "accounts"
        && !parts[2].is_empty()
        && parts[3] == "workers"
        && !parts[4].is_empty()
        && parts[5] == "deployment-uploads"
        && (parts.len() == 6 || !parts[6].is_empty())
        && (parts.len() == 6
            || parts.len() == 7
            || (parts.len() == 8 && parts[7] == "finalize")
            || (parts.len() == 9 && parts[7] == "objects" && !parts[8].is_empty()))
}

fn product_operation(path: &str) -> Option<OperationClass> {
    if path.contains("/workers") {
        Some(OperationClass::Workers)
    } else if path.contains("/kv/") {
        Some(OperationClass::Kv)
    } else if path.contains("/r2/") {
        Some(OperationClass::R2)
    } else if path.contains("/d1/") {
        Some(OperationClass::D1)
    } else if path.contains("/durable-objects/") {
        Some(OperationClass::DurableObjects)
    } else if path.contains("/queues") {
        Some(OperationClass::Scheduler)
    } else {
        None
    }
}

fn bound_route(path: &str) -> &'static str {
    match path {
        "/health/live" => "/health/live",
        "/health/ready" => "/health/ready",
        "/health/status" => "/health/status",
        "/metrics" => "/metrics",
        _ if path.starts_with("/v1/accounts/") => "/v1/accounts/:account/workers/*",
        _ if path.starts_with("/__workers/") => "/__workers/:account/:worker/*",
        _ => "/other",
    }
}

fn bound_method(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        _ => "OTHER",
    }
}

/// Bind a TCP listener on `addr`.
pub async fn bind(
    addr: std::net::SocketAddr,
) -> Result<TcpListener, open_compute_core::PlatformError> {
    TcpListener::bind(addr).await.map_err(|_| {
        open_compute_core::PlatformError::new(
            ErrorCode::ConfigInvalid,
            "failed to bind health listener",
        )
    })
}

/// Serve a router until `shutdown` resolves.
pub async fn serve_until(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), open_compute_core::PlatformError> {
    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| {
            open_compute_core::PlatformError::new(
                ErrorCode::ConfigInvalid,
                "health listener failed",
            )
        })
}

impl HttpState {
    /// Health coordinator handle.
    #[must_use]
    pub fn health(&self) -> &HealthCoordinator {
        &self.health
    }
}

/// Empty body type alias used by tests.
pub type EmptyBody = Body;

#[cfg(test)]
#[path = "http_tests.rs"]
mod coverage_tests;
