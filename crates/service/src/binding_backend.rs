//! Generation-authenticated private backend for typed resource-binding adapters.

use crate::metrics::{BindingBackendOperation, MetricsRegistry};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use open_compute_core::{BindingId, BindingKind, DeploymentId, ErrorCode, PlatformError};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{AuthorizedBinding, BindingRepository, PlatformStorage};
use open_compute_workers::{ResourcePin, ResourcePins};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpListener;

const TOKEN_HEADER: &str = "x-open-compute-binding-token";
const GENERATION_HEADER: &str = "x-open-compute-startup-generation";
const DEPLOYMENT_HEADER: &str = "x-open-compute-deployment-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const REQUEST_HEADER: &str = "x-open-compute-request-id";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const JSON_CONTENT_TYPE: &str = "application/vnd.open-compute.kv.v1+json";
const STREAM_CONTENT_TYPE: &str = "application/vnd.open-compute.kv.v1+octet-stream";
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_KEY_BYTES: usize = 1024;
const BACKEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Static, platform-owned executor for the P0 KV adapter protocol.
pub trait KvBindingExecutor: Send + Sync + 'static {
    /// Read one value from an already-authorized physical resource.
    fn get(&self, binding: &AuthorizedBinding, key: &str) -> Result<Option<String>, PlatformError>;

    /// Write one value to an already-authorized physical resource.
    fn put(&self, binding: &AuthorizedBinding, key: &str, value: &str)
    -> Result<(), PlatformError>;

    /// Delete one value from an already-authorized physical resource.
    fn delete(&self, binding: &AuthorizedBinding, key: &str) -> Result<(), PlatformError>;
}

/// Fail-closed executor used until a concrete product backend is composed.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableKvBindingExecutor;

impl KvBindingExecutor for UnavailableKvBindingExecutor {
    fn get(
        &self,
        _binding: &AuthorizedBinding,
        _key: &str,
    ) -> Result<Option<String>, PlatformError> {
        Err(unavailable())
    }

    fn put(
        &self,
        _binding: &AuthorizedBinding,
        _key: &str,
        _value: &str,
    ) -> Result<(), PlatformError> {
        Err(unavailable())
    }

    fn delete(&self, _binding: &AuthorizedBinding, _key: &str) -> Result<(), PlatformError> {
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
}

/// Bind the private binding backend to an ephemeral IPv4 loopback port.
pub async fn bind_binding_backend() -> Result<TcpListener, PlatformError> {
    TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "failed to bind private binding backend listener",
            )
        })
}

/// Serve the binding backend without public HTTP middleware or tenant-visible diagnostics.
pub async fn serve_binding_backend(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    serve_binding_backend_with_metrics(listener, storage, auth, pins, executor, None, shutdown)
        .await
}

/// Serve the binding backend while recording its fixed, low-cardinality metric set.
pub async fn serve_binding_backend_with_metrics(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let state = BackendState {
        storage,
        auth,
        pins,
        executor,
        metrics,
    };
    let router = Router::new().fallback(handle).with_state(state);
    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "private binding backend listener failed",
            )
        })
}

#[derive(Clone, Copy)]
enum Operation {
    Get,
    Put,
    Delete,
    Echo,
}

