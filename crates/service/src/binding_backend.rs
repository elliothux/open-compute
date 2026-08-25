//! Generation-authenticated private backend for typed resource-binding adapters.

use crate::d1_backend::D1BindingService;
use crate::kv_backend::{
    KvCommand, KvCommandResult, KvStagedValue, KvStagingLease, KvStreamPart,
    ensure_storage_headroom,
};
use crate::metrics::{BindingBackendOperation, KvOperation, KvStagingGauge, MetricsRegistry};
use crate::r2_backend::R2BindingService;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{Stream, StreamExt as _, TryStreamExt};
use open_compute_core::{
    BindingId, BindingKind, DeploymentId, ErrorCode, PlatformError, ResourceId,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{AuthorizedBinding, BindingRepository, PlatformStorage};
use open_compute_workers::{ResourcePin, ResourcePins};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::{Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpListener;

const TOKEN_HEADER: &str = "x-open-compute-binding-token";
const GENERATION_HEADER: &str = "x-open-compute-startup-generation";
const DEPLOYMENT_HEADER: &str = "x-open-compute-deployment-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const REQUEST_HEADER: &str = "x-open-compute-request-id";
const ERROR_HEADER: &str = "x-open-compute-error-code";
const JSON_CONTENT_TYPE: &str = "application/vnd.open-compute.kv.v1+json";
const STREAM_CONTENT_TYPE: &str = "application/vnd.open-compute.kv.v1+octet-stream";
const FRAME_CONTENT_TYPE: &str = "application/vnd.open-compute.kv.v1+frame";
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_FRAME_BODY_BYTES: usize = open_compute_storage::KV_MAX_VALUE_BYTES + 64 * 1024;
const MAX_KEY_BYTES: usize = open_compute_storage::KV_MAX_KEY_BYTES;
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

    /// Read one value from an already-authorized physical resource.
    fn get(&self, binding: &AuthorizedBinding, key: &str) -> Result<Option<String>, PlatformError>;

    /// Write one value to an already-authorized physical resource.
    fn put(&self, binding: &AuthorizedBinding, key: &str, value: &str)
    -> Result<(), PlatformError>;

    /// Delete one value from an already-authorized physical resource.
    fn delete(&self, binding: &AuthorizedBinding, key: &str) -> Result<(), PlatformError>;

    /// Execute the complete P0.4 command surface.
    fn execute(
        &self,
        binding: &AuthorizedBinding,
        command: KvCommand,
    ) -> Result<KvCommandResult, PlatformError> {
        match command {
            KvCommand::Get { keys, .. } if keys.len() == 1 => {
                self.get(binding, &keys[0]).and_then(|value| {
                    if value
                        .as_ref()
                        .is_some_and(|value| value.len() > MAX_BODY_BYTES)
                    {
                        return Err(PlatformError::new(
                            ErrorCode::BindingLimitExceeded,
                            "legacy binding result exceeds its fixed budget",
                        ));
                    }
                    Ok(KvCommandResult::Entries(vec![value.map(|value| {
                        open_compute_storage::KvEntry {
                            value: value.into_bytes(),
                            metadata_json: None,
                            expires_at_ms: None,
                        }
                    })]))
                })
            }
            KvCommand::Put { key, value, .. } => {
                let value = std::str::from_utf8(&value).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::BindingCapabilityUnsupported,
                        "legacy binding executor accepts text values only",
                    )
                })?;
                self.put(binding, &key, value)
                    .map(|()| KvCommandResult::Mutation)
            }
            KvCommand::PutStaged { key, mut value, .. } => {
                let bytes = value.read_all()?;
                let value = std::str::from_utf8(&bytes).map_err(|_| {
                    PlatformError::new(
                        ErrorCode::BindingCapabilityUnsupported,
                        "legacy binding executor accepts text values only",
                    )
                })?;
                self.put(binding, &key, value)
                    .map(|()| KvCommandResult::Mutation)
            }
            KvCommand::Delete { key } => self
                .delete(binding, &key)
                .map(|()| KvCommandResult::Mutation),
            KvCommand::Get { .. } | KvCommand::List { .. } => Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "binding executor does not implement the requested KV operation",
            )),
        }
    }

    /// Stream one value in bounded chunks. Concrete storage backends should
    /// override this so the value is never materialized as one allocation.
    fn stream_get(
        &self,
        binding: &AuthorizedBinding,
        key: &str,
        cache_ttl: Option<u64>,
        sink: &mut dyn FnMut(KvStreamPart) -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        let result = self.execute(
            binding,
            KvCommand::Get {
                keys: vec![key.to_owned()],
                cache_ttl,
            },
        )?;
        let KvCommandResult::Entries(mut entries) = result else {
            return Err(protocol_error());
        };
        if entries.len() != 1 {
            return Err(protocol_error());
        }
        let entry = entries.pop().flatten();
        let info = entry
            .as_ref()
            .map(|entry| open_compute_storage::KvEntryInfo {
                value_length: entry.value.len(),
                metadata_json: entry.metadata_json.clone(),
                expires_at_ms: entry.expires_at_ms,
            });
        sink(KvStreamPart::Entry(info))?;
        if let Some(entry) = entry {
            for chunk in entry.value.chunks(64 * 1024) {
                sink(KvStreamPart::Bytes(chunk.to_vec()))?;
            }
        }
        Ok(())
    }
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
    stream_budget: StreamBudget,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
}

