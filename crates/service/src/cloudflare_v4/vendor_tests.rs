use super::*;
use crate::cloudflare_v4::wire::V4Role;
use crate::health::HealthCoordinator;
use crate::metrics::MetricsRegistry;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use open_compute_core::RequestId;
use open_compute_core::config::MetricsConfig;
use std::sync::Arc;

fn state() -> HttpState {
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    HttpState::for_test(HealthCoordinator::new(), metrics, false, None)
}

fn request() -> Request {
    Request::new(Body::empty())
}

#[tokio::test]
async fn direct_vendor_handlers_and_platform_errors_fail_closed() {
    let state = state();
    let responses = [
        capabilities(State(state.clone()), request()).await,
        system_status(State(state.clone()), request()).await,
        scheduler_status(State(state.clone()), request()).await,
        scheduler_pause(State(state.clone()), request()).await,
        scheduler_resume(State(state.clone()), request()).await,
        scheduler_repair(State(state.clone()), request()).await,
        cache_status(State(state.clone()), request()).await,
        cache_garbage_collection(State(state.clone()), request()).await,
        image_capacity(State(state.clone()), request()).await,
        worker_endpoints(
            State(state.clone()),
            Path(("account".to_owned(), "worker".to_owned())),
            request(),
        )
        .await,
        durable_object_namespaces(State(state.clone()), Path("account".to_owned()), request())
            .await,
        durable_object_records(
            State(state),
            Path(("account".to_owned(), "namespace".to_owned())),
            request(),
        )
        .await,
    ];
    for response in responses {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let request_id = RequestId::generate();
    let response = platform_error(
        &PlatformError::new(ErrorCode::Internal, "private detail"),
        V4RequestContext {
            role: V4Role::ReadOnly,
            request_id,
        },
    );
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response
            .headers()
            .get(crate::http::REQUEST_ID_HEADER)
            .unwrap(),
        request_id.to_string().as_str()
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!body.contains("private detail"));
}
