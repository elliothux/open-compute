//! Private `RuntimeSource` listener and streaming platformd-to-workerd transport.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine as _;
use http_body_util::Limited;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_core::{
    AccountId, CronSchedule, DeploymentId, ErrorCode, PlatformError, QueueMessageId, RequestId,
    WorkerId,
};
use open_compute_runtime::{
    GenerationAuthRegistry, SupervisorState, TOKEN_HEADER, WorkerdSupervisor,
};
use open_compute_storage::{
    AuthorizedDurableObjectDelete, ClaimedJob, QUEUE_MAX_MESSAGE_BYTES, QueueContentType,
};
use open_compute_workers::{
    RuntimeScope, RuntimeSource, RuntimeValidator, ValidationCandidate, loader_key,
};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

const SOURCE_PATH: &str = "/internal/runtime/v1/deployments/resolve";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const MAX_SOURCE_REQUEST: usize = 4096;
const DEFAULT_MAX_TENANT_BODY: usize = 16 * 1024 * 1024;
const RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CUSTOM_EVENT_RESPONSE: usize = 64 * 1024;
const MAX_QUEUE_CUSTOM_EVENT_REQUEST: usize = 18 * 1024 * 1024;

/// Internal-only observation of the native `WorkerLoader` cache path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoaderOutcome {
    /// The native `LOADER.get()` callback was invoked.
    Cold,
    /// The immutable key was already present in the workerd process.
    Warm,
}

/// Object-local result of one private Durable Object alarm delivery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AlarmDispatchResult {
    /// Stable state-machine outcome.
    pub outcome: AlarmDispatchOutcome,
    /// Authoritative due time for `not_due` or `retry`.
    #[serde(default)]
    pub scheduled_time_ms: Option<i64>,
    /// Authoritative retry count for `not_due` or `retry`.
    #[serde(default)]
    pub retry_count: Option<u8>,
    /// Stable low-cardinality tenant error code.
    #[serde(default)]
    pub error_code: Option<String>,
}

/// Stable private alarm delivery outcomes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AlarmDispatchOutcome {
    /// Handler completed and consumed the exact authority row.
    Success,
    /// Object authority no longer matches this projection.
    Stale,
    /// Object authority is valid but its due time moved forward.
    NotDue,
    /// Handler failed and object authority scheduled the next bounded retry.
    Retry,
    /// The sixth automatic retry failed and object authority was removed.
    Exhausted,
}

/// Strict object-local alarm DTO returned to bounded projection repair.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AlarmRepairResult {
    /// Whether valid object-local alarm authority exists.
    pub exists: bool,
    /// Authoritative due time when `exists`.
    #[serde(default)]
    pub scheduled_time_ms: Option<i64>,
    /// Authoritative retry count when `exists`.
    #[serde(default)]
    pub retry_count: Option<u8>,
    /// Authoritative row token when `exists`.
    #[serde(default)]
    pub row_token: Option<String>,
}

/// One trusted message delivered through the native Queue custom-event path.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDispatchMessage {
    /// Immutable scheduler message identity.
    pub id: String,
    /// Original enqueue timestamp.
    pub timestamp_ms: i64,
    /// One-based product delivery attempt exposed to the handler.
    pub attempts: u16,
    /// Persisted body representation.
    pub content_type: QueueContentType,
    /// Standard-base64 serialized body bytes.
    pub body_base64: String,
}

/// Trusted native Queue custom-event request assembled after a durable claim.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDispatchRequest {
    /// Tenant-visible Queue name.
    pub queue_name: String,
    /// Bounded claimed membership in deterministic order.
    pub messages: Vec<QueueDispatchMessage>,
}

/// Native batch-level retry decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueRetryBatchResult {
    /// Whether remaining undecided messages should retry.
    pub retry: bool,
    /// Optional explicit retry delay.
    #[serde(default)]
    pub delay_seconds: Option<i64>,
}

/// Native per-message retry decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueRetryMessageResult {
    /// Claimed message identity.
    pub msg_id: String,
    /// Optional explicit retry delay.
    #[serde(default)]
    pub delay_seconds: Option<i64>,
}

