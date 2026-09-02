//! Fixed health/metrics HTTP surface.

use crate::auth::{bearer_matches, resolve_admin_auth, resolve_bearer_auth};
use crate::cache_images_http::CacheImagesApiState;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::d1_http::D1ApiState;
use crate::dashboard::DashboardDispatch;
use crate::do_http::DoApiState;
use crate::health::HealthCoordinator;
use crate::kv_http::{self, KvApiState};
use crate::metrics::{CONTENT_TYPE, MetricsRegistry};
use crate::queue_http::QueueApiState;
use crate::r2_http::{self, R2ApiState};
use crate::scheduler::SchedulerService;
use crate::search_http::SearchApiState;
use crate::workers_http::{self, WorkerApiState};
use crate::workflow_http::WorkflowApiState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use open_compute_core::config::ServerConfig;
use open_compute_core::{
    ErrorCode, OperationClass, PlatformError, ReadinessReason, RequestId, SecretString,
};
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use open_compute_storage::PlatformStorage;
use serde::Serialize;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// Response header carrying the generated request ID.
pub const REQUEST_ID_HEADER: &str = "x-open-compute-request-id";
const MAX_BODY: usize = 4096;
const MAX_HEADER_BYTES: usize = 8192;
const MAX_HEADER_TOTAL: usize = 16_384;

/// Stable error metadata attached internally for low-cardinality product metrics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProductErrorCode(pub ErrorCode);

/// Shared HTTP state.
#[derive(Clone)]
pub struct HttpState {
    health: HealthCoordinator,
    metrics: Arc<MetricsRegistry>,
    metrics_enabled: bool,
    dashboard_enabled: bool,
    admin_secret: Option<Arc<SecretString>>,
    deployer_secret: Option<Arc<SecretString>>,
    read_only_secret: Option<Arc<SecretString>>,
    cloudflare_v4_account: Option<Arc<AccountAuthority>>,
    platform_storage: Option<Arc<PlatformStorage>>,
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
    dashboard_dispatch: Arc<RwLock<Option<DashboardDispatch>>>,
    search_api: Option<Arc<SearchApiState>>,
}

