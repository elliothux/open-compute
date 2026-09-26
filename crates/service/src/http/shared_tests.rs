use super::*;
use crate::health::HealthCoordinator;
use crate::instance_registry::{InstanceRecord, RegisteredObjectAuthority, ServiceScope};
use crate::metrics::MetricsRegistry;
use crate::run::daemon_control::{DaemonApi, RegisteredTokens};
use axum::body::Body;
use axum::http::Request;
use open_compute_core::MetricsConfig;

fn state(deployer: &str) -> HttpState {
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "test").unwrap());
    let mut state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("global-admin")),
    );
    state.deployer_secret = Some(Arc::new(SecretString::new(deployer)));
    state.read_only_secret = Some(Arc::new(SecretString::new(format!("{deployer}-read"))));
    state
}

#[test]
fn dispatch_identity_is_exact_and_host_first() {
    let id = InstanceId::generate();
    assert_eq!(
        local_origin_instance(&format!("app.{id}.localhost")),
        Some((id, true))
    );
    assert_eq!(
        local_origin_instance(&format!("{id}.localhost")),
        Some((id, false))
    );
    assert_eq!(
        account_path_instance(&format!("/client/v4/accounts/{id}/workers")),
        Some(id)
    );
    assert_eq!(
        git_path_instance(&format!("/git/{id}/apps/site.git/info/refs")),
        Some(id)
    );
    for resource in ["tails", "live-tails"] {
        assert_eq!(
            signed_tail_path_instance(&format!(
                "/client/v4/open-compute/{resource}/{id}/session/ticket"
            )),
            Some(id)
        );
    }
    for host in [
        "app.unknown.localhost",
        "app.id.other.localhost",
        "unknown.localhost",
        "localhost",
    ] {
        assert!(local_origin_instance(host).is_none());
    }
    assert!(account_path_instance("/client/v4/accounts/").is_none());
    assert!(git_path_instance("/git/not-an-instance/apps/site.git").is_none());
    assert!(signed_tail_path_instance("/client/v4/open-compute/tails/bad/id/ticket").is_none());
    assert!(signed_tail_path_instance("/client/v4/open-compute/other/bad/id/ticket").is_none());
}

#[test]
fn shared_host_accepts_only_the_active_listener_port_and_exact_loopback() {
    assert_eq!(
        shared_request_host("127.0.0.1:9100", 9100).as_deref(),
        Some("127.0.0.1")
    );
    assert_eq!(
        shared_request_host("localhost:9100", 9100).as_deref(),
        Some("localhost")
    );
    assert_eq!(
        shared_request_host("app.abc.localhost:9100", 9100).as_deref(),
        Some("app.abc.localhost")
    );
    assert!(shared_request_host("127.0.0.1:8787", 9100).is_none());
    assert!(shared_request_host("127.0.0.2:9100", 9100).is_none());
    assert!(shared_request_host("APP.abc.localhost:9100", 9100).is_none());
}

#[test]
fn registration_rejects_duplicate_tokens_and_old_lease_cannot_remove_new_generation() {
    let routes = SharedRoutes::new(None, 1024);
    let first_id = InstanceId::generate();
    let second_id = InstanceId::generate();
    let first = routes.insert(first_id, state("first"), None).unwrap();
    assert!(matches!(
        routes.insert(second_id, state("first"), None),
        Err(error) if error.code() == ErrorCode::SecretRefInvalid
    ));
    let second = routes.insert(second_id, state("second"), None).unwrap();
    assert_eq!(routes.inner.read().unwrap().len(), 2);
    first.withdraw();
    let replacement = routes.insert(first_id, state("first"), None).unwrap();
    drop(first);
    assert!(routes.inner.read().unwrap().contains_key(&first_id));
    drop(replacement);
    drop(second);
    assert!(routes.inner.read().unwrap().is_empty());
}

#[test]
fn private_gateway_routes_only_to_the_matching_running_instance() {
    let routes = SharedRoutes::new(None, 1024);
    let first_id = InstanceId::generate();
    let second_id = InstanceId::generate();
    let first = routes
        .insert(first_id, state("first"), Some("a.example.com"))
        .unwrap();
    assert!(
        routes
            .insert(
                InstanceId::generate(),
                state("overlap"),
                Some("nested.a.example.com"),
            )
            .is_err()
    );
    let second = routes
        .insert(second_id, state("second"), Some("b.example.net"))
        .unwrap();
    assert_eq!(
        routes
            .gateway_route("app.a.example.com")
            .unwrap()
            .unwrap()
            .0,
        first_id
    );
    assert_eq!(
        routes
            .gateway_route("bucket.r2.b.example.net")
            .unwrap()
            .unwrap()
            .0,
        second_id
    );
    for host in [
        "a.example.com",
        "app.a.example.com.evil.net",
        "app.c.example.org",
    ] {
        assert!(routes.gateway_route(host).unwrap().is_none());
    }
    first.withdraw();
    assert!(routes.gateway_route("app.a.example.com").unwrap().is_none());
    assert!(routes.gateway_route("app.b.example.net").unwrap().is_some());
    drop(second);
    assert!(routes.gateway_route("app.b.example.net").unwrap().is_none());
}