#[derive(Clone)]
struct StreamBudget {
    global: Arc<tokio::sync::Semaphore>,
    per_resource: usize,
    resources: Arc<Mutex<std::collections::HashMap<ResourceId, Weak<tokio::sync::Semaphore>>>>,
}

impl StreamBudget {
    fn new(global: u32, per_resource: u32) -> Self {
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(global.max(1) as usize)),
            per_resource: per_resource.max(1) as usize,
            resources: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<KvStagingLease, PlatformError> {
        let global = tokio::time::timeout(timeout, self.global.clone().acquire_owned())
            .await
            .map_err(|_| kv_busy())?
            .map_err(|_| kv_busy())?;
        let resource_gate = {
            let mut resources = self
                .resources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            resources.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = resources.get(&resource).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Semaphore::new(self.per_resource));
                resources.insert(resource, Arc::downgrade(&gate));
                gate
            }
        };
        let resource = tokio::time::timeout(timeout, resource_gate.acquire_owned())
            .await
            .map_err(|_| kv_busy())?
            .map_err(|_| kv_busy())?;
        Ok(KvStagingLease::new(global, resource))
    }
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
    serve_binding_backend_with_r2(
        listener, storage, auth, pins, executor, metrics, None, shutdown,
    )
    .await
}

/// Serve the private binding backend with the optional P0.5 R2 data plane.
#[allow(clippy::too_many_arguments)]
pub async fn serve_binding_backend_with_r2(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    serve_binding_backend_with_products(
        listener, storage, auth, pins, executor, metrics, r2, None, shutdown,
    )
    .await
}