impl std::fmt::Debug for HttpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpState")
            .field("metrics_enabled", &self.metrics_enabled)
            .field("dashboard_enabled", &self.dashboard_enabled)
            .field("admin_auth", &self.admin_secret.is_some())
            .field("deployer_auth", &self.deployer_secret.is_some())
            .field("read_only_auth", &self.read_only_secret.is_some())
            .field(
                "cloudflare_v4_account",
                &self.cloudflare_v4_account.is_some(),
            )
            .field("platform_storage", &self.platform_storage.is_some())
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
            .field("dashboard_dispatch", &"<async>")
            .field("search_api", &self.search_api.is_some())
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
        dashboard_enabled: bool,
        server: &ServerConfig,
        supervisor: Arc<dyn Fn() -> Option<SanitizedSupervisor> + Send + Sync>,
    ) -> Result<Self, PlatformError> {
        let admin_secret = Arc::new(resolve_admin_auth(&server.admin_auth)?);
        let deployer_secret = Arc::new(resolve_bearer_auth(&server.deployer_auth)?);
        let read_only_secret = Arc::new(resolve_bearer_auth(&server.read_only_auth)?);
        if admin_secret.expose() == deployer_secret.expose()
            || admin_secret.expose() == read_only_secret.expose()
            || deployer_secret.expose() == read_only_secret.expose()
        {
            return Err(PlatformError::new(
                ErrorCode::SecretRefInvalid,
                "server Bearer tokens must be distinct",
            ));
        }
        Ok(Self {
            health,
            metrics,
            metrics_enabled,
            dashboard_enabled,
            admin_secret: Some(admin_secret),
            deployer_secret: Some(deployer_secret),
            read_only_secret: Some(read_only_secret),
            cloudflare_v4_account: None,
            platform_storage: None,
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
            dashboard_dispatch: Arc::new(RwLock::new(None)),
            search_api: None,
        })
    }

    /// Share the dashboard dispatch slot populated after runtime bootstrap.
    #[must_use]
    pub fn with_dashboard_dispatch(
        mut self,
        dispatch: Arc<RwLock<Option<DashboardDispatch>>>,
    ) -> Self {
        self.dashboard_dispatch = dispatch;
        self
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
            dashboard_enabled: false,
            admin_secret: admin_secret.map(Arc::new),
            deployer_secret: None,
            read_only_secret: None,
            cloudflare_v4_account: None,
            platform_storage: None,
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
            dashboard_dispatch: Arc::new(RwLock::new(None)),
            search_api: None,
        }
    }

    /// Enable dashboard surface responses in tests without runtime bootstrap.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_dashboard_enabled(mut self, enabled: bool) -> Self {
        self.dashboard_enabled = enabled;
        self
    }

    /// Override supervisor snapshot reporting for operator status tests.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_supervisor(
        mut self,
        supervisor: Arc<dyn Fn() -> Option<SanitizedSupervisor> + Send + Sync>,
    ) -> Self {
        self.supervisor = supervisor;
        self
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

    /// Attach Vectorize and AI Search operator lifecycle authority.
    #[must_use]
    pub fn with_search_api(mut self, api: SearchApiState) -> Self {
        self.search_api = Some(Arc::new(api));
        self
    }

    /// Borrow the optional Vectorize and AI Search operator authority.
    #[must_use]
    pub(crate) fn search_api(&self) -> Option<&Arc<SearchApiState>> {
        self.search_api.as_ref()
    }

    /// Borrow the resolved admin capability without exposing its value.
    #[must_use]
    pub(crate) fn admin_secret(&self) -> Option<&SecretString> {
        self.admin_secret.as_deref()
    }

    /// Borrow the resolved deployer capability without exposing its value.
    #[must_use]
    pub(crate) fn deployer_secret(&self) -> Option<&SecretString> {
        self.deployer_secret.as_deref()
    }

    /// Borrow the resolved read-only capability without exposing its value.
    #[must_use]
    pub(crate) fn read_only_secret(&self) -> Option<&SecretString> {
        self.read_only_secret.as_deref()
    }

    /// Attach three distinct v4 Bearer capabilities in test-support builds.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub(crate) fn with_v4_tokens(
        mut self,
        deployer: SecretString,
        read_only: SecretString,
    ) -> Self {
        self.deployer_secret = Some(Arc::new(deployer));
        self.read_only_secret = Some(Arc::new(read_only));
        self
    }

    /// Attach the stable one-account Cloudflare v4 identity mapping.
    #[must_use]
    pub(crate) fn with_cloudflare_v4_account(mut self, authority: AccountAuthority) -> Self {
        self.cloudflare_v4_account = Some(Arc::new(authority));
        self
    }

    /// Borrow the stable one-account Cloudflare v4 identity mapping.
    #[must_use]
    pub(crate) fn cloudflare_v4_account(&self) -> Option<&AccountAuthority> {
        self.cloudflare_v4_account.as_deref()
    }

    /// Attach the one platform persistence authority for v4 vendor inspection.
    #[must_use]
    pub(crate) fn with_platform_storage(mut self, storage: Arc<PlatformStorage>) -> Self {
        self.platform_storage = Some(storage);
        self
    }

    /// Borrow the one platform persistence authority.
    #[must_use]
    pub(crate) fn platform_storage(&self) -> Option<&Arc<PlatformStorage>> {
        self.platform_storage.as_ref()
    }
}

/// Public routes only.
pub fn public_router(state: HttpState) -> Router {
    let middleware_state = state.clone();
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .merge(removed_management_router(true))
        .fallback(workers_http::public_ingress)
        .layer(middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state)
}

/// Admin routes, including public health plus operator API and metrics.
pub fn admin_router(state: HttpState) -> Router {
    let metrics_enabled = state.metrics_enabled;
    let middleware_state = state.clone();
    let v4_state = state.clone();
    let mut router = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .nest(
            "/client/v4",
            crate::cloudflare_v4::router(v4_state, Router::new()),
        )
        .merge(removed_management_router(false));
    if metrics_enabled {
        router = router.route("/metrics", get(metrics_handler));
    }
    router = router
        .route("/operator", any(operator_surface))
        .route("/operator/", any(operator_surface))
        .route("/operator/{*rest}", any(operator_surface))
        .merge(test_control_router());
    router
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state)
}

