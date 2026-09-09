use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DoResolveRequest {
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

pub(super) fn handle(
    State(state): State<BackendState>,
    request: Request,
) -> Pin<Box<dyn Future<Output = Response> + Send>> {
    Box::pin(async move {
        let headers = request.headers();
        let token = header_text(headers, TOKEN_HEADER).unwrap_or("").to_owned();
        let generation = header_text(headers, GENERATION_HEADER)
            .unwrap_or("")
            .to_owned();
        if !state.auth.authorize(&token, &generation) {
            return StatusCode::NOT_FOUND.into_response();
        }
        if request
            .uri()
            .path()
            .starts_with("/internal/ai/to-markdown/v1/")
        {
            return match &state.document_parser {
                Some(document_parser) => document_parser.handle(request).await,
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        if request.method() != Method::POST {
            return backend_error(
                ErrorCode::BindingProtocolError,
                StatusCode::METHOD_NOT_ALLOWED,
            );
        }
        if request.uri().path().starts_with("/internal/ai-search/v1/") {
            return match &state.ai_search {
                Some(ai_search) => ai_search.handle(request).await,
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        if request.uri().path().starts_with("/internal/vectorize/v1/") {
            let vectorize = crate::vectorize_backend::VectorizeBindingService::new(
                state.storage.clone(),
                state.pins.clone(),
            );
            let vectorize = match &state.metrics {
                Some(metrics) => vectorize.with_metrics(metrics.clone()),
                None => vectorize,
            };
            return vectorize.handle(request).await;
        }
        if request.uri().path().starts_with("/internal/services/v1/") {
            return match &state.services {
                Some(services) => {
                    handle_service_invocation(
                        services,
                        &state.auth,
                        &token,
                        &generation,
                        state.metrics.as_deref(),
                        request,
                    )
                    .await
                }
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        if request.uri().path() == "/internal/assets/v1/fetch" {
            return match &state.assets {
                Some(assets) => assets.handle(request).await,
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        if request.uri().path().starts_with("/internal/cache/v1/") {
            return match &state.cache {
                Some(cache) => cache.handle(request).await,
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        if request.uri().path().starts_with("/internal/images/v1/") {
            return match &state.images {
                Some(images) => images.handle(request).await,
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
        let version_id = match parse_header::<VersionId>(headers, VERSION_HEADER) {
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
                    version_id,
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
        if binding.binding.kind != BindingKind::KvNamespace
            || binding.binding.capability_version != 1
        {
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
    })
}

pub(super) async fn handle_service_invocation(
    registry: &crate::service_invocations::ServiceInvocationRegistry,
    auth: &GenerationAuthRegistry,
    token: &str,
    generation: &str,
    metrics: Option<&MetricsRegistry>,
    request: Request,
) -> Response {
    use crate::service_invocations::{
        CapabilityBeginRequest, ServiceConnectFinalizeRequest, ServiceReleaseRequest,
        ServiceResolveRequest, ServiceRetainRequest, ServiceRootCompleteRequest,
    };
    let path = request.uri().path().to_owned();
    let Ok(bytes) = to_bytes(request.into_body(), 16 * 1024).await else {
        return backend_error(ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST);
    };
    let started = Instant::now();
    let mut metric_operation = None;
    let response = auth.with_authorized(token, generation, || {
        registry.activate_generation(generation);
        match path.as_str() {
            "/internal/services/v1/resolve" => {
                serde_json::from_slice::<ServiceResolveRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| {
                        metric_operation = Some(match value.operation {
                            crate::service_invocations::ServiceOperation::DefaultFetch => {
                                ServiceMetricOperation::DefaultFetch
                            }
                            crate::service_invocations::ServiceOperation::NamedFetch => {
                                ServiceMetricOperation::NamedFetch
                            }
                            crate::service_invocations::ServiceOperation::Rpc => {
                                ServiceMetricOperation::Rpc
                            }
                            crate::service_invocations::ServiceOperation::Connect => {
                                ServiceMetricOperation::Connect
                            }
                        });
                        registry.resolve(&value)
                    })
                    .and_then(|value| json_response(&value))
            }
            "/internal/services/v1/capabilities/begin" => {
                metric_operation = Some(ServiceMetricOperation::Capability);
                serde_json::from_slice::<CapabilityBeginRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| registry.begin_capability(&value))
                    .and_then(|value| json_response(&value))
            }
            "/internal/services/v1/retain" => {
                serde_json::from_slice::<ServiceRetainRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| registry.retain(&value))
                    .and_then(|retention| {
                        json_response(&serde_json::json!({ "retention": retention }))
                    })
            }
            "/internal/services/v1/complete" => {
                serde_json::from_slice::<ServiceReleaseRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| registry.complete(&value))
                    .and_then(|()| json_response(&serde_json::json!({ "ok": true })))
            }
            "/internal/services/v1/release" => {
                serde_json::from_slice::<ServiceReleaseRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| registry.release(&value))
                    .and_then(|()| json_response(&serde_json::json!({ "ok": true })))
            }
            "/internal/services/v1/root/complete" => {
                serde_json::from_slice::<ServiceRootCompleteRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| registry.complete_root(&value))
                    .and_then(|()| json_response(&serde_json::json!({ "ok": true })))
            }
            "/internal/services/v1/connect/finalize" => {
                serde_json::from_slice::<ServiceConnectFinalizeRequest>(&bytes)
                    .map_err(|_| protocol_error())
                    .and_then(|value| registry.finalize_connect(&value))
                    .and_then(|()| json_response(&serde_json::json!({ "ok": true })))
            }
            _ => Ok(StatusCode::NOT_FOUND.into_response()),
        }
    });
    if let Some(metrics) = metrics {
        let (roots, operations, retentions) = registry.counts();
        metrics.set_service_invocation_counts(roots, operations, retentions);
        if let (Some(operation), Some(result)) = (metric_operation, response.as_ref()) {
            metrics.observe_service_invocation(operation, result.is_ok(), started.elapsed());
        }
    }
    match response {
        Some(Ok(response)) => response,
        Some(Err(error)) => platform_error(&error),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn json_response(value: &impl serde::Serialize) -> Result<Response, PlatformError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "private Service response serialization failed",
        )
    })?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
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
                        target_version_id: authority.version_id,
                        execution_generation: authority.route_generation,
                        retry_count,
                    },
                    open_compute_core::wall_time_ms(),
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
    let Ok(version_id) = parse_header::<VersionId>(request.headers(), VERSION_HEADER) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(descriptor) = parse_digest(request.headers()) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(bytes) = to_bytes(request.into_body(), 4096).await else {
        return backend_error(
            ErrorCode::DoInternalProtocolError,
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    };
    let body = match parse_json::<DoReadyRequest>(&bytes) {
        Ok(value) if value.object_generation > 0 => value,
        _ => {
            return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
        }
    };
    let storage = state.storage.clone();
    let result = tokio::task::spawn_blocking(move || {
        let binding =
            BindingRepository::new(storage.db()).authorize(binding_id, version_id, &descriptor)?;
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
            open_compute_core::wall_time_ms(),
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
        Some("connect") => DoOperation::Connect,
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
    let Ok(version_id) = parse_header::<VersionId>(request.headers(), VERSION_HEADER) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(descriptor) = parse_digest(request.headers()) else {
        return backend_error(ErrorCode::DoInternalProtocolError, StatusCode::BAD_REQUEST);
    };
    let Ok(bytes) = to_bytes(request.into_body(), 4096).await else {
        return backend_error(
            ErrorCode::DoInternalProtocolError,
            StatusCode::PAYLOAD_TOO_LARGE,
        );
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
            version_id,
            &descriptor,
            object_id,
            open_compute_core::wall_time_ms(),
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