/// Serve the private binding backend with all composed product data planes.
#[allow(clippy::too_many_arguments)]
pub async fn serve_binding_backend_with_products(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let (global_streams, resource_streams) = executor.stream_limits();
    let state = BackendState {
        storage,
        auth,
        pins,
        executor,
        metrics,
        stream_budget: StreamBudget::new(global_streams, resource_streams),
        r2,
        d1,
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
    GetWithMetadata,
    GetMany,
    Put,
    Delete,
    List,
    Echo,
}

impl Operation {
    const fn metric(self) -> BindingBackendOperation {
        match self {
            Self::Get | Self::GetWithMetadata | Self::GetMany | Self::List => {
                BindingBackendOperation::Get
            }
            Self::Put => BindingBackendOperation::Put,
            Self::Delete => BindingBackendOperation::Delete,
            Self::Echo => BindingBackendOperation::Echo,
        }
    }

    const fn kv_metric(self) -> KvOperation {
        match self {
            Self::Get => KvOperation::Get,
            Self::GetWithMetadata => KvOperation::GetWithMetadata,
            Self::GetMany => KvOperation::GetMany,
            Self::Put => KvOperation::Put,
            Self::Delete => KvOperation::Delete,
            Self::List => KvOperation::List,
            Self::Echo => KvOperation::Get,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameGetRequest {
    keys: Vec<String>,
    cache_ttl: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FramePutHeader {
    key: String,
    expiration: Option<u64>,
    expiration_ttl: Option<u64>,
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    metadata_present: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrameListRequest {
    #[serde(default)]
    prefix: String,
    limit: u16,
    cursor: Option<String>,
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
    if request
        .uri()
        .path()
        .starts_with("/internal/bindings/v1/r2/")
    {
        return match &state.r2 {
            Some(r2) => r2.handle(request).await,
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if request
        .uri()
        .path()
        .starts_with("/internal/bindings/v1/d1/")
    {
        return match &state.d1 {
            Some(d1) => d1.handle(request).await,
            None => StatusCode::NOT_FOUND.into_response(),
        };
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
    let started = Instant::now();
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
            if !matches!(operation, Operation::Echo) {
                metrics.observe_kv_operation(
                    operation.kv_metric(),
                    response.status().is_success(),
                    ingress_bytes,
                    egress_bytes,
                    started.elapsed(),
                );
            }
            if response
                .headers()
                .get(ERROR_HEADER)
                .is_some_and(|value| value == ErrorCode::KvCorrupt.as_str())
            {
                metrics.inc_kv_corruption(2);
            }
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
    let request_id = header_text(headers, REQUEST_HEADER)
        .unwrap_or("")
        .to_owned();
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
    if content_type_is(request.headers(), FRAME_CONTENT_TYPE) {
        observe(dispatch_frame(state.clone(), binding, operation, request_id, request, pin).await)
    } else {
        observe(dispatch(state.executor, binding, operation, request, pin).await)
    }
}

async fn dispatch_frame(
    state: BackendState,
    binding: AuthorizedBinding,
    operation: Operation,
    request_id: String,
    request: Request,
    pin: ResourcePin,
) -> Response {
    if matches!(operation, Operation::Echo) {
        drop(pin);
        return backend_error(ErrorCode::KvInternalProtocolError, StatusCode::BAD_REQUEST);
    }
    let command = if matches!(operation, Operation::Put) {
        match stage_put_frame(
            &state.storage,
            &binding,
            &request_id,
            request.into_body(),
            &state.stream_budget,
            state.executor.operation_timeout(),
            state.metrics.as_ref(),
        )
        .await
        {
            Ok(command) => command,
            Err(error) => {
                drop(pin);
                return platform_error(&error);
            }
        }
    } else {
        let Ok(bytes) = to_bytes(request.into_body(), MAX_FRAME_BODY_BYTES).await else {
            drop(pin);
            return backend_error(ErrorCode::KvValueTooLarge, StatusCode::PAYLOAD_TOO_LARGE);
        };
        match parse_frame_command(operation, &bytes) {
            Ok(command) => command,
            Err(error) => {
                drop(pin);
                return platform_error(&error);
            }
        }
    };
    if matches!(operation, Operation::Get | Operation::GetWithMetadata) {
        let KvCommand::Get {
            mut keys,
            cache_ttl,
        } = command
        else {
            drop(pin);
            return backend_error(ErrorCode::KvInternalProtocolError, StatusCode::BAD_REQUEST);
        };
        let Some(key) = keys.pop() else {
            drop(pin);
            return backend_error(ErrorCode::KvTooManyKeys, StatusCode::BAD_REQUEST);
        };
        return dispatch_stream_get(state.executor, binding, key, cache_ttl, pin).await;
    }
    let executor = state.executor.clone();
    let timeout = executor.operation_timeout();
    let blocking = tokio::task::spawn_blocking(move || {
        let _pin = pin;
        executor.execute(&binding, command)
    });
    let result = match tokio::time::timeout(timeout, blocking).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(protocol_error()),
        Err(_) if matches!(operation, Operation::Put | Operation::Delete) => Err(
            PlatformError::new(ErrorCode::KvResultUnknown, "KV mutation result is unknown"),
        ),
        Err(_) => Err(PlatformError::new(
            ErrorCode::KvUnavailable,
            "KV namespace operation timed out",
        )),
    };
    match result.and_then(|result| encode_frame_result(operation, result)) {
        Ok((content_type, bytes)) => {
            let length = bytes.len();
            let mut response = Response::new(Body::from(bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
                response.headers_mut().insert(header::CONTENT_LENGTH, value);
            }
            response
        }
        Err(error) => platform_error(&error),
    }
}

enum StreamMessage {
    Part(KvStreamPart),
    Complete,
    Error(PlatformError),
}

async fn dispatch_stream_get(
    executor: Arc<dyn KvBindingExecutor>,
    binding: AuthorizedBinding,
    key: String,
    cache_ttl: Option<u64>,
    pin: ResourcePin,
) -> Response {
    let timeout = executor.operation_timeout();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        let mut sink = |part| {
            sender
                .blocking_send(StreamMessage::Part(part))
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::BindingProtocolError,
                        "KV response consumer cancelled the value stream",
                    )
                })
        };
        let terminal = match executor.stream_get(&binding, &key, cache_ttl, &mut sink) {
            Ok(()) => StreamMessage::Complete,
            Err(error) => StreamMessage::Error(error),
        };
        let _ = sender.blocking_send(terminal);
    });

    let first = match tokio::time::timeout(timeout, receiver.recv()).await {
        Ok(Some(StreamMessage::Part(KvStreamPart::Entry(entry)))) => entry,
        Ok(Some(StreamMessage::Error(error))) => return platform_error(&error),
        Ok(Some(StreamMessage::Part(KvStreamPart::Bytes(_)) | StreamMessage::Complete))
        | Ok(None) => return platform_error(&protocol_error()),
        Err(_) => {
            return platform_error(&PlatformError::new(
                ErrorCode::KvUnavailable,
                "KV namespace operation timed out",
            ));
        }
    };
    let value_length = first.as_ref().map_or(0, |entry| entry.value_length);
    let prefix = match encode_stream_header(first) {
        Ok(prefix) => prefix,
        Err(error) => return platform_error(&error),
    };
    let content_length = prefix.len().saturating_add(value_length);
    let deadline = tokio::time::Instant::now() + timeout;
    let tail = futures::stream::unfold(
        (receiver, deadline),
        |(mut receiver, deadline)| async move {
            let Ok(message) = tokio::time::timeout_at(deadline, receiver.recv()).await else {
                return Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        ErrorCode::KvUnavailable.as_str(),
                    )),
                    (receiver, deadline),
                ));
            };
            match message {
                Some(StreamMessage::Part(KvStreamPart::Bytes(bytes))) => Some((
                    Ok::<_, std::io::Error>(Bytes::from(bytes)),
                    (receiver, deadline),
                )),
                Some(StreamMessage::Error(error)) => Some((
                    Err(std::io::Error::other(error.code().as_str())),
                    (receiver, deadline),
                )),
                Some(StreamMessage::Complete) | None => None,
                Some(StreamMessage::Part(KvStreamPart::Entry(_))) => Some((
                    Err(std::io::Error::other(
                        ErrorCode::KvInternalProtocolError.as_str(),
                    )),
                    (receiver, deadline),
                )),
            }
        },
    );
    let body = futures::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from(prefix)) })
        .chain(tail);
    let mut response = Response::new(Body::from_stream(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(FRAME_CONTENT_TYPE),
    );
    if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
        response.headers_mut().insert(header::CONTENT_LENGTH, value);
    }
    response
}