impl Operation {
    const fn metric(self) -> BindingBackendOperation {
        match self {
            Self::Get => BindingBackendOperation::Get,
            Self::Put => BindingBackendOperation::Put,
            Self::Delete => BindingBackendOperation::Delete,
            Self::Echo => BindingBackendOperation::Echo,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRequest {
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutRequest {
    key: String,
    value: String,
}

enum ParsedOperation {
    Get(String),
    Put { key: String, value: String },
    Delete(String),
}

#[derive(Serialize)]
struct GetResponse {
    value: Option<String>,
}

async fn handle(State(state): State<BackendState>, request: Request) -> Response {
    let headers = request.headers();
    let token = header_text(headers, TOKEN_HEADER).unwrap_or("");
    let generation = header_text(headers, GENERATION_HEADER).unwrap_or("");
    if !state.auth.authorize(token, generation) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if request.method() != Method::POST {
        return backend_error(
            ErrorCode::BindingProtocolError,
            StatusCode::METHOD_NOT_ALLOWED,
        );
    }
    if declared_too_large(headers) {
        return backend_error(
            ErrorCode::BindingLimitExceeded,
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    }
    let Some((binding_id, operation)) = parse_path(request.uri().path()) else {
        if let Some(metrics) = &state.metrics {
            metrics.inc_binding_protocol_error();
        }
        return backend_error(ErrorCode::BindingProtocolError, StatusCode::NOT_FOUND);
    };
    let ingress_bytes = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let observe = |response: Response| {
        if let Some(metrics) = &state.metrics {
            let egress_bytes = response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            if response
                .headers()
                .get(ERROR_HEADER)
                .is_some_and(|value| value == ErrorCode::BindingProtocolError.as_str())
            {
                metrics.inc_binding_protocol_error();
            }
            metrics.observe_binding_backend(
                operation.metric(),
                response.status().is_success(),
                ingress_bytes,
                egress_bytes,
            );
        }
        response
    };
    let deployment_id = match parse_header::<DeploymentId>(headers, DEPLOYMENT_HEADER) {
        Ok(value) => value,
        Err(error) => return observe(platform_error(&error)),
    };
    if !valid_request_id(headers) {
        return observe(backend_error(
            ErrorCode::BindingProtocolError,
            StatusCode::BAD_REQUEST,
        ));
    }
    let descriptor_sha256 = match parse_digest(headers) {
        Ok(value) => value,
        Err(error) => return observe(platform_error(&error)),
    };
    if !content_type_matches(headers, operation) {
        return observe(backend_error(
            ErrorCode::BindingProtocolError,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ));
    }
    let storage = state.storage.clone();
    let binding = match tokio::time::timeout(
        BACKEND_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            BindingRepository::new(storage.db()).authorize(
                binding_id,
                deployment_id,
                &descriptor_sha256,
            )
        }),
    )
    .await
    {
        Ok(Ok(Ok(binding))) => binding,
        Ok(Ok(Err(error))) => return observe(platform_error(&error)),
        Ok(Err(_)) => {
            return observe(backend_error(
                ErrorCode::BindingProtocolError,
                StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
        Err(_) => {
            return observe(backend_error(
                ErrorCode::ResourceUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }
    };
    if binding.binding.kind != BindingKind::KvNamespace || binding.binding.capability_version != 1 {
        return observe(backend_error(
            ErrorCode::BindingCapabilityUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    if !permission_allows(&binding, operation) {
        return observe(backend_error(
            ErrorCode::BindingPermissionDenied,
            StatusCode::FORBIDDEN,
        ));
    }
    let pin = match state.pins.try_pin(binding.resource.id) {
        Ok(pin) => pin,
        Err(error) => return observe(platform_error(&error)),
    };
    observe(dispatch(state.executor, binding, operation, request, pin).await)
}

async fn dispatch(
    executor: Arc<dyn KvBindingExecutor>,
    binding: AuthorizedBinding,
    operation: Operation,
    request: Request,
    pin: ResourcePin,
) -> Response {
    if matches!(operation, Operation::Echo) {
        let stream = PinnedLimitedStream::new(request.into_body(), pin, MAX_BODY_BYTES);
        let mut response = Response::new(Body::from_stream(stream));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(STREAM_CONTENT_TYPE),
        );
        return response;
    }
    let Ok(bytes) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return backend_error(
            ErrorCode::BindingLimitExceeded,
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    };
    let parsed = match operation {
        Operation::Get => parse_json::<KeyRequest>(&bytes).and_then(|body| {
            validate_key(&body.key)?;
            Ok(ParsedOperation::Get(body.key))
        }),
        Operation::Put => parse_json::<PutRequest>(&bytes).and_then(|body| {
            validate_key(&body.key)?;
            Ok(ParsedOperation::Put {
                key: body.key,
                value: body.value,
            })
        }),
        Operation::Delete => parse_json::<KeyRequest>(&bytes).and_then(|body| {
            validate_key(&body.key)?;
            Ok(ParsedOperation::Delete(body.key))
        }),
        Operation::Echo => unreachable!("echo returns before buffering"),
    };
    let result = match parsed {
        Ok(parsed) => {
            let executor = executor.clone();
            let blocking = tokio::task::spawn_blocking(move || match parsed {
                ParsedOperation::Get(key) => {
                    let _pin = pin;
                    executor
                        .get(&binding, &key)
                        .and_then(serialize_get_response)
                }
                ParsedOperation::Put { key, value } => {
                    let _pin = pin;
                    executor.put(&binding, &key, &value).map(|()| Vec::new())
                }
                ParsedOperation::Delete(key) => {
                    let _pin = pin;
                    executor.delete(&binding, &key).map(|()| Vec::new())
                }
            });
            match tokio::time::timeout(BACKEND_TIMEOUT, blocking).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(protocol_error()),
                Err(_) if matches!(operation, Operation::Put | Operation::Delete) => {
                    Err(PlatformError::new(
                        ErrorCode::BindingResultUnknown,
                        "binding mutation result is unknown",
                    ))
                }
                Err(_) => Err(unavailable()),
            }
        }
        Err(error) => {
            drop(pin);
            Err(error)
        }
    };
    match result {
        Ok(bytes) if matches!(operation, Operation::Get) => {
            let length = bytes.len();
            let mut response = Response::new(Body::from(bytes));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
                response.headers_mut().insert(header::CONTENT_LENGTH, value);
            }
            response
        }
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => platform_error(&error),
    }
}

fn parse_path(path: &str) -> Option<(BindingId, Operation)> {
    let rest = path.strip_prefix("/internal/bindings/v1/kv/")?;
    let (id, operation) = rest.split_once('/')?;
    if operation.contains('/') {
        return None;
    }
    let operation = match operation {
        "get" => Operation::Get,
        "put" => Operation::Put,
        "delete" => Operation::Delete,
        "echo" => Operation::Echo,
        _ => return None,
    };
    Some((BindingId::from_str(id).ok()?, operation))
}

fn declared_too_large(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_BODY_BYTES)
}

fn content_type_matches(headers: &HeaderMap, operation: Operation) -> bool {
    let expected = if matches!(operation, Operation::Echo) {
        STREAM_CONTENT_TYPE
    } else {
        JSON_CONTENT_TYPE
    };
    header_text(headers, header::CONTENT_TYPE.as_str())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

fn permission_allows(binding: &AuthorizedBinding, operation: Operation) -> bool {
    match operation {
        Operation::Get | Operation::Echo => binding.binding.permissions.read,
        Operation::Put | Operation::Delete => binding.binding.permissions.write,
    }
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

fn validate_key(key: &str) -> Result<(), PlatformError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(PlatformError::new(
            ErrorCode::BindingLimitExceeded,
            "binding key exceeds its fixed budget",
        ));
    }
    Ok(())
}

fn serialize_get_response(value: Option<String>) -> Result<Vec<u8>, PlatformError> {
    let bytes = serde_json::to_vec(&GetResponse { value }).map_err(|_| protocol_error())?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(PlatformError::new(
            ErrorCode::BindingLimitExceeded,
            "binding result exceeds its fixed budget",
        ));
    }
    Ok(bytes)
}

fn platform_error(error: &PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::BindingNotFound | ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::BindingPermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::BindingLimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::ResourceNotReady | ErrorCode::ResourceReferenced => StatusCode::CONFLICT,
        ErrorCode::ResourceUnavailable | ErrorCode::BindingResultUnknown => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ErrorCode::BindingTypeMismatch
        | ErrorCode::BindingCapabilityUnsupported
        | ErrorCode::ResourceInvariantViolation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::BindingProtocolError => StatusCode::BAD_REQUEST,
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
            | ErrorCode::BindingResultUnknown
    );
    let body = serde_json::json!({
        "ok": false,
        "error": {
            "code": code.as_str(),
            "retryable": retryable,
            "resultUnknown": code == ErrorCode::BindingResultUnknown,
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

struct PinnedLimitedStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
    _pin: ResourcePin,
    seen: usize,
    limit: usize,
    done: bool,
}

impl PinnedLimitedStream {
    fn new(body: Body, pin: ResourcePin, limit: usize) -> Self {
        let stream = TryStreamExt::map_err(body.into_data_stream(), std::io::Error::other);
        Self {
            inner: Box::pin(stream),
            _pin: pin,
            seen: 0,
            limit,
            done: false,
        }
    }
}

impl Stream for PinnedLimitedStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                self.seen = self.seen.saturating_add(bytes.len());
                if self.seen > self.limit {
                    self.done = true;
                    Poll::Ready(Some(Err(std::io::Error::other(
                        "binding stream exceeded its fixed budget",
                    ))))
                } else {
                    Poll::Ready(Some(Ok(bytes)))
                }
            }
            Poll::Ready(Some(Err(error))) => {
                self.done = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
#[path = "binding_backend_tests.rs"]
mod tests;