/// Strict result returned by workerd's native Queue dispatcher.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueDispatchResult {
    /// Pinned workerd event outcome spelling.
    pub outcome: String,
    /// Native batch acknowledgement flag.
    pub ack_all: bool,
    /// Native batch retry decision.
    pub retry_batch: QueueRetryBatchResult,
    /// Native explicit acknowledgement identities.
    pub explicit_acks: Vec<String>,
    /// Native explicit retry decisions.
    pub retry_messages: Vec<QueueRetryMessageResult>,
}

/// Trusted scheduled custom-event request.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledDispatchRequest {
    /// Logical UTC slot in Unix milliseconds.
    pub scheduled_time_ms: i64,
    /// Exact deployment-declared expression.
    pub cron: String,
}

/// Strict result returned by workerd's native scheduled dispatcher.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledDispatchResult {
    /// Pinned workerd event outcome spelling.
    pub outcome: String,
    /// Whether `controller.noRetry()` disabled product retry.
    pub no_retry: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlarmObjectRequest<'a> {
    namespace_resource_id: open_compute_core::ResourceId,
    object_id: open_compute_core::DurableObjectId,
    object_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    row_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_count: Option<u8>,
}

/// Immutable target frozen by route resolution or deployment validation.
#[derive(Clone, Debug)]
pub struct DispatchTarget {
    /// Account authority.
    pub account_id: AccountId,
    /// Worker authority.
    pub worker_id: WorkerId,
    /// Deployment authority.
    pub deployment_id: DeploymentId,
    /// Expected immutable descriptor digest.
    pub worker_code_sha256: String,
    /// Optional named entrypoint.
    pub entrypoint: Option<String>,
    /// Route generation observed at the `SQLite` linearization point.
    pub route_generation: i64,
    /// Platform-generated request identity.
    pub request_id: RequestId,
}

impl DispatchTarget {
    fn loader_key(&self) -> String {
        loader_key(self.account_id, self.worker_id, self.deployment_id)
    }
}

#[derive(Clone)]
struct SourceState {
    source: RuntimeSource,
    auth: GenerationAuthRegistry,
}

/// Bind the private `RuntimeSource` endpoint to an ephemeral IPv4 loopback port.
pub async fn bind_runtime_source() -> Result<TcpListener, PlatformError> {
    TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "failed to bind private RuntimeSource listener",
            )
        })
}

/// Serve `RuntimeSource` without the public HTTP logging/body middleware.
pub async fn serve_runtime_source(
    listener: TcpListener,
    source: RuntimeSource,
    auth: GenerationAuthRegistry,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let state = SourceState { source, auth };
    let router = Router::new()
        .route(SOURCE_PATH, post(resolve))
        .with_state(state);
    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "private RuntimeSource listener failed",
            )
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    startup_generation: String,
    key: String,
    expected_worker_code_sha256: String,
    scope: SourceScope,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SourceScope {
    Runtime,
    Validation,
    Probe,
}