fn encode_stream_header(
    entry: Option<open_compute_storage::KvEntryInfo>,
) -> Result<Vec<u8>, PlatformError> {
    let mut output = b"KVS1".to_vec();
    let Some(entry) = entry else {
        output.push(0);
        output.extend_from_slice(&(-1_i64).to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        return Ok(output);
    };
    output.push(1);
    output.extend_from_slice(&entry.expires_at_ms.unwrap_or(-1).to_be_bytes());
    if let Some(metadata) = entry.metadata_json {
        let length = u32::try_from(metadata.len()).map_err(|_| kv_protocol_error())?;
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(&metadata);
    } else {
        output.extend_from_slice(&u32::MAX.to_be_bytes());
    }
    let value_length = u32::try_from(entry.value_length).map_err(|_| kv_protocol_error())?;
    output.extend_from_slice(&value_length.to_be_bytes());
    Ok(output)
}

async fn stage_put_frame(
    storage: &PlatformStorage,
    binding: &AuthorizedBinding,
    request_id: &str,
    body: Body,
    stream_budget: &StreamBudget,
    timeout: Duration,
    metrics: Option<&Arc<MetricsRegistry>>,
) -> Result<KvCommand, PlatformError> {
    let stream_permits = stream_budget.acquire(binding.resource.id, timeout).await?;
    let mut staging_metric = KvStagingGauge::new(metrics);
    let mut stream = body.into_data_stream();
    let mut header_bytes = Vec::with_capacity(4100);
    let mut header_end: Option<usize> = None;
    let mut header: Option<FramePutHeader> = None;
    let mut staged: Option<(std::path::PathBuf, tokio::fs::File)> = None;
    let mut value_length = 0_usize;

    loop {
        let chunk = match stream.try_next().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                cleanup_staged(&mut staged).await;
                return Err(kv_protocol_error());
            }
        };
        let mut remaining = chunk.as_ref();
        while !remaining.is_empty() {
            if header.is_none() {
                let needed = match header_end {
                    Some(end) => end.saturating_sub(header_bytes.len()),
                    None => 4_usize.saturating_sub(header_bytes.len()),
                };
                let take = needed.min(remaining.len());
                header_bytes.extend_from_slice(&remaining[..take]);
                remaining = &remaining[take..];
                if header_end.is_none() && header_bytes.len() == 4 {
                    let length = usize::try_from(u32::from_be_bytes(
                        header_bytes[..4]
                            .try_into()
                            .map_err(|_| kv_protocol_error())?,
                    ))
                    .map_err(|_| kv_protocol_error())?;
                    if length > 4096 {
                        return Err(kv_protocol_error());
                    }
                    header_end = Some(4_usize.checked_add(length).ok_or_else(kv_protocol_error)?);
                }
                if header_end.is_some_and(|end| header_bytes.len() == end) {
                    let parsed = parse_json::<FramePutHeader>(&header_bytes[4..])?;
                    open_compute_storage::validate_key(&parsed.key)?;
                    let paths = open_compute_storage::KvPaths::open(storage.data_dir().root())?;
                    let path = paths.create_write_staging(binding.resource.id, request_id)?;
                    let Ok(file) = tokio::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .await
                    else {
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(PlatformError::new(
                            ErrorCode::KvUnavailable,
                            "KV value staging file is unavailable",
                        ));
                    };
                    staged = Some((path, file));
                    header = Some(parsed);
                }
                continue;
            }

            value_length = value_length
                .checked_add(remaining.len())
                .ok_or_else(value_too_large)?;
            if value_length > open_compute_storage::KV_MAX_VALUE_BYTES {
                cleanup_staged(&mut staged).await;
                return Err(value_too_large());
            }
            let staged_bytes = remaining.len();
            if let Err(error) = ensure_storage_headroom(storage, remaining.len()) {
                cleanup_staged(&mut staged).await;
                return Err(error);
            }
            if let Some((_, file)) = staged.as_mut()
                && file.write_all(remaining).await.is_err()
            {
                cleanup_staged(&mut staged).await;
                return Err(PlatformError::new(
                    ErrorCode::KvStorageFull,
                    "failed to stage KV value bytes",
                ));
            }
            staging_metric.add(staged_bytes);
            remaining = &[];
        }
    }

    let Some(header) = header else {
        return Err(kv_protocol_error());
    };
    let Some((path, file)) = staged else {
        return Err(kv_protocol_error());
    };
    if file.sync_all().await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(PlatformError::new(
            ErrorCode::KvStorageFull,
            "failed to sync KV value staging file",
        ));
    }
    let mut file = file.into_std().await;
    std::io::Seek::rewind(&mut file).map_err(|_| {
        PlatformError::new(
            ErrorCode::KvUnavailable,
            "KV value staging file is unavailable",
        )
    })?;
    Ok(KvCommand::PutStaged {
        key: header.key,
        value: KvStagedValue::with_lease(path, file, value_length, stream_permits)
            .with_staging_metric(staging_metric),
        expiration: header.expiration,
        expiration_ttl: header.expiration_ttl,
        metadata: header.metadata,
        metadata_present: header.metadata_present,
    })
}