/// Merged public+admin on one listener.
pub fn merged_router(state: HttpState) -> Router {
    let metrics_enabled = state.metrics_enabled;
    let middleware_state = state.clone();
    let v4_state = state.clone();
    let mut router = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .nest(
            "/client/v4",
            crate::cloudflare_v4::router(v4_state, Router::new()),
        )
        .merge(removed_management_router(false));
    if metrics_enabled {
        router = router.route("/metrics", get(metrics_handler));
    }
    router
        .route("/operator", any(operator_surface))
        .route("/operator/", any(operator_surface))
        .route("/operator/{*rest}", any(operator_surface))
        .merge(test_control_router())
        .fallback(workers_http::public_ingress)
        .layer(middleware::from_fn_with_state(
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

fn removed_management_router(reserve_v4: bool) -> Router<HttpState> {
    let mut router = Router::new()
        .route("/operator/api", any(neutral_not_found))
        .route("/operator/api/{*rest}", any(neutral_not_found))
        .route("/operator/metrics", any(neutral_not_found))
        .route("/v1", any(neutral_not_found))
        .route("/v1/{*rest}", any(neutral_not_found));
    if reserve_v4 {
        router = router
            .route("/client/v4", any(neutral_not_found))
            .route("/client/v4/{*rest}", any(neutral_not_found));
    }
    router
}

async fn neutral_not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn metrics_handler(State(state): State<HttpState>, request: Request) -> Response {
    if !authorize(&state, &request) {
        let request_id = operator_request_id(&request);
        let error = PlatformError::new(
            ErrorCode::AdminAuthRequired,
            "admin authentication is required",
        );
        return operator_error_response(&error, request_id);
    }
    let body = state.metrics.render(&state.health.snapshot());
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE))],
        body,
    )
        .into_response()
}

async fn operator_surface(State(state): State<HttpState>, request: Request) -> Response {
    if !state.dashboard_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dispatch = state.dashboard_dispatch.read().await.clone();
    let Some(dispatch) = dispatch else {
        return dashboard_not_ready().into_response();
    };
    let (mut parts, body) = request.into_parts();
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost")
        .to_owned();
    let asset_path = dashboard_asset_path(parts.uri.path());
    let query = parts
        .uri
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    let Ok(uri) = Uri::builder()
        .path_and_query(format!("{asset_path}{query}"))
        .build()
    else {
        return dashboard_not_ready().into_response();
    };
    parts.uri = uri;
    if !parts.headers.contains_key(header::HOST) {
        parts.headers.insert(
            header::HOST,
            HeaderValue::from_str(&host).unwrap_or(HeaderValue::from_static("localhost")),
        );
    }
    let request = Request::from_parts(parts, body);
    match dispatch.dispatch(request).await {
        Ok(response) => apply_dashboard_security_headers(response),
        Err(_) => dashboard_not_ready().into_response(),
    }
}

fn apply_dashboard_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; object-src 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

pub(crate) fn dashboard_asset_path(request_path: &str) -> String {
    let stripped = request_path
        .strip_prefix("/operator")
        .unwrap_or(request_path);
    if stripped.is_empty() || stripped == "/" {
        "/".to_owned()
    } else if stripped.starts_with('/') {
        stripped.to_owned()
    } else {
        format!("/{stripped}")
    }
}

fn dashboard_not_ready() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": "platform_unavailable",
                "message": "dashboard is not ready",
            }
        })),
    )
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
        return false;
    };
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    bearer_matches(header, secret)
}

fn operator_request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

