//! Worker v4 management API state and public route ingress.

use crate::asset_backend::pin_response;
use crate::http::HttpState;
use crate::runtime_bridge::{DispatchTarget, WorkerdTransport};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{ErrorCode, InstanceId, PlatformError, RequestId, VersionId, WorkerId};
use open_compute_storage::{PlatformStorage, WorkerOriginExposure, WorkerRepository};
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
    observability: Option<Arc<crate::observability::ObservabilityService>>,
    traffic: Arc<WorkerTrafficRegistry>,
    upload_serial: Arc<tokio::sync::Mutex<()>>,
    local_extensions: Arc<crate::local_extensions::LocalExtensionRegistry>,
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
    /// Read secret-free deployment admission counts for the namespaced status API.
    pub(crate) fn deployment_runtime_assessments(
        &self,
    ) -> Result<open_compute_storage::DeploymentRuntimeAssessmentSummary, PlatformError> {
        WorkerRepository::new(self.storage.db()).deployment_runtime_assessments()
    }

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
            observability: None,
            traffic: Arc::new(WorkerTrafficRegistry::default()),
            upload_serial: Arc::new(tokio::sync::Mutex::new(())),
            local_extensions: Arc::new(crate::local_extensions::LocalExtensionRegistry::empty()),
        }
    }

    pub(crate) fn with_local_extensions(
        mut self,
        local_extensions: Arc<crate::local_extensions::LocalExtensionRegistry>,
    ) -> Self {
        self.local_extensions = local_extensions;
        self
    }

    pub(crate) fn local_extension_exists(&self, name: &str) -> bool {
        self.local_extensions.contains(name)
    }

    pub(crate) fn local_service_target(
        &self,
        name: &str,
        account_id: InstanceId,
        worker_id: WorkerId,
        version_id: Option<VersionId>,
        entrypoint: Option<&str>,
    ) -> Option<open_compute_storage::ServiceTarget> {
        self.local_extensions
            .service_target(name, account_id, worker_id, version_id, entrypoint)
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

    /// Attach the Workers Logs and realtime-tail authority.
    #[must_use]
    pub(crate) fn with_observability(
        mut self,
        observability: Arc<crate::observability::ObservabilityService>,
    ) -> Self {
        self.observability = Some(observability);
        self
    }

    /// Borrow the Workers Logs and realtime-tail authority.
    pub(crate) fn observability(
        &self,
    ) -> Result<&Arc<crate::observability::ObservabilityService>, PlatformError> {
        self.observability.as_ref().ok_or_else(|| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "observability service is unavailable",
            )
        })
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

/// Resolve a local Worker origin from the public HTTP listener.
pub async fn local_ingress(State(state): State<HttpState>, mut request: Request) -> Response {
    request.headers_mut().remove("cf-connecting-ip");
    if let Some(peer) = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|value| value.0.ip().to_string())
        && let Ok(value) = axum::http::HeaderValue::from_str(&peer)
    {
        request.headers_mut().insert("cf-connecting-ip", value);
    }
    dispatch_ingress(state, request, WorkerOriginExposure::Local).await
}

/// Resolve a public Worker origin only on the private Caddy upstream listener.
pub async fn gateway_ingress(State(state): State<HttpState>, mut request: Request) -> Response {
    let client_ip = request
        .headers()
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<std::net::IpAddr>().ok());
    request.headers_mut().remove("cf-connecting-ip");
    if let Some(client_ip) = client_ip
        && let Ok(value) = axum::http::HeaderValue::from_str(&client_ip.to_string())
    {
        request.headers_mut().insert("cf-connecting-ip", value);
    }
    request
        .extensions_mut()
        .insert(crate::runtime_bridge::TrustedHttpsOrigin);
    dispatch_ingress(state, request, WorkerOriginExposure::Public).await
}

async fn dispatch_ingress(
    state: HttpState,
    request: Request,
    exposure: WorkerOriginExposure,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = state.worker_api() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let hostname = match request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| PlatformError::new(ErrorCode::RouteNotFound, "Host header is required"))
        .and_then(|value| {
            canonical_request_host(
                value,
                if exposure == WorkerOriginExposure::Local {
                    state.local_origin_port()
                } else {
                    Some(443)
                },
            )
        }) {
        Ok(hostname) => hostname,
        Err(error) => return crate::http::platform_error_response(&error, request_id),
    };
    let repo = WorkerRepository::new(api.storage.db());
    let snapshot = match repo.resolve_route(&hostname, request.uri().path(), exposure) {
        Ok(snapshot) => snapshot,
        Err(error) => return crate::http::platform_error_response(&error, request_id),
    };
    let Some(deployment_id) = snapshot.worker.active_deployment_id else {
        return crate::http::platform_error_response(
            &PlatformError::new(ErrorCode::RouteNotFound, "route has no active deployment"),
            request_id,
        );
    };
    let pin = match api.pins.pin_deployment(snapshot.version.id, deployment_id) {
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
        instance_id: snapshot.route.instance_id,
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

pub(crate) fn canonical_request_host(
    value: &str,
    expected_port: Option<u16>,
) -> Result<String, PlatformError> {
    let authority = value.parse::<axum::http::uri::Authority>().map_err(|_| {
        PlatformError::new(ErrorCode::RouteNotFound, "public request Host is invalid")
    })?;
    if authority
        .port_u16()
        .is_some_and(|port| Some(port) != expected_port)
    {
        return Err(PlatformError::new(
            ErrorCode::RouteNotFound,
            "public request Host port does not match the listener",
        ));
    }
    canonical_hostname(authority.host())
}

fn canonical_hostname(value: &str) -> Result<String, PlatformError> {
    if value.is_empty()
        || value.len() > 253
        || value.ends_with('.')
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
        || value.contains(['/', '@', '#', '?'])
    {
        return Err(PlatformError::new(
            ErrorCode::RouteNotFound,
            "public request Host is invalid",
        ));
    }
    let parsed = url::Host::parse(value).map_err(|_| {
        PlatformError::new(ErrorCode::RouteNotFound, "public request Host is invalid")
    })?;
    match parsed {
        url::Host::Domain(host) if host == value => Ok(host),
        _ => Err(PlatformError::new(
            ErrorCode::RouteNotFound,
            "public request Host is invalid",
        )),
    }
}

#[cfg(test)]
mod local_origin_tests {
    use super::*;

    #[test]
    fn canonical_local_host_rejects_aliases_and_port_spoofing() {
        assert_eq!(
            canonical_request_host("app.account.localhost:8787", Some(8787)).unwrap(),
            "app.account.localhost"
        );
        for value in [
            "APP.account.localhost:8787",
            "app.account.localhost.:8787",
            "app.account.localhost:9999",
            "127.0.0.1:8787",
        ] {
            assert!(
                canonical_request_host(value, Some(8787)).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn canonical_public_host_accepts_only_external_https_port() {
        assert_eq!(
            canonical_request_host("app.compute.example.com:443", Some(443)).unwrap(),
            "app.compute.example.com"
        );
        assert!(canonical_request_host("app.compute.example.com:8443", Some(443)).is_err());
    }
}