async fn cleanup_staged(staged: &mut Option<(std::path::PathBuf, tokio::fs::File)>) {
    if let Some((path, file)) = staged.take() {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::remove_dir(parent).await;
        }
    }
}

fn value_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvValueTooLarge,
        "KV value exceeds the 25 MiB limit",
    )
}

fn kv_busy() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvBusy,
        "KV active stream limit is temporarily saturated",
    )
}

fn parse_frame_command(operation: Operation, bytes: &[u8]) -> Result<KvCommand, PlatformError> {
    match operation {
        Operation::Get | Operation::GetWithMetadata | Operation::GetMany => {
            let request = parse_json::<FrameGetRequest>(bytes)?;
            let valid_count = match operation {
                Operation::Get | Operation::GetWithMetadata => request.keys.len() == 1,
                Operation::GetMany => {
                    request.keys.len() <= open_compute_storage::KV_MAX_MULTI_GET_KEYS
                }
                _ => false,
            };
            if !valid_count {
                return Err(PlatformError::new(
                    ErrorCode::KvTooManyKeys,
                    "KV get key count is outside the supported range",
                ));
            }
            for key in &request.keys {
                open_compute_storage::validate_key(key)?;
            }
            Ok(KvCommand::Get {
                keys: request.keys,
                cache_ttl: request.cache_ttl,
            })
        }
        Operation::Put => {
            if bytes.len() < 4 {
                return Err(kv_protocol_error());
            }
            let header_len = usize::try_from(u32::from_be_bytes(
                bytes[..4].try_into().map_err(|_| kv_protocol_error())?,
            ))
            .map_err(|_| kv_protocol_error())?;
            let header_end = 4_usize
                .checked_add(header_len)
                .ok_or_else(kv_protocol_error)?;
            if header_len > 4096 || header_end > bytes.len() {
                return Err(kv_protocol_error());
            }
            let header = parse_json::<FramePutHeader>(&bytes[4..header_end])?;
            open_compute_storage::validate_key(&header.key)?;
            let value = bytes[header_end..].to_vec();
            if value.len() > open_compute_storage::KV_MAX_VALUE_BYTES {
                return Err(PlatformError::new(
                    ErrorCode::KvValueTooLarge,
                    "KV value exceeds the 25 MiB limit",
                ));
            }
            Ok(KvCommand::Put {
                key: header.key,
                value,
                expiration: header.expiration,
                expiration_ttl: header.expiration_ttl,
                metadata: header.metadata,
                metadata_present: header.metadata_present,
            })
        }
        Operation::Delete => {
            let request = parse_json::<KeyRequest>(bytes)?;
            open_compute_storage::validate_key(&request.key)?;
            Ok(KvCommand::Delete { key: request.key })
        }
        Operation::List => {
            let request = parse_json::<FrameListRequest>(bytes)?;
            Ok(KvCommand::List {
                prefix: request.prefix,
                limit: request.limit,
                cursor: request.cursor,
            })
        }
        Operation::Echo => Err(kv_protocol_error()),
    }
}

