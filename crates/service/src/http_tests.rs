use super::*;
use axum::body::to_bytes;
use axum::middleware;
use open_compute_core::config::{MetricsConfig, SecretReference};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

fn metrics() -> Arc<MetricsRegistry> {
    Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap())
}

#[tokio::test]
async fn metrics_auth_state_conversion_and_bounded_route_labels_are_covered() {
    for (path, expected) in [
        ("/health/live", "/health/live"),
        ("/health/ready", "/health/ready"),
        ("/metrics", "/metrics"),
        (
            "/client/v4/open-compute/system/status",
            "/client/v4/open-compute/*",
        ),
        (
            "/client/v4/accounts/a/workers/scripts",
            "/client/v4/accounts/:account/*",
        ),
        ("/__workers/a/w", "/__workers/:account/:worker/*"),
        ("/tenant-controlled", "/other"),
    ] {
        assert_eq!(bound_route(path), expected);
    }
    assert_eq!(
        product_operation("/client/v4/accounts/a/storage/kv/namespaces"),
        Some(OperationClass::Kv)
    );
    assert_eq!(product_operation("/__workers/a/w"), None);

    let health = HealthCoordinator::new();
    let authenticated = HttpState::for_test(
        health.clone(),
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    );
    let response = admin_router(authenticated)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header(header::AUTHORIZATION, "Bearer admin-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        CONTENT_TYPE
    );
    assert!(
        !to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .is_empty()
    );

    let protected = HttpState::for_test(
        health,
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    );
    let response = admin_router(protected)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn product_error_extension_updates_admission_metrics_without_tenant_labels() {
    let registry = metrics();
    let state = HttpState::for_test(HealthCoordinator::new(), registry.clone(), true, None);
    let middleware_state = state.clone();
    let router = Router::new()
        .route(
            "/client/v4/accounts/a/storage/kv/namespaces",
            axum::routing::post(|| async {
                let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
                response
                    .extensions_mut()
                    .insert(ProductErrorCode(ErrorCode::QuotaExceeded));
                response
            }),
        )
        .layer(middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state);
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/client/v4/accounts/a/storage/kv/namespaces")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let rendered = registry.render(&HealthCoordinator::new().snapshot());
    assert!(rendered.contains("platform_admission_total{operation=\"kv\",outcome=\"quota\"} 1"));
    assert!(rendered.contains("platform_quota_reject_total{product=\"kv\"} 1"));
}

#[tokio::test]
async fn test_support_runtime_restart_requires_auth_ack_and_an_attached_hook() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    )
    .with_test_runtime_restart(Arc::new(move || {
        hook_calls.fetch_add(1, Ordering::Relaxed);
        true
    }));
    let router = admin_router(state);
    let request = |auth: bool, acknowledge: bool| {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/__test/runtime/restart");
        if auth {
            request = request.header(header::AUTHORIZATION, "Bearer admin-secret");
        }
        if acknowledge {
            request = request.header("x-open-compute-test-ack", "restart-generation");
        }
        request.body(Body::empty()).unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(request(false, true))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(true, false))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        router.oneshot(request(true, true)).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn state_constructor_resolves_file_auth_and_debug_is_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("admin.secret");
    let deployer = dir.path().join("deployer.secret");
    let read_only = dir.path().join("read-only.secret");
    fs::write(&secret, b"admin-secret").unwrap();
    fs::write(&deployer, b"deployer-secret").unwrap();
    fs::write(&read_only, b"read-only-secret").unwrap();
    for path in [&secret, &deployer, &read_only] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let server = ServerConfig {
        admin_auth: SecretReference {
            env: None,
            file: Some(secret),
        },
        deployer_auth: SecretReference {
            env: None,
            file: Some(deployer),
        },
        read_only_auth: SecretReference {
            env: None,
            file: Some(read_only),
        },
        ..ServerConfig::default()
    };
    let state = HttpState::new(HealthCoordinator::new(), metrics(), true, false, &server).unwrap();
    let debug = format!("{state:?}");
    assert!(debug.contains("admin_auth: true"));
    assert!(!debug.contains("admin-secret"));
}

#[tokio::test]
async fn dashboard_surface_is_disabled_by_default() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    );
    let response = admin_router(state)
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboard_surface_returns_not_ready_when_enabled_without_bootstrap() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    )
    .with_dashboard_enabled(true);
    let response = admin_router(state)
        .oneshot(
            Request::builder()
                .uri("/operator/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "platform_unavailable");
}

#[tokio::test]
async fn dashboard_surface_trailing_slash_hits_handler_when_enabled_without_bootstrap() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    )
    .with_dashboard_enabled(true);
    let response = admin_router(state)
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "/operator/ must reach operator_surface, not the 404 fallback"
    );
}

#[test]
fn dashboard_asset_path_strips_operator_prefix() {
    assert_eq!(dashboard_asset_path("/operator/"), "/");
    assert_eq!(dashboard_asset_path("/operator"), "/");
    assert_eq!(
        dashboard_asset_path("/operator/assets/index.js"),
        "/assets/index.js"
    );
    assert_eq!(dashboard_asset_path("/operator/login"), "/login");
}

#[tokio::test]
async fn legacy_operator_paths_return_not_found() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    );
    for path in [
        "/v1/account",
        "/v1/accounts/a/workers",
        "/health/status",
        "/operator/metrics",
    ] {
        let response = admin_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, "Bearer admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "path={path}");
    }
}

#[tokio::test]
async fn metrics_requires_canonical_unauthorized_envelope() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        true,
        Some(SecretString::new("admin-secret")),
    );
    let response = admin_router(state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "ADMIN_AUTH_REQUIRED");
}

#[tokio::test]
async fn system_status_remains_available_while_starting() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    )
    .with_v4_tokens(
        SecretString::new("deployer-secret"),
        SecretString::new("read-secret"),
    );
    let response = admin_router(state)
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/system/status")
                .header(header::AUTHORIZATION, "Bearer admin-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["success"], true);
    assert_eq!(json["result"]["state"], "STARTING");
}

#[tokio::test]
async fn runtime_operator_control_returns_unavailable_without_supervisor() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    );
    let response = admin_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/__test/runtime/restart")
                .header(header::AUTHORIZATION, "Bearer admin-secret")
                .header("x-open-compute-test-ack", "restart-generation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn public_listener_does_not_expose_operator_api() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    );
    for path in ["/operator/", "/operator/api/v1/meta", "/operator/metrics"] {
        let response = public_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, "Bearer admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "public listener must not serve operator path {path}"
        );
    }
}

#[tokio::test]
async fn merged_listener_neutrally_rejects_removed_operator_api() {
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    );
    let response = merged_router(state)
        .oneshot(
            Request::builder()
                .uri("/operator/api/v1/meta")
                .header(header::AUTHORIZATION, "Bearer admin-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
