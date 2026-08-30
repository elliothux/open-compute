use super::*;
use axum::body::to_bytes;
use open_compute_core::config::{MetricsConfig, SecretReference};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;
use tower::ServiceExt;

fn metrics() -> Arc<MetricsRegistry> {
    Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap())
}

#[tokio::test]
async fn metrics_auth_state_conversion_and_bounded_route_labels_are_covered() {
    let snapshot = SupervisorSnapshot {
        state: SupervisorState::Running,
        reason: ReadinessReason::Ready,
        last_transition_at: SystemTime::UNIX_EPOCH,
        attempt: 3,
        last_exit: None,
        next_retry_at: None,
        pid: Some(1),
        pgid: Some(1),
        binary_digest: "digest".to_owned(),
        config_digest: "config".to_owned(),
        startup_id: None,
        token_fingerprint: None,
        listen_port: Some(8080),
    };
    let sanitized = SanitizedSupervisor::from(&snapshot);
    assert_eq!(sanitized.state, SupervisorState::Running);
    assert_eq!(sanitized.reason, ReadinessReason::Ready);
    assert_eq!(sanitized.attempt, 3);

    for (path, expected) in [
        ("/health/live", "/health/live"),
        ("/health/ready", "/health/ready"),
        ("/health/status", "/health/status"),
        ("/metrics", "/metrics"),
        ("/v1/accounts/a/workers", "/v1/accounts/:account/workers/*"),
        ("/__workers/a/w", "/__workers/:account/:worker/*"),
        ("/tenant-controlled", "/other"),
    ] {
        assert_eq!(bound_route(path), expected);
    }
    assert_eq!(
        product_operation("/v1/accounts/a/kv/namespaces"),
        Some(OperationClass::Kv)
    );
    assert_eq!(product_operation("/__workers/a/w"), None);

    let health = HealthCoordinator::new();
    let open_state = HttpState::for_test(health.clone(), metrics(), true, None);
    let response = admin_router(open_state)
        .oneshot(
            Request::builder()
                .uri("/metrics")
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
            "/v1/accounts/a/kv/namespaces",
            axum::routing::post(|| async {
                let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
                response
                    .extensions_mut()
                    .insert(ProductErrorCode(ErrorCode::QuotaExceeded));
                response
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            middleware_state,
            bounds_middleware,
        ))
        .with_state(state);
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/accounts/a/kv/namespaces")
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
    fs::write(&secret, b"admin-secret").unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let server = ServerConfig {
        admin_auth: Some(SecretReference {
            env: None,
            file: Some(secret),
        }),
        ..ServerConfig::default()
    };
    let state = HttpState::new(
        HealthCoordinator::new(),
        metrics(),
        true,
        &server,
        Arc::new(|| None),
    )
    .unwrap();
    let debug = format!("{state:?}");
    assert!(debug.contains("admin_auth: true"));
    assert!(!debug.contains("admin-secret"));
}