pub(crate) fn operator_error_response(error: &PlatformError, request_id: RequestId) -> Response {
    let code = error.code();
    let status = match code {
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::AccountNotFound
        | ErrorCode::WorkerNotFound
        | ErrorCode::VersionNotFound
        | ErrorCode::RouteNotFound
        | ErrorCode::EntrypointNotFound
        | ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::WorkerDeleted => StatusCode::GONE,
        ErrorCode::WorkerNameConflict | ErrorCode::RouteConflict => StatusCode::CONFLICT,
        ErrorCode::VersionNotReady
        | ErrorCode::VersionActive
        | ErrorCode::VersionReferenced
        | ErrorCode::ServiceTargetReferenced
        | ErrorCode::QueueConsumerGenerationStale
        | ErrorCode::IdempotencyConflict
        | ErrorCode::AssetUploadIncomplete
        | ErrorCode::AssetUploadConflict => StatusCode::CONFLICT,
        ErrorCode::BundleTooLarge | ErrorCode::LimitInvalid | ErrorCode::AssetLimitExceeded => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ErrorCode::BundleRuntimeInvalid
        | ErrorCode::CompatibilityUnsupported
        | ErrorCode::AssetConfigUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::RuntimeUnavailable
        | ErrorCode::ArtifactUnavailable
        | ErrorCode::CacheUnavailable
        | ErrorCode::AssetStorageUnavailable
        | ErrorCode::SchedulerUnavailable
        | ErrorCode::SchedulerCorrupt
        | ErrorCode::SchedulerBusy
        | ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ResourceLimitExceeded | ErrorCode::QuotaExceeded | ErrorCode::AdmissionBusy => {
            StatusCode::TOO_MANY_REQUESTS
        }
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit => StatusCode::INSUFFICIENT_STORAGE,
        ErrorCode::Internal
        | ErrorCode::CacheCorrupt
        | ErrorCode::RuntimeResultUnknown
        | ErrorCode::VersionInvariantViolation
        | ErrorCode::ArtifactIntegrityError
        | ErrorCode::AssetIntegrityError => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut response = (
        status,
        Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": code.as_str(),
                "message": error.message(),
                "requestId": request_id,
            }
        })),
    )
        .into_response();
    response.extensions_mut().insert(ProductErrorCode(code));
    response
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
    next: Next,
) -> Result<Response, StatusCode> {
    let start = Instant::now();
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && matches!(
            request.uri().path(),
            "/health/live" | "/health/ready" | "/metrics"
        )
    {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let worker_upload = is_v4_worker_upload(request.uri().path());
    let mut header_total = 0_usize;
    for (name, value) in request.headers() {
        if value.len() > MAX_HEADER_BYTES || name.as_str().len() > 256 {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        header_total = header_total
            .saturating_add(name.as_str().len())
            .saturating_add(value.len());
        if header_total > MAX_HEADER_TOTAL {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
    }
    let kv_value_put = request.method() == Method::PUT
        && kv_http::operator_kv_value_put_path(request.uri().path());
    let r2_object_put = request.method() == Method::PUT
        && r2_http::operator_r2_object_put_path(request.uri().path());
    let body_limit = if worker_upload {
        workers_http::HARD_MAX_BUNDLE_BODY
    } else if kv_value_put {
        kv_http::KV_OPERATOR_PUT_MAX_BODY
    } else if r2_object_put {
        r2_http::R2_OPERATOR_PUT_MAX_BODY
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

fn is_v4_worker_upload(path: &str) -> bool {
    let parts = path
        .strip_prefix('/')
        .unwrap_or(path)
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() < 7
        || parts[0] != "client"
        || parts[1] != "v4"
        || parts[2] != "accounts"
        || parts[3].is_empty()
        || parts[4] != "workers"
    {
        return false;
    }
    (parts.len() == 8 && parts[5] == "scripts" && !parts[6].is_empty() && parts[7] == "versions")
        || (parts.len() == 7 && parts[5] == "scripts" && !parts[6].is_empty())
        || (parts.len() == 8
            && parts[5] == "assets"
            && parts[6] == "upload"
            && !parts[7].is_empty())
        || (parts.len() == 7 && parts[5] == "assets" && parts[6] == "upload")
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
        "/metrics" => "/metrics",
        _ if path.starts_with("/client/v4/accounts/") => "/client/v4/accounts/:account/*",
        _ if path.starts_with("/client/v4/open-compute/") => "/client/v4/open-compute/*",
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
pub async fn bind(addr: std::net::SocketAddr) -> Result<TcpListener, PlatformError> {
    TcpListener::bind(addr)
        .await
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "failed to bind health listener"))
}

/// Serve a router until `shutdown` resolves.
pub async fn serve_until(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "health listener failed"))
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