async fn resolve(State(state): State<SourceState>, request: Request) -> Response {
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_SOURCE_REQUEST)
    {
        return source_error(ErrorCode::BundleTooLarge, StatusCode::PAYLOAD_TOO_LARGE);
    }
    let token = request
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let Ok(bytes) = to_bytes(request.into_body(), MAX_SOURCE_REQUEST).await else {
        return source_error(ErrorCode::BundleTooLarge, StatusCode::PAYLOAD_TOO_LARGE);
    };
    let body: ResolveRequest = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => return source_error(ErrorCode::BundleInvalid, StatusCode::BAD_REQUEST),
    };
    if !state.auth.authorize(&token, &body.startup_generation) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let scope = match body.scope {
        SourceScope::Runtime => RuntimeScope::Runtime,
        SourceScope::Validation => RuntimeScope::Validation,
        SourceScope::Probe => RuntimeScope::Probe,
    };
    let snapshot = match state
        .source
        .resolve(&body.key, &body.expected_worker_code_sha256, scope)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return source_platform_error(error),
    };
    let payload = match RuntimeSource::internal_payload(&snapshot) {
        Ok(payload) => payload,
        Err(error) => return source_platform_error(error),
    };
    let mut response = Response::new(Body::from(payload.expose().to_vec()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[allow(clippy::needless_pass_by_value)]
fn source_platform_error(error: PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::DeploymentNotReady => StatusCode::CONFLICT,
        ErrorCode::ArtifactUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ArtifactIntegrityError
        | ErrorCode::DeploymentInvariantViolation
        | ErrorCode::BundleInvalid
        | ErrorCode::BundleRuntimeInvalid => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    source_error(error.code(), status)
}

fn source_error(code: ErrorCode, status: StatusCode) -> Response {
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(code.as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(ERROR_HEADER), value);
    }
    response
}

/// Streaming client for the current workerd generation.
#[derive(Clone)]
pub struct WorkerdTransport {
    client: Client<HttpConnector, Body>,
    auth: GenerationAuthRegistry,
    supervisor: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>,
    max_request_body: usize,
    #[cfg(test)]
    test_endpoint: Option<u16>,
}

impl std::fmt::Debug for WorkerdTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerdTransport").finish_non_exhaustive()
    }
}