fn encode_frame_result(
    operation: Operation,
    result: KvCommandResult,
) -> Result<(&'static str, Vec<u8>), PlatformError> {
    match (operation, result) {
        (Operation::Get | Operation::GetWithMetadata, KvCommandResult::Entries(mut entries))
            if entries.len() == 1 =>
        {
            let mut bytes = b"KVS1".to_vec();
            encode_entry(&mut bytes, entries.pop().unwrap_or(None))?;
            Ok((FRAME_CONTENT_TYPE, bytes))
        }
        (Operation::GetMany, KvCommandResult::Entries(entries)) => {
            let count = u16::try_from(entries.len()).map_err(|_| kv_protocol_error())?;
            let mut bytes = b"KVB1".to_vec();
            bytes.extend_from_slice(&count.to_be_bytes());
            for entry in entries {
                encode_entry(&mut bytes, entry)?;
            }
            Ok((FRAME_CONTENT_TYPE, bytes))
        }
        (Operation::Put | Operation::Delete, KvCommandResult::Mutation) => {
            Ok((FRAME_CONTENT_TYPE, Vec::new()))
        }
        (
            Operation::List,
            KvCommandResult::List {
                rows,
                complete,
                cursor,
            },
        ) => {
            let keys = rows
                .into_iter()
                .map(|row| {
                    let name = String::from_utf8(row.key).map_err(|_| {
                        PlatformError::new(ErrorCode::KvCorrupt, "KV key is not valid UTF-8")
                    })?;
                    let metadata = row
                        .metadata_json
                        .map(|bytes| {
                            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| {
                                PlatformError::new(
                                    ErrorCode::KvCorrupt,
                                    "KV metadata is not canonical JSON",
                                )
                            })
                        })
                        .transpose()?;
                    Ok(serde_json::json!({
                        "name": name,
                        "expiration": row.expires_at_ms.map(|value| value / 1000),
                        "metadata": metadata,
                    }))
                })
                .collect::<Result<Vec<_>, PlatformError>>()?;
            let bytes = serde_json::to_vec(&serde_json::json!({
                "keys": keys,
                "list_complete": complete,
                "cursor": cursor,
            }))
            .map_err(|_| kv_protocol_error())?;
            Ok(("application/json", bytes))
        }
        _ => Err(kv_protocol_error()),
    }
}