#[tokio::test]
async fn daemon_readiness_does_not_follow_instance_health() {
    let routes = SharedRoutes::new(None, 1024);
    let router = routes.router(true, 9100);
    let ready = || {
        Request::builder()
            .uri("/health/ready")
            .header(header::HOST, "localhost:9100")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        router.clone().oneshot(ready()).await.unwrap().status(),
        StatusCode::OK
    );
    let lease = routes
        .insert(InstanceId::generate(), state("deployer"), None)
        .unwrap();
    assert_eq!(
        router.clone().oneshot(ready()).await.unwrap().status(),
        StatusCode::OK
    );
    lease.withdraw();
    assert_eq!(
        router.oneshot(ready()).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn metric_series_are_instance_labelled_and_share_one_capacity() {
    let first_id = InstanceId::generate();
    let second_id = InstanceId::generate();
    let routes = SharedRoutes::new(None, crate::metrics::REQUIRED_SERIES);
    let mut first_state = state("first");
    first_state.metrics_enabled = true;
    let mut second_state = state("second");
    second_state.metrics_enabled = true;
    let first = routes.insert(first_id, first_state, None).unwrap();
    let _second = routes.insert(second_id, second_state, None).unwrap();
    let router = routes.router(true, 9100);
    async fn scrape(router: &Router, id: InstanceId) -> Response {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .header(header::HOST, format!("{id}.localhost:9100"))
                    .header(header::AUTHORIZATION, "Bearer global-admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    let response = scrape(&router, first_id).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains(&format!("instance_id=\"{first_id}\"")));
    assert_eq!(
        text.lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .count() as u64,
        crate::metrics::REQUIRED_SERIES
    );
    assert_eq!(
        scrape(&router, second_id).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    first.withdraw();
    assert_eq!(scrape(&router, second_id).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn daemon_discovery_requires_global_admin_and_loopback_management_host() {
    let (daemon, _receiver) =
        DaemonApi::channel(&[], Vec::new(), SecretString::new("global-admin")).unwrap();
    let router = SharedRoutes::new(Some(daemon), 1024).router(true, 9100);
    for (host, token, expected) in [
        ("localhost:9100", "global-admin", StatusCode::OK),
        ("localhost:9100", "wrong-token", StatusCode::UNAUTHORIZED),
        (
            "app.unknown.localhost:9100",
            "global-admin",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/client/v4/accounts")
                    .header(header::HOST, host)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/client/v4/accounts")
                .header(header::HOST, "localhost:9100")
                .header(header::AUTHORIZATION, "Bearer global-admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn single_instance_dashboard_shell_loads_before_session_authentication() {
    let id = InstanceId::generate();
    let record = InstanceRecord {
        instance_id: id.to_string(),
        name: Some("dashboard".to_owned()),
        canonical_config_path: "/unused/dashboard.toml".to_owned(),
        config_sha256: String::new(),
        data_path: "/unused/dashboard".to_owned(),
        object_authority: RegisteredObjectAuthority::Local,
        public_base_domain: None,
        service_scope: ServiceScope::User,
        created_at: 0,
        autostart: true,
    };
    let (daemon, _receiver) = DaemonApi::channel(
        std::slice::from_ref(&record),
        vec![RegisteredTokens {
            instance_id: id,
            deployer: SecretString::new("dashboard"),
            read_only: SecretString::new("dashboard-read"),
        }],
        SecretString::new("global-admin"),
    )
    .unwrap();
    let routes = SharedRoutes::new(Some(daemon), 1024);
    let _lease = routes
        .insert(id, state("dashboard").with_dashboard_enabled(true), None)
        .unwrap();
    let router = routes.router(true, 9100);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .header(header::HOST, "127.0.0.1:9100")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("dashboard is not ready")
    );
    for (method, path, expected) in [
        (Method::POST, "/operator/", StatusCode::NOT_FOUND),
        (
            Method::GET,
            "/operator/api/instances",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::HOST, "127.0.0.1:9100")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let response = routes
        .router(false, 9100)
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .header(header::HOST, "127.0.0.1:9100")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let second_id = InstanceId::generate();
    let second_record = InstanceRecord {
        instance_id: second_id.to_string(),
        name: Some("second".to_owned()),
        canonical_config_path: "/unused/second.toml".to_owned(),
        config_sha256: String::new(),
        data_path: "/unused/second".to_owned(),
        object_authority: RegisteredObjectAuthority::Local,
        public_base_domain: None,
        service_scope: ServiceScope::User,
        created_at: 0,
        autostart: true,
    };
    let (daemon, _receiver) = DaemonApi::channel(
        &[record, second_record],
        vec![
            RegisteredTokens {
                instance_id: id,
                deployer: SecretString::new("dashboard"),
                read_only: SecretString::new("dashboard-read"),
            },
            RegisteredTokens {
                instance_id: second_id,
                deployer: SecretString::new("second"),
                read_only: SecretString::new("second-read"),
            },
        ],
        SecretString::new("global-admin"),
    )
    .unwrap();
    let routes = SharedRoutes::new(Some(daemon), 1024);
    let _first = routes
        .insert(id, state("dashboard").with_dashboard_enabled(true), None)
        .unwrap();
    let _second = routes
        .insert(
            second_id,
            state("second").with_dashboard_enabled(true),
            None,
        )
        .unwrap();
    let response = routes
        .router(true, 9100)
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .header(header::HOST, "127.0.0.1:9100")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboard_session_is_shared_across_registered_instances() {
    let first_id = InstanceId::generate();
    let second_id = InstanceId::generate();
    let records = [
        InstanceRecord {
            instance_id: first_id.to_string(),
            name: Some("first".to_owned()),
            canonical_config_path: "/unused/first.toml".to_owned(),
            config_sha256: String::new(),
            data_path: "/unused/first".to_owned(),
            object_authority: RegisteredObjectAuthority::Local,
            public_base_domain: None,
            service_scope: ServiceScope::User,
            created_at: 0,
            autostart: true,
        },
        InstanceRecord {
            instance_id: second_id.to_string(),
            name: Some("second".to_owned()),
            canonical_config_path: "/unused/second.toml".to_owned(),
            config_sha256: String::new(),
            data_path: "/unused/second".to_owned(),
            object_authority: RegisteredObjectAuthority::Local,
            public_base_domain: None,
            service_scope: ServiceScope::User,
            created_at: 0,
            autostart: true,
        },
    ];
    let (daemon, _receiver) = DaemonApi::channel(
        &records,
        vec![
            RegisteredTokens {
                instance_id: first_id,
                deployer: SecretString::new("first"),
                read_only: SecretString::new("first-read"),
            },
            RegisteredTokens {
                instance_id: second_id,
                deployer: SecretString::new("second"),
                read_only: SecretString::new("second-read"),
            },
        ],
        SecretString::new("global-admin"),
    )
    .unwrap();
    let routes = SharedRoutes::new(Some(daemon), 1024);
    let auth = routes.dashboard_auth();
    let _first = routes
        .insert(
            first_id,
            state("first").with_dashboard_auth(auth.clone()),
            None,
        )
        .unwrap();
    let _second = routes
        .insert(second_id, state("second").with_dashboard_auth(auth), None)
        .unwrap();
    let router = routes.router(true, 9100);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/operator/session")
                .header(header::HOST, "localhost:9100")
                .header(header::AUTHORIZATION, "Bearer global-admin")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session = payload["session_token"].as_str().unwrap();

    let response = router
        .oneshot(
            Request::builder()
                .uri("/client/v4/accounts")
                .header(header::HOST, "localhost:9100")
                .header(header::AUTHORIZATION, format!("Bearer {session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains(first_id.as_str()));
    assert!(text.contains(second_id.as_str()));
}

#[tokio::test]
async fn stopped_instance_keeps_only_its_registered_discovery_scope() {
    let id = InstanceId::generate();
    let record = InstanceRecord {
        instance_id: id.to_string(),
        name: Some("dev".to_owned()),
        canonical_config_path: "/unused/compute.toml".to_owned(),
        config_sha256: String::new(),
        data_path: "/unused/data".to_owned(),
        object_authority: RegisteredObjectAuthority::Local,
        public_base_domain: Some("a.example.com".to_owned()),
        service_scope: ServiceScope::User,
        created_at: 0,
        autostart: false,
    };
    let (daemon, _receiver) = DaemonApi::channel(
        &[record],
        vec![RegisteredTokens {
            instance_id: id,
            deployer: SecretString::new("deployer"),
            read_only: SecretString::new("deployer-read"),
        }],
        SecretString::new("global-admin"),
    )
    .unwrap();
    let routes = SharedRoutes::new(Some(daemon.clone()), 1024);
    assert!(matches!(
        routes.insert(id, state("wrong"), None),
        Err(error) if error.code() == ErrorCode::SecretRefInvalid
    ));
    assert!(matches!(
        routes.insert(id, state("deployer"), Some("b.example.net")),
        Err(error) if error.code() == ErrorCode::SecretRefInvalid
    ));
    let lease = routes
        .insert(id, state("deployer"), Some("a.example.com"))
        .unwrap();
    lease.withdraw();
    daemon.mark(&id, "stopped", None).unwrap();
    let router = routes.router(true, 9100);
    for (token, expected) in [
        ("deployer", StatusCode::OK),
        ("deployer-read", StatusCode::OK),
        ("wrong", StatusCode::UNAUTHORIZED),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/client/v4/accounts")
                    .header(header::HOST, "localhost:9100")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    daemon.remove(&id).unwrap();
    assert!(
        daemon
            .visible_for_bearer(Some("Bearer deployer"))
            .unwrap()
            .is_none()
    );
}