impl WorkerdTransport {
    /// Bind transport to the supervisor slot and generation credential authority.
    #[must_use]
    pub fn new(
        auth: GenerationAuthRegistry,
        supervisor: Arc<Mutex<Option<Arc<WorkerdSupervisor>>>>,
    ) -> Self {
        let mut connector = HttpConnector::new();
        connector.enforce_http(true);
        Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            auth,
            supervisor,
            max_request_body: DEFAULT_MAX_TENANT_BODY,
            #[cfg(test)]
            test_endpoint: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_endpoint(auth: GenerationAuthRegistry, port: u16) -> Self {
        let mut transport = Self::new(auth, Arc::new(Mutex::new(None)));
        transport.test_endpoint = Some(port);
        transport
    }

    /// Apply the host-observed streaming request body ceiling.
    #[must_use]
    pub fn with_max_request_body(mut self, max_request_body: usize) -> Self {
        self.max_request_body = max_request_body.max(1);
        self
    }

    /// Dispatch a public request to an already-frozen deployment target.
    pub async fn dispatch(
        &self,
        target: DispatchTarget,
        request: Request,
    ) -> Result<Response, PlatformError> {
        self.send(target, request, false, false).await
    }

    /// Deliver one frozen Queue claim through workerd's native custom-event API.
    pub async fn dispatch_queue(
        &self,
        target: &DispatchTarget,
        request: &QueueDispatchRequest,
        timeout: Duration,
    ) -> Result<QueueDispatchResult, PlatformError> {
        validate_queue_dispatch_request(request)?;
        let result = tokio::time::timeout(
            timeout,
            self.custom_event_request("/internal/queue", target, request),
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::QueueSendResultUnknown,
                "Queue custom-event result is unknown",
            )
        })??;
        validate_queue_dispatch_result(result, request.messages.len())
    }

    /// Deliver one frozen Cron run through workerd's native scheduled API.
    pub async fn dispatch_scheduled(
        &self,
        target: &DispatchTarget,
        request: &ScheduledDispatchRequest,
        timeout: Duration,
    ) -> Result<ScheduledDispatchResult, PlatformError> {
        validate_scheduled_dispatch_request(request)?;
        let result = tokio::time::timeout(
            timeout,
            self.custom_event_request("/internal/scheduled", target, request),
        )
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::SchedulerUnavailable,
                "scheduled custom-event result is unknown",
            )
        })??;
        validate_scheduled_dispatch_result(result)
    }

    async fn custom_event_request<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        target: &DispatchTarget,
        body: &impl Serialize,
    ) -> Result<T, PlatformError> {
        let (port, credential) = self.endpoint()?;
        if target.route_generation < 1 {
            return Err(custom_event_protocol_error());
        }
        let bytes = serde_json::to_vec(body).map_err(|_| custom_event_protocol_error())?;
        let mut request = hyper::Request::builder()
            .method(Method::POST)
            .uri(format!("http://127.0.0.1:{port}{path}"))
            .header(TOKEN_HEADER, credential.expose())
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-open-compute-account-id", target.account_id.to_string())
            .header("x-open-compute-worker-id", target.worker_id.to_string())
            .header(
                "x-open-compute-deployment-id",
                target.deployment_id.to_string(),
            )
            .header("x-open-compute-loader-key", target.loader_key())
            .header(
                "x-open-compute-worker-code-sha256",
                &target.worker_code_sha256,
            )
            .header(
                "x-open-compute-route-generation",
                target.route_generation.to_string(),
            )
            .header("x-open-compute-request-id", target.request_id.to_string());
        if let Some(entrypoint) = &target.entrypoint {
            request = request.header("x-open-compute-entrypoint", entrypoint);
        }
        let request = request
            .body(Body::from(bytes))
            .map_err(|_| custom_event_protocol_error())?;
        let response = self
            .client
            .request(request)
            .await
            .map_err(|_| runtime_unavailable())?;
        if !response.status().is_success() {
            return Err(PlatformError::new(
                if path == "/internal/queue" {
                    ErrorCode::QueueCustomEventUnsupported
                } else {
                    ErrorCode::CronCustomEventUnsupported
                },
                "native custom-event dispatch failed",
            ));
        }
        let bytes = to_bytes(Body::new(response.into_body()), MAX_CUSTOM_EVENT_RESPONSE)
            .await
            .map_err(|_| custom_event_protocol_error())?;
        serde_json::from_slice(&bytes).map_err(|_| custom_event_protocol_error())
    }

    /// Execute one trusted native facet delete after the control-plane fence commits.
    pub async fn delete_durable_object(
        &self,
        authority: &AuthorizedDurableObjectDelete,
    ) -> Result<(), PlatformError> {
        let (port, credential) = self.endpoint()?;
        let body = serde_json::to_vec(authority).map_err(|_| runtime_unavailable())?;
        let request = hyper::Request::builder()
            .method(Method::POST)
            .uri(format!("http://127.0.0.1:{port}/internal/do-delete"))
            .header(TOKEN_HEADER, credential.expose())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .map_err(|_| runtime_unavailable())?;
        let response = tokio::time::timeout(RESPONSE_HEADER_TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::DoDispatchTimeout,
                    "Durable Object delete result is unknown",
                )
            })?
            .map_err(|_| runtime_unavailable())?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(PlatformError::new(
                ErrorCode::DoStorageUnavailable,
                "Durable Object native delete did not complete",
            ))
        }
    }

    /// Deliver one scheduler claim through the generation-authenticated private alarm path.
    pub async fn dispatch_alarm(
        &self,
        job: &ClaimedJob,
        timeout: Duration,
    ) -> Result<AlarmDispatchResult, PlatformError> {
        let result = self
            .alarm_request(
                "/internal/do-alarm",
                &AlarmObjectRequest {
                    namespace_resource_id: job.namespace_resource_id,
                    object_id: job.object_id,
                    object_generation: job.object_generation,
                    row_token: Some(&job.row_token),
                    retry_count: Some(job.retry_count),
                },
                timeout,
            )
            .await?;
        validate_alarm_dispatch_result(result)
    }

    /// Deliver one scheduler claim while the scheduler clock owns the timeout.
    pub(crate) async fn dispatch_alarm_unbounded(
        &self,
        job: &ClaimedJob,
    ) -> Result<AlarmDispatchResult, PlatformError> {
        let result = self
            .alarm_request_unbounded(
                "/internal/do-alarm",
                &AlarmObjectRequest {
                    namespace_resource_id: job.namespace_resource_id,
                    object_id: job.object_id,
                    object_generation: job.object_generation,
                    row_token: Some(&job.row_token),
                    retry_count: Some(job.retry_count),
                },
            )
            .await?;
        validate_alarm_dispatch_result(result)
    }

    /// Probe one live object for bounded projection repair without exposing arbitrary SQL.
    pub async fn repair_alarm(
        &self,
        namespace_resource_id: open_compute_core::ResourceId,
        object_id: open_compute_core::DurableObjectId,
        object_generation: u64,
        timeout: Duration,
    ) -> Result<AlarmRepairResult, PlatformError> {
        let result = self
            .alarm_request(
                "/internal/do-alarm-repair",
                &AlarmObjectRequest {
                    namespace_resource_id,
                    object_id,
                    object_generation,
                    row_token: None,
                    retry_count: None,
                },
                timeout,
            )
            .await?;
        validate_alarm_repair_result(result)
    }

    /// Probe an Alarm projection while the scheduler clock owns the timeout.
    pub(crate) async fn repair_alarm_unbounded(
        &self,
        namespace_resource_id: open_compute_core::ResourceId,
        object_id: open_compute_core::DurableObjectId,
        object_generation: u64,
    ) -> Result<AlarmRepairResult, PlatformError> {
        let result = self
            .alarm_request_unbounded(
                "/internal/do-alarm-repair",
                &AlarmObjectRequest {
                    namespace_resource_id,
                    object_id,
                    object_generation,
                    row_token: None,
                    retry_count: None,
                },
            )
            .await?;
        validate_alarm_repair_result(result)
    }

    async fn alarm_request<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &AlarmObjectRequest<'_>,
        timeout: Duration,
    ) -> Result<T, PlatformError> {
        tokio::time::timeout(timeout, self.alarm_request_unbounded(path, body))
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::DoDispatchTimeout,
                    "Durable Object alarm dispatch result is unknown",
                )
            })?
    }

    async fn alarm_request_unbounded<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &AlarmObjectRequest<'_>,
    ) -> Result<T, PlatformError> {
        let (port, credential) = self.endpoint()?;
        let bytes = serde_json::to_vec(body).map_err(|_| alarm_protocol_error())?;
        let request = hyper::Request::builder()
            .method(Method::POST)
            .uri(format!("http://127.0.0.1:{port}{path}"))
            .header(TOKEN_HEADER, credential.expose())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(bytes))
            .map_err(|_| alarm_protocol_error())?;
        let response = self
            .client
            .request(request)
            .await
            .map_err(|_| runtime_unavailable())?;
        if !response.status().is_success() {
            return Err(PlatformError::new(
                ErrorCode::DoStorageUnavailable,
                "Durable Object alarm dispatch failed",
            ));
        }
        let bytes = to_bytes(Body::new(response.into_body()), 4096)
            .await
            .map_err(|_| alarm_protocol_error())?;
        serde_json::from_slice(&bytes).map_err(|_| alarm_protocol_error())
    }

    /// Prove that a named module export exists without invoking tenant `fetch()`.
    pub async fn probe_entrypoint(
        &self,
        candidate: ValidationCandidate,
        entrypoint: String,
    ) -> Result<(), PlatformError> {
        self.validate_candidate(candidate, Some(entrypoint)).await
    }

    async fn validate_candidate(
        &self,
        candidate: ValidationCandidate,
        entrypoint: Option<String>,
    ) -> Result<(), PlatformError> {
        let target = DispatchTarget {
            account_id: candidate.account_id,
            worker_id: candidate.worker_id,
            deployment_id: candidate.deployment_id,
            worker_code_sha256: hex::encode(candidate.worker_code_sha256),
            entrypoint,
            route_generation: 0,
            request_id: RequestId::generate(),
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/")
            .body(Body::empty())
            .map_err(|_| runtime_unavailable())?;
        let response = self.send(target, request, true, false).await?;
        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),
            StatusCode::NOT_FOUND => Err(PlatformError::new(
                ErrorCode::EntrypointNotFound,
                "named entrypoint was not found",
            )),
            StatusCode::UNPROCESSABLE_ENTITY => Err(PlatformError::new(
                ErrorCode::BundleRuntimeInvalid,
                "real workerd rejected deployment startup",
            )),
            _ => Err(runtime_unavailable()),
        }
    }

    async fn send(
        &self,
        target: DispatchTarget,
        request: Request,
        validation: bool,
        durable_object_class: bool,
    ) -> Result<Response, PlatformError> {
        let (port, credential) = self.endpoint()?;
        let (parts, body) = request.into_parts();
        let original_method = parts.method.as_str().to_owned();
        let original_url = if validation {
            "https://validation.invalid/".to_owned()
        } else {
            original_url(&parts.headers, &parts.uri)?
        };
        let mut headers = sanitize_tenant_headers(parts.headers);
        insert_header(&mut headers, TOKEN_HEADER, credential.expose())?;
        insert_header(
            &mut headers,
            "x-open-compute-account-id",
            &target.account_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-worker-id",
            &target.worker_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-deployment-id",
            &target.deployment_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-loader-key",
            &target.loader_key(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-worker-code-sha256",
            &target.worker_code_sha256,
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-route-generation",
            &target.route_generation.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-request-id",
            &target.request_id.to_string(),
        )?;
        insert_header(
            &mut headers,
            "x-open-compute-original-method",
            &original_method,
        )?;
        insert_header(&mut headers, "x-open-compute-original-url", &original_url)?;
        if let Some(entrypoint) = &target.entrypoint {
            insert_header(&mut headers, "x-open-compute-entrypoint", entrypoint)?;
        }
        let uri: Uri = format!(
            "http://127.0.0.1:{port}{}",
            if durable_object_class {
                "/internal/validate-do"
            } else if validation {
                "/internal/validate"
            } else {
                "/internal/dispatch"
            }
        )
        .parse()
        .map_err(|_| runtime_unavailable())?;
        let mut internal =
            hyper::Request::new(Body::new(Limited::new(body, self.max_request_body)));
        *internal.method_mut() = Method::POST;
        *internal.uri_mut() = uri;
        *internal.headers_mut() = headers;
        let response = tokio::time::timeout(RESPONSE_HEADER_TIMEOUT, self.client.request(internal))
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::ResourceLimitExceeded,
                    "runtime response header deadline exceeded",
                )
            })?
            .map_err(|_| runtime_unavailable())?;
        let (mut parts, body) = response.into_parts();
        let loader_outcome = parts
            .headers
            .get("x-open-compute-loader-outcome")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| match value {
                "cold" => Some(LoaderOutcome::Cold),
                "warm" => Some(LoaderOutcome::Warm),
                _ => None,
            });
        sanitize_response_headers(&mut parts.headers);
        if let Some(outcome) = loader_outcome {
            parts.extensions.insert(outcome);
        }
        Ok(Response::from_parts(parts, Body::new(body)))
    }

    fn endpoint(&self) -> Result<(u16, open_compute_runtime::GenerationCredential), PlatformError> {
        #[cfg(test)]
        if let Some(port) = self.test_endpoint {
            let credential = self.auth.credential().ok_or_else(runtime_unavailable)?;
            return Ok((port, credential));
        }
        let supervisor = self
            .supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(runtime_unavailable)?;
        let snapshot = supervisor.snapshot();
        if snapshot.state != SupervisorState::Running {
            return Err(runtime_unavailable());
        }
        let port = snapshot.listen_port.ok_or_else(runtime_unavailable)?;
        let credential = self.auth.credential().ok_or_else(runtime_unavailable)?;
        Ok((port, credential))
    }
}

