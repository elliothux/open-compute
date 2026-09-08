//! Generation-authenticated private backend for typed resource-binding adapters.

use crate::d1_backend::D1BindingService;
use crate::kv_backend::{KvCommand, KvCommandResult, KvStreamPart};
use crate::metrics::{AlarmMutation, DoOperation, MetricsRegistry, ServiceMetricOperation};
use crate::queue_backend::QueueBindingService;
use crate::r2_backend::R2BindingService;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{
    BindingId, BindingKind, DurableObjectId, DurableObjectsConfig, ErrorCode, OperationClass,
    PlatformError, QueuesConfig, ResourceId, VersionId,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{
    AlarmProjection, AuthorizedBinding, BindingRepository, DurableObjectRepository,
    PlatformStorage, SchedulerStore,
};
use open_compute_workers::ResourcePins;
use serde::Deserialize;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

mod kv;
mod search_composition;
use kv::{
    FRAME_CONTENT_TYPE, StreamBudget, declared_too_large, dispatch, parse_path, permission_allows,
};
#[cfg(any(test, feature = "test-support"))]
pub use search_composition::serve_binding_backend_with_ai_search;
pub(crate) use search_composition::serve_binding_backend_with_ai_search_and_snapshot_pins;
pub use search_composition::serve_binding_backend_with_document_parser;

const TOKEN_HEADER: &str = "x-open-compute-binding-token";
const GENERATION_HEADER: &str = "x-open-compute-startup-generation";
const VERSION_HEADER: &str = "x-open-compute-version-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const REQUEST_HEADER: &str = "x-open-compute-request-id";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const BACKEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Static, platform-owned executor for the P0 KV adapter protocol.
pub trait KvBindingExecutor: Send + Sync + 'static {
    /// Maximum foreground duration before the private transport stops waiting.
    fn operation_timeout(&self) -> Duration {
        BACKEND_TIMEOUT
    }

    /// Global and per-namespace active body-stream limits.
    fn stream_limits(&self) -> (u32, u32) {
        (16, 4)
    }

    /// Execute one structured command against an already-authorized resource.
    fn execute(
        &self,
        binding: &AuthorizedBinding,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError>;

    /// Stream one value in bounded chunks without materializing the entire value.
    fn stream_get(
        &self,
        binding: &AuthorizedBinding,
        key: &str,
        cache_ttl: Option<u64>,
        sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError>;
}

/// Fail-closed executor used when no KV backend has been composed.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableKvBindingExecutor;

#[cfg(any(test, feature = "test-support"))]
impl KvBindingExecutor for UnavailableKvBindingExecutor {
    fn execute(
        &self,
        _binding: &AuthorizedBinding,
        _command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        Err(unavailable())
    }

    fn stream_get(
        &self,
        _binding: &AuthorizedBinding,
        _key: &str,
        _cache_ttl: Option<u64>,
        _sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        Err(unavailable())
    }
}

#[derive(Clone)]
struct BackendState {
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    stream_budget: StreamBudget,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    queue: Option<Arc<QueueBindingService>>,
    workflow: Option<Arc<crate::workflow_backend::WorkflowBindingService>>,
    assets: Option<Arc<crate::asset_backend::AssetBindingService>>,
    services: Option<Arc<crate::service_invocations::ServiceInvocationRegistry>>,
    cache: Option<Arc<crate::cache_backend::CacheBindingService>>,
    images: Option<Arc<crate::images_backend::ImageBindingService>>,
    document_parser: Option<Arc<crate::document_parser_backend::DocumentParserBindingService>>,
    ai_search: Option<Arc<crate::ai_search_backend::AiSearchBindingService>>,
}

mod handlers;
mod server;

use handlers::handle;
#[cfg(test)]
use handlers::{DoResolveRequest, handle_service_invocation};
pub use server::*;

fn parse_do_resolve_path(path: &str) -> Option<BindingId> {
    parse_do_path(path, "resolve")
}

fn parse_do_path(path: &str, operation: &str) -> Option<BindingId> {
    let rest = path.strip_prefix("/internal/bindings/v1/do/")?;
    let id = rest.strip_suffix(&format!("/{operation}"))?;
    (!id.contains('/'))
        .then(|| BindingId::from_str(id).ok())
        .flatten()
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    header_text(headers, header::CONTENT_TYPE.as_str())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn parse_header<T: FromStr>(headers: &HeaderMap, name: &str) -> Result<T, PlatformError> {
    header_text(headers, name)
        .and_then(|value| T::from_str(value).ok())
        .ok_or_else(protocol_error)
}

fn parse_digest(headers: &HeaderMap) -> Result<[u8; 32], PlatformError> {
    let value = header_text(headers, DESCRIPTOR_HEADER).ok_or_else(protocol_error)?;
    let bytes = hex::decode(value).map_err(|_| protocol_error())?;
    bytes.try_into().map_err(|_| protocol_error())
}

fn valid_request_id(headers: &HeaderMap) -> bool {
    let Some(value) = header_text(headers, REQUEST_HEADER) else {
        return false;
    };
    let Ok(parsed) = uuid::Uuid::parse_str(value) else {
        return false;
    };
    parsed.hyphenated().to_string() == value
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, PlatformError> {
    serde_json::from_slice(bytes).map_err(|_| protocol_error())
}

fn platform_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BindingNotFound | ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::ServiceEntrypointNotFound => StatusCode::NOT_FOUND,
        ErrorCode::ServiceBindingDenied => StatusCode::FORBIDDEN,
        ErrorCode::DoNamespaceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::BindingLimitExceeded
        | ErrorCode::KvKeyTooLarge
        | ErrorCode::KvValueTooLarge
        | ErrorCode::KvMetadataTooLarge
        | ErrorCode::KvResponseTooLarge
        | ErrorCode::KvTooManyKeys => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::ResourceNotReady
        | ErrorCode::ResourceReferenced
        | ErrorCode::DoObjectDeleting
        | ErrorCode::DoVersionStale
        | ErrorCode::DoNamespaceNotEmpty => StatusCode::CONFLICT,
        ErrorCode::ServiceTargetNotReady => StatusCode::CONFLICT,
        ErrorCode::ResourceUnavailable
        | ErrorCode::KvBusy
        | ErrorCode::KvStorageFull
        | ErrorCode::KvUnavailable
        | ErrorCode::KvResultUnknown => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ServiceUnavailable | ErrorCode::ServiceTimeout => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::ServiceLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::DoStorageUnavailable
        | ErrorCode::DoStorageLimit
        | ErrorCode::DoDispatchTimeout
        | ErrorCode::DoAlarmIndexUnavailable
        | ErrorCode::SchedulerUnavailable
        | ErrorCode::SchedulerBusy => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BindingTypeMismatch
        | ErrorCode::BindingCapabilityUnsupported
        | ErrorCode::ResourceInvariantViolation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::BindingProtocolError
        | ErrorCode::KvKeyInvalid
        | ErrorCode::KvMetadataInvalid
        | ErrorCode::KvInvalidOptions
        | ErrorCode::KvCursorInvalid
        | ErrorCode::KvInternalProtocolError => StatusCode::BAD_REQUEST,
        ErrorCode::DoIdInvalid
        | ErrorCode::DoRpcUnsupported
        | ErrorCode::DoInternalProtocolError
        | ErrorCode::SchedulerInternalProtocolError => StatusCode::BAD_REQUEST,
        ErrorCode::DoClassNotFound | ErrorCode::DoRuntimeException => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ErrorCode::KvCorrupt | ErrorCode::SchedulerCorrupt => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    backend_error(error.code(), status)
}

fn backend_error(code: ErrorCode, status: StatusCode) -> Response {
    let retryable = matches!(
        code,
        ErrorCode::ResourceNotReady
            | ErrorCode::ResourceUnavailable
            | ErrorCode::BindingProtocolError
    );
    let body = serde_json::json!({
        "ok": false,
        "error": {
            "code": code.as_str(),
            "retryable": retryable,
            "resultUnknown": code == ErrorCode::KvResultUnknown,
        }
    });
    let mut response = (status, axum::Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(code.as_str()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(ERROR_HEADER), value);
    }
    response
}

#[cfg(any(test, feature = "test-support"))]
fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "resource backend is unavailable",
    )
}

fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingProtocolError,
        "binding request payload is invalid",
    )
}

fn alarm_protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "alarm projection request is invalid",
    )
}

fn alarm_unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::DoAlarmIndexUnavailable,
        "alarm projection authority is unavailable",
    )
}

#[cfg(test)]
#[path = "binding_backend_tests.rs"]
mod tests;
