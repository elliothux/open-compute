//! Generation-authenticated private backend for typed resource-binding adapters.

use crate::d1_backend::D1BindingService;
use crate::kv_backend::{KvCommand, KvCommandResult, KvStreamPart};
use crate::metrics::{AlarmMutation, DoOperation, MetricsRegistry};
use crate::queue_backend::QueueBindingService;
use crate::r2_backend::R2BindingService;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{
    BindingId, BindingKind, DeploymentId, DurableObjectId, DurableObjectsConfig, ErrorCode,
    OperationClass, PlatformError, QueuesConfig, ResourceId,
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
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

mod kv;
use kv::{
    FRAME_CONTENT_TYPE, StreamBudget, declared_too_large, dispatch, parse_path, permission_allows,
};

const TOKEN_HEADER: &str = "x-open-compute-binding-token";
const GENERATION_HEADER: &str = "x-open-compute-startup-generation";
const DEPLOYMENT_HEADER: &str = "x-open-compute-deployment-id";
const DESCRIPTOR_HEADER: &str = "x-open-compute-descriptor-sha256";
const REQUEST_HEADER: &str = "x-open-compute-request-id";
const ROUTE_GENERATION_HEADER: &str = "x-open-compute-route-generation";
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

/// Serve every composed product plane on the private binding listener.
#[allow(clippy::too_many_arguments)]
pub async fn serve_binding_backend(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    queue_config: QueuesConfig,
    workflow_config: open_compute_core::WorkflowsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    serve_binding_backend_inner(
        listener,
        storage,
        auth,
        pins,
        executor,
        metrics,
        r2,
        d1,
        do_config,
        queue_config,
        workflow_config,
        scheduler,
        None,
        shutdown,
    )
    .await
}

/// Serve every product plane plus the deployment-scoped static-assets binding backend.
#[allow(clippy::too_many_arguments)]
pub async fn serve_binding_backend_with_assets(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    queue_config: QueuesConfig,
    workflow_config: open_compute_core::WorkflowsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    assets: Arc<crate::asset_backend::AssetBindingService>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    serve_binding_backend_inner(
        listener,
        storage,
        auth,
        pins,
        executor,
        metrics,
        r2,
        d1,
        do_config,
        queue_config,
        workflow_config,
        scheduler,
        Some(assets),
        shutdown,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn serve_binding_backend_inner(
    listener: TcpListener,
    storage: Arc<PlatformStorage>,
    auth: GenerationAuthRegistry,
    pins: ResourcePins,
    executor: Arc<dyn KvBindingExecutor>,
    metrics: Option<Arc<MetricsRegistry>>,
    r2: Option<Arc<R2BindingService>>,
    d1: Option<Arc<D1BindingService>>,
    do_config: DurableObjectsConfig,
    queue_config: QueuesConfig,
    workflow_config: open_compute_core::WorkflowsConfig,
    scheduler: Option<Arc<SchedulerStore>>,
    assets: Option<Arc<crate::asset_backend::AssetBindingService>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let (global_streams, resource_streams) = executor.stream_limits();
    let queue = scheduler.as_ref().map(|scheduler| {
        let service = QueueBindingService::new(storage.clone(), scheduler.clone())
            .with_concurrency_limits(
                queue_config.max_in_flight_requests,
                queue_config.max_in_flight_requests_per_binding,
            );
        Arc::new(match &metrics {
            Some(metrics) => service.with_metrics(metrics.clone()),
            None => service,
        })
    });
    let workflow = scheduler
        .as_ref()
        .map(|scheduler| {
            crate::workflow_backend::WorkflowBindingService::new(
                storage.clone(),
                scheduler.clone(),
                workflow_config,
            )
            .map(|service| match &metrics {
                Some(metrics) => service.with_metrics(metrics.clone()),
                None => service,
            })
        })
        .transpose()?
        .map(Arc::new);
    let state = BackendState {
        storage,
        auth,
        pins,
        executor,
        metrics,
        stream_budget: StreamBudget::new(global_streams, resource_streams),
        r2,
        d1,
        do_config,
        scheduler,
        queue,
        workflow,
        assets,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DoResolveRequest {
    object_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DoReadyRequest {
    namespace_resource_id: ResourceId,
    object_id: DurableObjectId,
    object_generation: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlarmRequest {
    namespace_resource_id: ResourceId,
    object_id: DurableObjectId,
    object_generation: u64,
    #[serde(default)]
    scheduled_time_ms: Option<i64>,
    #[serde(default)]
    retry_count: Option<u8>,
    #[serde(default)]
    row_token: Option<String>,
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
    if request.uri().path() == "/internal/assets/v1/fetch" {
        return match &state.assets {
            Some(assets) => assets.handle(request).await,
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if request.uri().path().starts_with("/internal/alarms/v1/") {
        return handle_alarm_index(state, request).await;
    }
    if request
        .uri()
        .path()
        .starts_with("/internal/workflows/runs/")
        || request
            .uri()
            .path()
            .starts_with("/internal/bindings/v1/workflow/")
    {
        return match &state.workflow {
            Some(workflow) => workflow.handle(request, state.auth.clone()).await,
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if request
        .uri()
        .path()
        .starts_with("/internal/bindings/v1/queue/")
    {
        return match &state.queue {
            Some(queue) => queue.handle(request).await,
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if request
        .uri()
        .path()
        .starts_with("/internal/bindings/v1/do/")
    {
        return if request.uri().path().ends_with("/ready") {
            acknowledge_durable_object(state, request).await
        } else {
            resolve_durable_object(state, request).await
        };
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
            metrics.observe_kv_operation(
                operation.kv_metric(),
                response.status().is_success(),
                ingress_bytes,
                egress_bytes,
                started.elapsed(),
            );
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
    if !content_type_is(headers, FRAME_CONTENT_TYPE) {
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
    observe(dispatch(state.clone(), binding, operation, request_id, request, pin).await)
}

async fn handle_alarm_index(state: BackendState, request: Request) -> Response {
    if !content_type_is(request.headers(), "application/json")
        || !valid_request_id(request.headers())
    {
        return backend_error(
            ErrorCode::SchedulerInternalProtocolError,
            StatusCode::BAD_REQUEST,
        );
    }
    let operation = request
        .uri()
        .path()
        .strip_prefix("/internal/alarms/v1/")
        .unwrap_or("")
        .to_owned();
    if !matches!(
        operation.as_str(),
        "resolve" | "upsert" | "delete" | "clear"
    ) {
        return backend_error(
            ErrorCode::SchedulerInternalProtocolError,
            StatusCode::NOT_FOUND,
        );
    }
    let Ok(bytes) = to_bytes(request.into_body(), 4096).await else {
        return backend_error(
            ErrorCode::SchedulerInternalProtocolError,
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    };
    let body = match parse_json::<AlarmRequest>(&bytes) {
        Ok(value) if value.object_generation > 0 => value,
        _ => {
            return backend_error(
                ErrorCode::SchedulerInternalProtocolError,
                StatusCode::BAD_REQUEST,
            );
        }
    };
    let storage = state.storage.clone();
    let scheduler = state.scheduler.clone();
    let metrics = state.metrics.clone();
    let mutation = match operation.as_str() {
        "upsert" => Some(AlarmMutation::Set),
        "delete" => Some(AlarmMutation::Delete),
        "clear" => Some(AlarmMutation::Clear),
        _ => None,
    };
    let admission = if operation == "upsert" {
        let result = state.storage.reserve_mutation(64 * 1024);
        if let Some(metrics) = &state.metrics {
            metrics.observe_admission(
                OperationClass::Scheduler,
                result.as_ref().err().map(PlatformError::code),
            );
        }
        match result {
            Ok(reservation) => Some(reservation),
            Err(error) => return platform_error(&error),
        }
    } else {
        None
    };
    let result = tokio::task::spawn_blocking(move || {
        let _admission = admission;
        let authority = DurableObjectRepository::new(&storage).authorize_alarm_dispatch(
            body.namespace_resource_id,
            body.object_id,
            body.object_generation,
        )?;
        match operation.as_str() {
            "resolve" => {
                if body.scheduled_time_ms.is_some()
                    || body.retry_count.is_some()
                    || body.row_token.is_some()
                {
                    return Err(alarm_protocol_error());
                }
                serde_json::to_vec(&authority)
                    .map(Some)
                    .map_err(|_| alarm_protocol_error())
            }
            "upsert" => {
                let store = scheduler.as_ref().ok_or_else(alarm_unavailable)?;
                let (Some(due_at_ms), Some(retry_count), Some(row_token)) =
                    (body.scheduled_time_ms, body.retry_count, body.row_token)
                else {
                    return Err(alarm_protocol_error());
                };
                store.upsert_alarm(
                    &AlarmProjection {
                        namespace_resource_id: body.namespace_resource_id,
                        object_id: body.object_id,
                        object_generation: body.object_generation,
                        row_token,
                        due_at_ms,
                        target_deployment_id: authority.deployment_id,
                        execution_generation: authority.route_generation,
                        retry_count,
                    },
                    unix_ms(),
                )?;
                Ok(None)
            }
            "delete" => {
                let store = scheduler.as_ref().ok_or_else(alarm_unavailable)?;
                if body.scheduled_time_ms.is_some() || body.retry_count.is_some() {
                    return Err(alarm_protocol_error());
                }
                let Some(row_token) = body.row_token else {
                    return Err(alarm_protocol_error());
                };
                store.delete_alarm_exact(
                    body.namespace_resource_id,
                    body.object_id,
                    body.object_generation,
                    &row_token,
                )?;
                Ok(None)
            }
            "clear" => {
                let store = scheduler.as_ref().ok_or_else(alarm_unavailable)?;
                if body.scheduled_time_ms.is_some()
                    || body.retry_count.is_some()
                    || body.row_token.is_some()
                {
                    return Err(alarm_protocol_error());
                }
                store.delete_object(
                    body.namespace_resource_id,
                    body.object_id,
                    body.object_generation,
                )?;
                Ok(None)
            }
            _ => Err(alarm_protocol_error()),
        }
    })
    .await;
    if let (Some(metrics), Some(mutation)) = (metrics, mutation) {
        metrics.inc_alarm_mutation(mutation, matches!(&result, Ok(Ok(_))));
    }
    match result {
        Ok(Ok(Some(bytes))) => {
            let mut response = Response::new(Body::from(bytes));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => platform_error(&error),
        Err(_) => backend_error(
            ErrorCode::SchedulerUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

async fn acknowledge_durable_object(state: BackendState, request: Request) -> Response {
    let Some(binding_id) = parse_do_path(request.uri().path(), "ready") else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::NOT_FOUND);
    };
    if !content_type_is(request.headers(), "application/json")
        || !valid_request_id(request.headers())
    {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    }
    let Ok(deployment_id) = parse_header::<DeploymentId>(request.headers(), DEPLOYMENT_HEADER)
    else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(descriptor) = parse_digest(request.headers()) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(bytes) = to_bytes(request.into_body(), 4096).await else {
        return backend_error(ErrorCode::DoRpcUnsupported, StatusCode::PAYLOAD_TOO_LARGE);
    };
    let body = match parse_json::<DoReadyRequest>(&bytes) {
        Ok(value) if value.object_generation > 0 => value,
        _ => {
            return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
        }
    };
    let storage = state.storage.clone();
    let result = tokio::task::spawn_blocking(move || {
        let binding = BindingRepository::new(storage.db()).authorize(
            binding_id,
            deployment_id,
            &descriptor,
        )?;
        if binding.binding.kind != BindingKind::DoNamespace
            || binding.resource.id != body.namespace_resource_id
        {
            return Err(PlatformError::new(
                ErrorCode::DoInternalProtocolError,
                "Durable Object ready acknowledgement is outside binding authority",
            ));
        }
        DurableObjectRepository::new(&storage).finish_object_create(
            body.namespace_resource_id,
            body.object_id,
            body.object_generation,
            unix_ms(),
        )?;
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => platform_error(&error),
        Err(_) => backend_error(
            ErrorCode::DoStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

async fn resolve_durable_object(state: BackendState, request: Request) -> Response {
    let started = Instant::now();
    let operation = match header_text(request.headers(), "x-open-compute-do-operation") {
        Some("fetch") => DoOperation::Fetch,
        Some("rpc") => DoOperation::Rpc,
        _ => {
            return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
        }
    };
    let Some(binding_id) = parse_do_resolve_path(request.uri().path()) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::NOT_FOUND);
    };
    if !content_type_is(request.headers(), "application/json")
        || !valid_request_id(request.headers())
    {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    }
    let Ok(deployment_id) = parse_header::<DeploymentId>(request.headers(), DEPLOYMENT_HEADER)
    else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(descriptor) = parse_digest(request.headers()) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let route_generation = match parse_header::<u64>(request.headers(), ROUTE_GENERATION_HEADER) {
        Ok(value) if value > 0 => value,
        _ => {
            return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
        }
    };
    let Ok(bytes) = to_bytes(request.into_body(), 4096).await else {
        return backend_error(ErrorCode::DoRpcUnsupported, StatusCode::PAYLOAD_TOO_LARGE);
    };
    let Ok(body) = parse_json::<DoResolveRequest>(&bytes) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(object_id) = DurableObjectId::from_str(&body.object_id) else {
        return backend_error(ErrorCode::DoIdInvalid, StatusCode::BAD_REQUEST);
    };
    let used_percent = match state.storage.filesystem_used_percent() {
        Ok(value) => value,
        Err(error) => return platform_error(&error),
    };
    if let Some(metrics) = &state.metrics {
        let watermark = if used_percent >= state.do_config.disk_stop_writes_percent {
            2
        } else if used_percent >= state.do_config.disk_high_watermark_percent {
            1
        } else {
            0
        };
        metrics.set_do_storage_watermark(watermark);
    }
    let allow_create = used_percent < state.do_config.disk_stop_writes_percent;
    let admission = state.storage.reserve_mutation(64 * 1024);
    if let Some(metrics) = &state.metrics {
        metrics.observe_admission(
            OperationClass::DurableObjects,
            admission.as_ref().err().map(PlatformError::code),
        );
    }
    let _admission = match admission {
        Ok(value) => value,
        Err(error) => return platform_error(&error),
    };
    let storage = state.storage.clone();
    let result = tokio::task::spawn_blocking(move || {
        DurableObjectRepository::new(&storage).authorize_dispatch(
            binding_id,
            deployment_id,
            &descriptor,
            route_generation,
            object_id,
            unix_ms(),
            allow_create,
        )
    })
    .await;
    let success = matches!(&result, Ok(Ok(_)));
    if let Some(metrics) = &state.metrics {
        metrics.observe_do_dispatch(operation, success, started.elapsed());
        if success
            && let Ok(hosts) = DurableObjectRepository::new(&state.storage).count_live_objects()
        {
            metrics.set_do_active_hosts(hosts);
        }
    }
    match result {
        Ok(Ok(authority)) => match serde_json::to_vec(&authority) {
            Ok(bytes) => {
                let mut response = Response::new(Body::from(bytes));
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response
            }
            Err(_) => backend_error(
                ErrorCode::DoInternalProtocolError,
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        },
        Ok(Err(error)) => platform_error(&error),
        Err(_) => backend_error(
            ErrorCode::DoStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    }
}

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
        | ErrorCode::DoDeploymentStale
        | ErrorCode::DoNamespaceNotEmpty => StatusCode::CONFLICT,
        ErrorCode::ResourceUnavailable
        | ErrorCode::KvBusy
        | ErrorCode::KvStorageFull
        | ErrorCode::KvUnavailable
        | ErrorCode::KvResultUnknown => StatusCode::SERVICE_UNAVAILABLE,
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
        | ErrorCode::DoPlacementOptionUnsupported
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

fn unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
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