fn validate_alarm_dispatch_result(
    result: AlarmDispatchResult,
) -> Result<AlarmDispatchResult, PlatformError> {
    let schedule_valid = result.scheduled_time_ms.is_some_and(|value| value > 0)
        && result.retry_count.is_some_and(|value| value <= 6);
    let shape_valid = match result.outcome {
        AlarmDispatchOutcome::Success | AlarmDispatchOutcome::Stale => {
            result.scheduled_time_ms.is_none()
                && result.retry_count.is_none()
                && result.error_code.is_none()
        }
        AlarmDispatchOutcome::NotDue => schedule_valid && result.error_code.is_none(),
        AlarmDispatchOutcome::Retry => {
            schedule_valid && result.error_code.as_deref() == Some("DO_RUNTIME_EXCEPTION")
        }
        AlarmDispatchOutcome::Exhausted => {
            result.scheduled_time_ms.is_none()
                && result.retry_count.is_none()
                && result.error_code.as_deref() == Some("DO_RUNTIME_EXCEPTION")
        }
    };
    shape_valid
        .then_some(result)
        .ok_or_else(alarm_protocol_error)
}

fn validate_queue_dispatch_result(
    result: QueueDispatchResult,
    message_count: usize,
) -> Result<QueueDispatchResult, PlatformError> {
    let bounded = message_count > 0
        && message_count <= 100
        && result.explicit_acks.len() <= message_count
        && result.retry_messages.len() <= message_count;
    let outcome = matches!(
        result.outcome.as_str(),
        "ok" | "exception"
            | "canceled"
            | "killSwitch"
            | "daemonDown"
            | "exceededCpu"
            | "exceededMemory"
            | "loadShed"
            | "responseStreamDisconnected"
            | "scriptNotFound"
            | "internalError"
            | "exceededWallTime"
            | "aborted"
            | "unknown"
    );
    let delay = |value: Option<i64>| value.is_none_or(|value| (0..=86_400).contains(&value));
    let decisions = delay(result.retry_batch.delay_seconds)
        && result
            .retry_messages
            .iter()
            .all(|decision| !decision.msg_id.is_empty() && delay(decision.delay_seconds))
        && result
            .explicit_acks
            .iter()
            .all(|id| !id.is_empty() && id.len() <= 128);
    (bounded && outcome && decisions)
        .then_some(result)
        .ok_or_else(queue_protocol_error)
}

