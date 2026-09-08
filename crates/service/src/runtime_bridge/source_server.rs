use super::*;

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

#[allow(
    clippy::needless_pass_by_value,
    reason = "the callback contract transfers ownership of this value"
)]
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
