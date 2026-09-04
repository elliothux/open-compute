//! Generation-authenticated loopback ingestion endpoint for the platform tail consumer.

use crate::observability::ObservabilityService;
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::post;
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_runtime::GenerationAuthRegistry;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;

const TOKEN_HEADER: &str = "x-open-compute-observability-token";
const GENERATION_HEADER: &str = "x-open-compute-startup-generation";
const MAX_BODY: usize = 256 * 1024;

#[derive(Clone)]
struct BackendState {
    service: Arc<ObservabilityService>,
    auth: GenerationAuthRegistry,
}

/// Bind the private observability backend to an ephemeral IPv4 loopback port.
pub(crate) async fn bind_observability_backend() -> Result<TcpListener, PlatformError> {
    TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .map_err(|_| unavailable("failed to bind private observability backend listener"))
}

/// Serve generation-authenticated platform collector envelopes without public middleware.
pub(crate) async fn serve_observability_backend(
    listener: TcpListener,
    service: Arc<ObservabilityService>,
    auth: GenerationAuthRegistry,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), PlatformError> {
    let router = Router::new()
        .route("/internal/observability/v1/ingest", post(ingest))
        .with_state(BackendState { service, auth });
    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| unavailable("private observability backend listener failed"))
}

async fn ingest(State(state): State<BackendState>, request: Request) -> Response {
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|value| value > MAX_BODY)
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let token = request
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let generation = request
        .headers()
        .get(GENERATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if !state.auth.authorize(&token, &generation) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(bytes) = to_bytes(request.into_body(), MAX_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    match state.service.ingest(&bytes) {
        Ok(()) => {
            state.service.observe_ingest_result(true);
            StatusCode::ACCEPTED.into_response()
        }
        Err(error) => {
            state.service.observe_ingest_result(false);
            tracing::warn!(
                code = error.code().as_str(),
                "observability collector envelope rejected"
            );
            StatusCode::UNPROCESSABLE_ENTITY.into_response()
        }
    }
}

fn unavailable(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::RuntimeUnavailable, message)
}

#[cfg(test)]
#[path = "observability_backend_tests.rs"]
mod tests;