fn validate_queue_dispatch_request(request: &QueueDispatchRequest) -> Result<(), PlatformError> {
    if request.queue_name.is_empty()
        || request.queue_name.len() > 128
        || request.queue_name.chars().any(char::is_control)
        || request.messages.is_empty()
        || request.messages.len() > 100
    {
        return Err(queue_protocol_error());
    }
    let mut identities = HashSet::with_capacity(request.messages.len());
    let mut total = 0_usize;
    for message in &request.messages {
        let id: QueueMessageId = message.id.parse().map_err(|_| queue_protocol_error())?;
        if !identities.insert(id)
            || message.timestamp_ms < 0
            || !(1..=101).contains(&message.attempts)
        {
            return Err(queue_protocol_error());
        }
        let body = base64::engine::general_purpose::STANDARD
            .decode(&message.body_base64)
            .map_err(|_| queue_protocol_error())?;
        if u64::try_from(body.len()).map_err(|_| queue_protocol_error())? > QUEUE_MAX_MESSAGE_BYTES
        {
            return Err(queue_protocol_error());
        }
        match message.content_type {
            QueueContentType::Json => {
                serde_json::from_slice::<serde_json::Value>(&body)
                    .map_err(|_| queue_protocol_error())?;
            }
            QueueContentType::Text => {
                std::str::from_utf8(&body).map_err(|_| queue_protocol_error())?;
            }
            QueueContentType::Bytes => {}
        }
        total = total
            .checked_add(message.body_base64.len())
            .ok_or_else(queue_protocol_error)?;
    }
    if total > MAX_QUEUE_CUSTOM_EVENT_REQUEST {
        return Err(queue_protocol_error());
    }
    Ok(())
}

