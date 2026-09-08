//! Private `RuntimeSource` listener and streaming ocd-to-workerd transport.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine as _;
use http_body_util::Limited;
use hyper::body::Body as _;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_core::{
    AccountId, CronSchedule, ErrorCode, PlatformError, QueueMessageId, RequestId, VersionId,
    WorkerId,
};
use open_compute_runtime::{
    GenerationAuthRegistry, SupervisorState, TOKEN_HEADER, WorkerdSupervisor,
};
use open_compute_storage::{
    AuthorizedDurableObjectDelete, ClaimedJob, QUEUE_MAX_MESSAGE_BYTES, QueueContentType,
};
use open_compute_workers::{
    RuntimeScope, RuntimeSource, RuntimeValidator, ValidationCandidate, VersionPins, loader_key,
    validate_env_name,
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

#[path = "runtime_bridge/workflow.rs"]
mod workflow;
pub use workflow::{WorkflowDispatchResult, WorkflowOutcome, WorkflowRunRequest};
mod custom_events;
mod dispatch;
mod websocket;
#[path = "runtime_bridge/worker_loaders.rs"]
mod worker_loaders;

const SOURCE_PATH: &str = "/internal/runtime/v1/versions/resolve";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const SERVICE_WEBSOCKET_HANDOFF_HEADER: &str = "x-open-compute-service-websocket-handoffs";
const MAX_SOURCE_REQUEST: usize = 4096;
/// Fixed Standard ingress baseline in decimal bytes, independent of operator policy.
pub const MAX_TENANT_BODY_BYTES: usize = 100_000_000;
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

/// Live backlog metadata delivered with a native Queue custom event.
#[derive(Clone, Debug, Default, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDispatchMetrics {
    /// Retained message count, including delayed messages.
    pub backlog_count: u64,
    /// Retained serialized body bytes.
    pub backlog_bytes: u64,
    /// Oldest enqueue timestamp; omitted when empty or the epoch sentinel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_message_timestamp_ms: Option<i64>,
}

/// Native `MessageBatch.metadata` payload assembled after a durable claim.
#[derive(Clone, Debug, Default, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDispatchMetadata {
    /// Live backlog snapshot observed at dispatch.
    pub metrics: QueueDispatchMetrics,
}

impl QueueDispatchMetadata {
    /// Copy scheduler metrics, converting the epoch sentinel to absence.
    #[must_use]
    pub fn from_queue_metrics(metrics: open_compute_storage::QueueMetrics) -> Self {
        Self {
            metrics: QueueDispatchMetrics {
                backlog_count: metrics.backlog_count,
                backlog_bytes: metrics.backlog_bytes,
                oldest_message_timestamp_ms: metrics
                    .oldest_message_timestamp_ms
                    .filter(|value| *value != 0),
            },
        }
    }
}

/// Trusted native Queue custom-event request assembled after a durable claim.
#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDispatchRequest {
    /// Tenant-visible Queue name.
    pub queue_name: String,
    /// Bounded claimed membership in deterministic order.
    pub messages: Vec<QueueDispatchMessage>,
    /// Live backlog metadata for `MessageBatch.metadata`.
    #[serde(default)]
    pub metadata: QueueDispatchMetadata,
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
    /// Exact version-declared expression.
    pub cron: String,
    /// Whether the tenant Worker's scheduled handler owns this expression.
    pub scheduled_handler: bool,
    /// Workflow bindings directly triggered by this expression.
    pub workflow_bindings: Vec<String>,
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

/// Immutable target frozen by route resolution or version validation.
#[derive(Clone, Debug)]
pub struct DispatchTarget {
    /// Account authority.
    pub account_id: AccountId,
    /// Worker authority.
    pub worker_id: WorkerId,
    /// Version authority.
    pub version_id: VersionId,
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
        loader_key(self.account_id, self.worker_id, self.version_id)
    }
}

mod source_server;
mod transport;

#[cfg(test)]
use source_server::source_platform_error;
pub use source_server::*;
pub use transport::WorkerdTransport;
#[cfg(test)]
use transport::{validate_alarm_dispatch_result, validate_alarm_repair_result};
use transport::{
    validate_queue_dispatch_request, validate_queue_dispatch_result,
    validate_scheduled_dispatch_request, validate_scheduled_dispatch_result,
};

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