fn encode_entry(
    output: &mut Vec<u8>,
    entry: Option<open_compute_storage::KvEntry>,
) -> Result<(), PlatformError> {
    let Some(entry) = entry else {
        output.push(0);
        output.extend_from_slice(&(-1_i64).to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        output.extend_from_slice(&u32::MAX.to_be_bytes());
        return Ok(());
    };
    output.push(1);
    output.extend_from_slice(&entry.expires_at_ms.unwrap_or(-1).to_be_bytes());
    if let Some(metadata) = entry.metadata_json {
        let length = u32::try_from(metadata.len()).map_err(|_| kv_protocol_error())?;
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(&metadata);
    } else {
        output.extend_from_slice(&u32::MAX.to_be_bytes());
    }
    let length = u32::try_from(entry.value.len()).map_err(|_| kv_protocol_error())?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&entry.value);
    Ok(())
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
        Operation::GetWithMetadata | Operation::GetMany | Operation::List => Err(protocol_error()),
        Operation::Echo => unreachable!("echo returns before buffering"),
    };
    let result = match parsed {
        Ok(parsed) => {
            let executor = executor.clone();
            let timeout = executor.operation_timeout();
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
            match tokio::time::timeout(timeout, blocking).await {
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
        "get-with-metadata" => Operation::GetWithMetadata,
        "get-many" => Operation::GetMany,
        "put" => Operation::Put,
        "delete" => Operation::Delete,
        "list" => Operation::List,
        "echo" => Operation::Echo,
        _ => return None,
    };
    Some((BindingId::from_str(id).ok()?, operation))
}

fn declared_too_large(headers: &HeaderMap) -> bool {
    let limit = if content_type_is(headers, FRAME_CONTENT_TYPE) {
        MAX_FRAME_BODY_BYTES
    } else {
        MAX_BODY_BYTES
    };
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
}

fn content_type_matches(headers: &HeaderMap, operation: Operation) -> bool {
    if content_type_is(headers, FRAME_CONTENT_TYPE) {
        return !matches!(operation, Operation::Echo);
    }
    let expected = if matches!(operation, Operation::Echo) {
        STREAM_CONTENT_TYPE
    } else {
        JSON_CONTENT_TYPE
    };
    header_text(headers, header::CONTENT_TYPE.as_str())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    header_text(headers, header::CONTENT_TYPE.as_str())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim() == expected)
}

fn permission_allows(binding: &AuthorizedBinding, operation: Operation) -> bool {
    match operation {
        Operation::Get
        | Operation::GetWithMetadata
        | Operation::GetMany
        | Operation::List
        | Operation::Echo => binding.binding.permissions.read,
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
        ErrorCode::BindingLimitExceeded
        | ErrorCode::KvKeyTooLarge
        | ErrorCode::KvValueTooLarge
        | ErrorCode::KvMetadataTooLarge
        | ErrorCode::KvResponseTooLarge
        | ErrorCode::KvTooManyKeys => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::ResourceNotReady | ErrorCode::ResourceReferenced => StatusCode::CONFLICT,
        ErrorCode::ResourceUnavailable
        | ErrorCode::BindingResultUnknown
        | ErrorCode::KvBusy
        | ErrorCode::KvStorageFull
        | ErrorCode::KvUnavailable
        | ErrorCode::KvResultUnknown => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::BindingTypeMismatch
        | ErrorCode::BindingCapabilityUnsupported
        | ErrorCode::ResourceInvariantViolation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::BindingProtocolError
        | ErrorCode::KvKeyInvalid
        | ErrorCode::KvMetadataInvalid
        | ErrorCode::KvInvalidOptions
        | ErrorCode::KvCursorInvalid
        | ErrorCode::KvInternalProtocolError => StatusCode::BAD_REQUEST,
        ErrorCode::KvCorrupt => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    backend_error(error.code(), status)
}

fn kv_protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::KvInternalProtocolError,
        "KV private protocol frame is invalid",
    )
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
