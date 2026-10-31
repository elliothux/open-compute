use super::*;
use open_compute_core::SecretBytes;
use open_compute_runtime::GenerationCredential;

const PYTHON_SNAPSHOT_PATH: &str = "/internal/runtime/v1/versions/python-snapshot";

#[derive(Clone)]
struct SourceState {
    source: RuntimeSource,
    auth: GenerationAuthRegistry,
    containers: Option<Arc<crate::ContainerService>>,
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
    containers: Option<Arc<crate::ContainerService>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let state = SourceState {
        source,
        auth,
        containers,
    };
    let router = Router::new()
        .route(SOURCE_PATH, post(resolve))
        .route(PYTHON_SNAPSHOT_PATH, post(resolve_python))
        .route(
            "/internal/runtime/v1/containers/resolve",
            post(container_resolve),
        )
        .route("/internal/runtime/v1/containers/stop", post(container_stop))
        .route(
            "/internal/runtime/v1/containers/snapshot",
            post(container_snapshot),
        )
        .route(
            "/internal/runtime/v1/containers/begin-stop",
            post(container_begin_stop),
        )
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

async fn container_resolve(State(state): State<SourceState>, request: Request) -> Response {
    let (body, credential) = match source_request::<crate::containers::ResolveRequest>(
        &state,
        request,
        MAX_SOURCE_REQUEST + open_compute_storage::crypto::MAX_CONTAINER_STARTUP_BYTES,
    )
    .await
    {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let Some(service) = &state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if state.auth.with_current(&credential, || ()).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = service
        .resolve(
            body,
            open_compute_core::wall_time_ms(),
            &state.auth,
            &credential,
        )
        .await;
    state
        .auth
        .with_current(&credential, || match result {
            Ok(body) => axum::Json(body).into_response(),
            Err(error) => source_platform_error(error),
        })
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn container_stop(State(state): State<SourceState>, request: Request) -> Response {
    let (body, credential) =
        match source_request::<crate::containers::StopRequest>(&state, request, MAX_SOURCE_REQUEST)
            .await
        {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let Some(service) = &state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if state.auth.with_current(&credential, || ()).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = service
        .stop(
            &body,
            open_compute_core::wall_time_ms(),
            &state.auth,
            &credential,
        )
        .await;
    state
        .auth
        .with_current(&credential, || match result {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => source_platform_error(error),
        })
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn container_begin_stop(State(state): State<SourceState>, request: Request) -> Response {
    let (body, credential) =
        match source_request::<crate::containers::StopRequest>(&state, request, MAX_SOURCE_REQUEST)
            .await
        {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let Some(service) = &state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service.begin_stop(
        &body,
        open_compute_core::wall_time_ms(),
        &state.auth,
        &credential,
    ) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => source_platform_error(error),
    }
}

async fn container_snapshot(State(state): State<SourceState>, request: Request) -> Response {
    #[cfg(feature = "test-support")]
    tracing::info!(
        phase = "request-received",
        "Controlled snapshot private request"
    );
    let (body, credential) = match source_request::<crate::containers::SnapshotRequest>(
        &state,
        request,
        MAX_SOURCE_REQUEST,
    )
    .await
    {
        Ok(parsed) => parsed,
        Err(response) => {
            #[cfg(feature = "test-support")]
            tracing::info!(
                phase = "request-rejected",
                status = response.status().as_u16(),
                "Controlled snapshot private request"
            );
            return response;
        }
    };
    #[cfg(feature = "test-support")]
    tracing::info!(
        phase = "request-validated",
        "Controlled snapshot private request"
    );
    let Some(service) = &state.containers else {
        #[cfg(feature = "test-support")]
        tracing::info!(
            phase = "service-unavailable",
            "Controlled snapshot private request"
        );
        return StatusCode::NOT_FOUND.into_response();
    };
    if state.auth.with_current(&credential, || ()).is_none() {
        #[cfg(feature = "test-support")]
        tracing::info!(
            phase = "generation-rejected",
            "Controlled snapshot private request"
        );
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = service
        .capture_snapshot(body, state.auth.clone(), credential.clone())
        .await;
    #[cfg(feature = "test-support")]
    if let Err(error) = &result {
        tracing::info!(phase = "capture-rejected", code = %error.code(), "Controlled snapshot private request");
    }
    state
        .auth
        .with_current(&credential, || match result {
            Ok(body) => axum::Json(body).into_response(),
            Err(error) => source_platform_error(error),
        })
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    key: String,
    expected_worker_code_sha256: String,
    scope: SourceScope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolvePythonRequest {
    key: String,
    expected_worker_code_sha256: String,
    scope: SourceScope,
    expected_prepared_sha256: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SourceScope {
    Runtime,
    Validation,
    Preparation,
    Probe,
}

impl From<SourceScope> for RuntimeScope {
    fn from(scope: SourceScope) -> Self {
        match scope {
            SourceScope::Runtime => Self::Runtime,
            SourceScope::Validation => Self::Validation,
            SourceScope::Preparation => Self::Preparation,
            SourceScope::Probe => Self::Probe,
        }
    }
}

async fn source_request<T: serde::de::DeserializeOwned>(
    state: &SourceState,
    request: Request,
    max_bytes: usize,
) -> Result<(T, GenerationCredential), Response> {
    let credential = state
        .auth
        .credential()
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    let token = request
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let generation = request
        .headers()
        .get(GENERATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !state.auth.authorize(token, generation) {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > max_bytes)
    {
        return Err(source_error(
            ErrorCode::BundleTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }
    let Ok(bytes) = to_bytes(request.into_body(), max_bytes).await else {
        return Err(source_error(
            ErrorCode::BundleTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    };
    let body = serde_json::from_slice(&bytes)
        .map_err(|_| source_error(ErrorCode::BundleInvalid, StatusCode::BAD_REQUEST))?;
    Ok((body, credential))
}

async fn resolve(State(state): State<SourceState>, request: Request) -> Response {
    let (body, credential): (ResolveRequest, _) =
        match source_request(&state, request, MAX_SOURCE_REQUEST).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let snapshot = match state
        .source
        .resolve(
            &body.key,
            &body.expected_worker_code_sha256,
            body.scope.into(),
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return source_platform_error(error),
    };
    let payload = match RuntimeSource::internal_payload(&snapshot) {
        Ok(payload) => payload,
        Err(error) => return source_platform_error(error),
    };
    state
        .auth
        .with_current(&credential, || {
            let mut response = Response::new(Body::from(payload.expose().to_vec()));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        })
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

// Bytes owns this zeroizing buffer until the final private response frame is released.
struct SnapshotBody(SecretBytes);

impl AsRef<[u8]> for SnapshotBody {
    fn as_ref(&self) -> &[u8] {
        self.0.expose()
    }
}

async fn resolve_python(State(state): State<SourceState>, request: Request) -> Response {
    let (body, credential): (ResolvePythonRequest, _) =
        match source_request(&state, request, MAX_SOURCE_REQUEST).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };
    let snapshot = match state
        .source
        .resolve_python_prepared(
            &body.key,
            &body.expected_worker_code_sha256,
            body.scope.into(),
            &body.expected_prepared_sha256,
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return source_platform_error(error),
    };
    state
        .auth
        .with_current(&credential, || {
            let mut response =
                Response::new(Body::from(bytes::Bytes::from_owner(SnapshotBody(snapshot))));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        })
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

pub(super) fn source_platform_error(error: PlatformError) -> Response {
    let status = match error.code() {
        ErrorCode::VersionNotReady => StatusCode::CONFLICT,
        ErrorCode::ArtifactUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ArtifactIntegrityError
        | ErrorCode::VersionInvariantViolation
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

#[cfg(test)]
#[path = "source_server_tests.rs"]
mod tests;