fn validate_scheduled_dispatch_request(
    request: &ScheduledDispatchRequest,
) -> Result<(), PlatformError> {
    if request.scheduled_time_ms < 0 || request.scheduled_time_ms % 60_000 != 0 {
        return Err(PlatformError::new(
            ErrorCode::CronActivationStale,
            "Cron logical slot is invalid",
        ));
    }
    CronSchedule::parse(&request.cron).map(|_| ())
}

fn validate_scheduled_dispatch_result(
    result: ScheduledDispatchResult,
) -> Result<ScheduledDispatchResult, PlatformError> {
    matches!(
        result.outcome.as_str(),
        "ok" | "exception"
            | "canceled"
            | "killSwitch"
            | "daemonDown"
            | "exceededCpu"
            | "exceededMemory"
            | "loadShed"
            | "responseStreamDisconnected"
            | "scriptNotFound"
            | "internalError"
            | "exceededWallTime"
            | "aborted"
            | "unknown"
    )
    .then_some(result)
    .ok_or_else(custom_event_protocol_error)
}

fn validate_alarm_repair_result(
    result: AlarmRepairResult,
) -> Result<AlarmRepairResult, PlatformError> {
    let shape_valid = if result.exists {
        result.scheduled_time_ms.is_some_and(|value| value > 0)
            && result.retry_count.is_some_and(|value| value <= 6)
            && result
                .row_token
                .as_deref()
                .is_some_and(valid_alarm_row_token)
    } else {
        result.scheduled_time_ms.is_none()
            && result.retry_count.is_none()
            && result.row_token.is_none()
    };
    shape_valid
        .then_some(result)
        .ok_or_else(alarm_protocol_error)
}

