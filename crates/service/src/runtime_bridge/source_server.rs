use super::*;
use open_compute_core::SecretBytes;

const PYTHON_SNAPSHOT_PATH: &str = "/internal/runtime/v1/versions/python-snapshot";

#[derive(Clone)]
struct SourceState {
    source: RuntimeSource,
    containers: Option<Arc<crate::ContainerService>>,
}

/// Bind the private `RuntimeSource` endpoint to a private Unix socket.
pub async fn bind_runtime_source(
    path: &Path,
) -> Result<crate::http::PrivateUnixListener, PlatformError> {
    crate::http::PrivateUnixListener::bind(path.to_owned())
}

/// Serve `RuntimeSource` without the public HTTP logging/body middleware.
pub async fn serve_runtime_source(
    mut listener: crate::http::PrivateUnixListener,
    source: RuntimeSource,
    containers: Option<Arc<crate::ContainerService>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let state = SourceState { source, containers };
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
    axum::serve(listener.take_listener()?, router.into_make_service())
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
    // A non-secret process epoch scopes native Container control grants.
    let generation = request
        .headers()
        .get("x-open-compute-startup-generation")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if generation.is_empty()
        || generation.len() > 128
        || generation.bytes().any(|byte| byte.is_ascii_control())
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let body = match source_request::<crate::containers::ResolveRequest>(
        request,
        MAX_SOURCE_REQUEST + open_compute_storage::crypto::MAX_CONTAINER_STARTUP_BYTES,
    )
    .await
    {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(service) = state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service
        .resolve(body, open_compute_core::wall_time_ms(), &generation)
        .await
    {
        Ok(body) => axum::Json(body).into_response(),
        Err(error) => source_platform_error(error),
    }
}

async fn container_stop(State(state): State<SourceState>, request: Request) -> Response {
    let body =
        match source_request::<crate::containers::StopRequest>(request, MAX_SOURCE_REQUEST).await {
            Ok(body) => body,
            Err(response) => return response,
        };
    let Some(service) = state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service.stop(&body, open_compute_core::wall_time_ms()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => source_platform_error(error),
    }
}

async fn container_begin_stop(State(state): State<SourceState>, request: Request) -> Response {
    let body =
        match source_request::<crate::containers::StopRequest>(request, MAX_SOURCE_REQUEST).await {
            Ok(body) => body,
            Err(response) => return response,
        };
    let Some(service) = state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service.begin_stop(&body, open_compute_core::wall_time_ms()) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => source_platform_error(error),
    }
}

async fn container_snapshot(State(state): State<SourceState>, request: Request) -> Response {
    let body =
        match source_request::<crate::containers::SnapshotRequest>(request, MAX_SOURCE_REQUEST)
            .await
        {
            Ok(body) => body,
            Err(response) => return response,
        };
    let Some(service) = state.containers else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service.capture_snapshot(body).await {
        Ok(body) => axum::Json(body).into_response(),
        Err(error) => source_platform_error(error),
    }
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
    request: Request,
    max_bytes: usize,
) -> Result<T, Response> {
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
    Ok(body)
}

async fn resolve(State(state): State<SourceState>, request: Request) -> Response {
    let body: ResolveRequest = match source_request(request, MAX_SOURCE_REQUEST).await {
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

    let mut response = Response::new(Body::from(payload.expose().to_vec()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

// Bytes owns this zeroizing buffer until the final private response frame is released.
struct SnapshotBody(SecretBytes);

impl AsRef<[u8]> for SnapshotBody {
    fn as_ref(&self) -> &[u8] {
        self.0.expose()
    }
}

async fn resolve_python(State(state): State<SourceState>, request: Request) -> Response {
    let body: ResolvePythonRequest = match source_request(request, MAX_SOURCE_REQUEST).await {
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

    let mut response = Response::new(Body::from(bytes::Bytes::from_owner(SnapshotBody(snapshot))));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
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
