use super::*;
use crate::cloudflare_v4::wire::V4Role;
use crate::health::HealthCoordinator;
use crate::metrics::MetricsRegistry;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use open_compute_core::RequestId;
use open_compute_core::SecretString;
use open_compute_core::config::MetricsConfig;
use std::sync::Arc;
use tower::ServiceExt as _;

fn state() -> HttpState {
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("admin-token")),
    )
    .with_v4_tokens(
        SecretString::new("deployer-token"),
        SecretString::new("read-token"),
    )
}

fn app(state: HttpState) -> Router {
    crate::cloudflare_v4::router(state.clone(), Router::new()).with_state(state)
}

fn request() -> Request {
    Request::new(Body::empty())
}

fn authed(uri: &str) -> Request {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer read-token")
        .body(Body::empty())
        .unwrap()
}

fn admin(method: Method, uri: &str, body: Body) -> Request {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer admin-token")
        .body(body)
        .unwrap()
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

#[tokio::test]
async fn authenticated_vendor_status_and_upgrade_surfaces() {
    let state = state();
    let router = app(state);

    for uri in [
        "/open-compute/capabilities",
        "/open-compute/system/status",
        "/open-compute/upgrade/check",
    ] {
        let response = router.clone().oneshot(authed(uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
    }

    for uri in [
        "/open-compute/scheduler",
        "/open-compute/cache",
        "/open-compute/images/capacity",
    ] {
        let response = router.clone().oneshot(authed(uri)).await.unwrap();
        assert!(
            response.status() == StatusCode::OK
                || response.status().is_client_error()
                || response.status().is_server_error(),
            "{uri} {}",
            response.status()
        );
    }

    let forbidden = Request::builder()
        .method(Method::POST)
        .uri("/open-compute/upgrade")
        .header(header::AUTHORIZATION, "Bearer read-token")
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(forbidden).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = router
        .clone()
        .oneshot(admin(
            Method::POST,
            "/open-compute/scheduler/pause",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert!(
        response.status().is_client_error() || response.status().is_server_error(),
        "{}",
        response.status()
    );

    let response = router
        .clone()
        .oneshot(admin(
            Method::POST,
            "/open-compute/scheduler/resume",
            Body::from("x"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    for uri in [
        "/open-compute/scheduler/repair",
        "/open-compute/cache/garbage-collection",
    ] {
        let response = router
            .clone()
            .oneshot(admin(Method::POST, uri, Body::empty()))
            .await
            .unwrap();
        assert!(
            response.status().is_client_error() || response.status().is_server_error(),
            "{uri} {}",
            response.status()
        );
    }

    let response = router
        .clone()
        .oneshot(admin(
            Method::POST,
            "/open-compute/upgrade",
            Body::from(r#"{"version":"0.2.0"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    for uri in [
        "/accounts/account/open-compute/workers/worker/endpoints",
        "/accounts/account/open-compute/durable-objects",
        "/accounts/account/open-compute/durable-objects/namespace/objects",
    ] {
        let response = router.clone().oneshot(authed(uri)).await.unwrap();
        assert!(
            response.status().is_client_error() || response.status().is_server_error(),
            "{uri} {}",
            response.status()
        );
    }

    let bad = Request::builder()
        .uri("/open-compute/capabilities?q=1")
        .header(header::AUTHORIZATION, "Bearer read-token")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(bad).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn vendor_backup_routes_fail_closed_without_storage() {
    let state = state();
    let router = app(state);
    for uri in [
        "/accounts/account/open-compute/kv/namespaces/ns/backups",
        "/accounts/account/open-compute/d1/databases/db/backups",
    ] {
        let response = router.clone().oneshot(authed(uri)).await.unwrap();
        assert!(
            response.status().is_client_error() || response.status().is_server_error(),
            "{uri} {}",
            response.status()
        );
    }
    let create = Request::builder()
        .method(Method::POST)
        .uri("/accounts/account/open-compute/kv/namespaces/ns/backups")
        .header(header::AUTHORIZATION, "Bearer admin-token")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(create).await.unwrap();
    assert!(
        response.status().is_client_error() || response.status().is_server_error(),
        "{}",
        response.status()
    );
}