fn valid_alarm_row_token(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|token| token.get_version() == Some(uuid::Version::Random))
}

impl RuntimeValidator for WorkerdTransport {
    fn validate(
        &self,
        candidate: ValidationCandidate,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move { self.validate_candidate(candidate, None).await })
    }

    fn validate_entrypoint(
        &self,
        candidate: ValidationCandidate,
        entrypoint: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move { self.probe_entrypoint(candidate, entrypoint).await })
    }

    fn validate_durable_object_class(
        &self,
        candidate: ValidationCandidate,
        class_name: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async move {
            let target = DispatchTarget {
                account_id: candidate.account_id,
                worker_id: candidate.worker_id,
                deployment_id: candidate.deployment_id,
                worker_code_sha256: hex::encode(candidate.worker_code_sha256),
                entrypoint: Some(class_name),
                route_generation: 0,
                request_id: RequestId::generate(),
            };
            let request = Request::builder()
                .method(Method::POST)
                .uri("/")
                .body(Body::empty())
                .map_err(|_| runtime_unavailable())?;
            let response = self.send(target, request, true, true).await?;
            match response.status() {
                StatusCode::NO_CONTENT => Ok(()),
                StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
                    Err(PlatformError::new(
                        ErrorCode::DoClassNotFound,
                        "Durable Object class was not found",
                    ))
                }
                _ => Err(runtime_unavailable()),
            }
        })
    }
}

fn original_url(headers: &HeaderMap, uri: &Uri) -> Result<String, PlatformError> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            PlatformError::new(ErrorCode::RouteNotFound, "public request Host is required")
        })?;
    let path = uri.path_and_query().map_or("/", |value| value.as_str());
    let value = format!("http://{host}{path}");
    HeaderValue::from_str(&value).map_err(|_| {
        PlatformError::new(ErrorCode::RouteNotFound, "public request URL is invalid")
    })?;
    Ok(value)
}

fn sanitize_tenant_headers(mut headers: HeaderMap) -> HeaderMap {
    let connection_tokens = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|value| HeaderName::from_bytes(value.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    for name in connection_tokens {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
    ] {
        headers.remove(name);
    }
    let internal = headers
        .keys()
        .filter(|name| name.as_str().starts_with("x-open-compute-"))
        .cloned()
        .collect::<Vec<_>>();
    for name in internal {
        headers.remove(name);
    }
    headers
}

fn sanitize_response_headers(headers: &mut HeaderMap) {
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
    let internal = headers
        .keys()
        .filter(|name| {
            name.as_str().starts_with("x-open-compute-")
                && name.as_str() != "x-open-compute-request-id"
        })
        .cloned()
        .collect::<Vec<_>>();
    for name in internal {
        headers.remove(name);
    }
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), PlatformError> {
    let value = HeaderValue::from_str(value).map_err(|_| runtime_unavailable())?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

fn runtime_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeUnavailable,
        "the workerd runtime is unavailable",
    )
}

fn alarm_protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "private alarm dispatch response is invalid",
    )
}

fn queue_protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueDispositionInvalid,
        "native Queue disposition is invalid",
    )
}

fn custom_event_protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "native custom-event response is invalid",
    )
}

#[cfg(test)]
#[path = "runtime_bridge_tests.rs"]
mod tests;
