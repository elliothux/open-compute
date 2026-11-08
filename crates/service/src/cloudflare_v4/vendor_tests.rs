use super::worker_origins::{
    delete_worker_public_origin, project_worker_endpoint, set_worker_public_origin,
    worker_endpoints, worker_public_origin,
};
use super::*;
use crate::cloudflare_v4::accounts::V4InstanceContext;
use crate::cloudflare_v4::wire::V4Role;
use crate::health::HealthCoordinator;
use crate::metrics::MetricsRegistry;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{DataConfig, MetricsConfig};
use open_compute_core::{InstanceId, RequestId, SecretString, WorkerId};
use open_compute_storage::PlatformStorage;
use open_compute_storage::public_gateway::PublicGatewayRepository;
use open_compute_storage::worker_repository::{
    RouteRecord, WorkerOriginExposure, WorkerOriginTransport, WorkerRepository,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
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

#[test]
fn endpoints_are_projected_per_trusted_ingress() {
    let pid = Arc::new(AtomicI32::new(0));
    let qualified = Arc::new(AtomicI32::new(0));
    let state = state()
        .with_local_origin_addr("127.0.0.1:8787".parse().unwrap())
        .with_public_gateway_process(pid.clone(), qualified.clone());
    let route = RouteRecord {
        id: "route".to_owned(),
        instance_id: InstanceId::generate(),
        worker_id: WorkerId::generate(),
        hostname_ascii: "worker.example.com".to_owned(),
        exposure: WorkerOriginExposure::Public,
        transport: WorkerOriginTransport::ManagedGateway,
        path_prefix: "/".to_owned(),
        entrypoint: None,
        generation: 1,
        created_at_ms: 1_000,
    };
    assert!(project_worker_endpoint(route.clone(), &state).is_none());
    pid.store(42, Ordering::Release);
    assert!(project_worker_endpoint(route.clone(), &state).is_none());
    qualified.store(41, Ordering::Release);
    assert!(project_worker_endpoint(route.clone(), &state).is_none());
    qualified.store(42, Ordering::Release);
    let public = project_worker_endpoint(route.clone(), &state)
        .unwrap()
        .unwrap();
    let public = serde_json::to_value(public).unwrap();
    assert_eq!(public["kind"], "public_origin");
    assert_eq!(public["scope"], "public_network");
    assert_eq!(public["url"], "https://worker.example.com/");
    assert_eq!(public["transport"], "managed_gateway");
    pid.store(43, Ordering::Release);
    assert!(project_worker_endpoint(route.clone(), &state).is_none());

    let external = RouteRecord {
        transport: WorkerOriginTransport::ExternalProxy {
            scheme: open_compute_core::ExternalScheme::Https,
            port: 8443,
        },
        ..route.clone()
    };
    assert!(project_worker_endpoint(external.clone(), &state).is_none());
    let bound = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let external_state = state
        .clone()
        .with_external_ingress(vec!["worker.example.com".to_owned()], Some(bound.clone()));
    let endpoint = project_worker_endpoint(external.clone(), &external_state)
        .unwrap()
        .unwrap();
    let endpoint = serde_json::to_value(endpoint).unwrap();
    assert_eq!(endpoint["kind"], "public_origin");
    assert_eq!(endpoint["transport"], "external_proxy");
    assert_eq!(endpoint["url"], "https://worker.example.com:8443/");
    bound.store(false, Ordering::Release);
    assert!(project_worker_endpoint(external.clone(), &external_state).is_none());
    let unauthorized = state.clone().with_external_ingress(
        vec!["other.example.com".to_owned()],
        Some(Arc::new(std::sync::atomic::AtomicBool::new(true))),
    );
    assert!(project_worker_endpoint(external, &unauthorized).is_none());

    let local = project_worker_endpoint(
        RouteRecord {
            exposure: WorkerOriginExposure::Local,
            transport: WorkerOriginTransport::LocalHttp,
            ..route
        },
        &state,
    )
    .unwrap()
    .unwrap();
    let local = serde_json::to_value(local).unwrap();
    assert_eq!(local["kind"], "local_origin");
    assert_eq!(local["scope"], "local_machine");
    assert_eq!(local["url"], "http://worker.example.com:8787/");
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
        worker_public_origin(
            State(state.clone()),
            Path(("account".to_owned(), "worker".to_owned())),
            request(),
        )
        .await,
        set_worker_public_origin(
            State(state.clone()),
            Path(("account".to_owned(), "worker".to_owned())),
            request(),
        )
        .await,
        delete_worker_public_origin(
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
async fn public_origin_mutation_requires_write_role() {
    let router = app(state());
    let path = "/accounts/account/open-compute/workers/worker/public-origin";
    for method in [Method::PUT, Method::DELETE] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, "Bearer read-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"transport":"managed_gateway","name":"worker"}"#,
            ))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn public_origin_api_preserves_local_entrypoint() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().instance_id;
    WorkerRepository::new(storage.db())
        .create_worker(account, "app", RequestId::generate(), 1, 100)
        .unwrap();
    PublicGatewayRepository::new(storage.db())
        .provision("compute.example.com", 2)
        .unwrap();
    PublicGatewayRepository::new(storage.db())
        .activate_workers("compute.example.com", 3)
        .unwrap();
    let authority = V4InstanceContext::new(
        storage.identity().instance_id,
        storage.identity().created_at_ms,
    );
    let path = format!(
        "/accounts/{}/open-compute/workers/app/public-origin",
        authority.public_id(),
    );
    let endpoints_path = format!(
        "/accounts/{}/open-compute/workers/app/endpoints",
        authority.public_id(),
    );
    let pid = Arc::new(AtomicI32::new(42));
    let qualified = Arc::new(AtomicI32::new(0));
    let router = app(state()
        .with_v4_instance_context(authority)
        .with_platform_storage(storage)
        .with_local_origin_addr("127.0.0.1:8787".parse().unwrap())
        .with_public_gateway_process(pid.clone(), qualified.clone()));
    let request = Request::builder()
        .method(Method::PUT)
        .uri(&path)
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"transport":"managed_gateway","name":"app"}"#,
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    qualified.store(42, Ordering::Release);
    let request = Request::builder()
        .method(Method::PUT)
        .uri(&path)
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"transport":"managed_gateway","name":"app"}"#,
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["result"]["origin"], "https://app.compute.example.com");
    assert_eq!(body["result"]["transport"], "managed_gateway");
    assert_eq!(body["result"]["ingress_available"], true);
    let response = router
        .clone()
        .oneshot(authed(&endpoints_path))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["result"].as_array().unwrap().len(), 2);
    qualified.store(0, Ordering::Release);
    let request = Request::builder()
        .method(Method::PUT)
        .uri(&path)
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"transport":"managed_gateway","name":"renamed"}"#,
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response = router
        .clone()
        .oneshot(authed(&endpoints_path))
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["result"].as_array().unwrap().len(), 1);
    assert_eq!(body["result"][0]["kind"], "local_origin");
    let response = router.clone().oneshot(authed(&path)).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["result"]["name"], "app");
    assert_eq!(body["result"]["origin"], "https://app.compute.example.com");
    assert_eq!(body["result"]["ingress_available"], false);
    let response = router
        .clone()
        .oneshot(admin(Method::DELETE, &path, Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    qualified.store(42, Ordering::Release);
    let response = router.oneshot(authed(&endpoints_path)).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["result"].as_array().unwrap().len(), 1);
}

fn put_origin(path: &str, token: &str, body: &'static str) -> Request {
    Request::builder()
        .method(Method::PUT)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn json_body(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

#[tokio::test]
async fn external_public_origin_requires_authorization_not_gateway() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().instance_id;
    let workers = WorkerRepository::new(storage.db());
    for name in ["app", "other"] {
        workers
            .create_worker(account, name, RequestId::generate(), 1, 100)
            .unwrap();
    }
    let authority = V4InstanceContext::new(
        storage.identity().instance_id,
        storage.identity().created_at_ms,
    );
    let path = format!(
        "/accounts/{}/open-compute/workers/app/public-origin",
        authority.public_id(),
    );
    let other_path = format!(
        "/accounts/{}/open-compute/workers/other/public-origin",
        authority.public_id(),
    );
    let endpoints_path = format!(
        "/accounts/{}/open-compute/workers/app/endpoints",
        authority.public_id(),
    );
    let bound = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let router = app(state()
        .with_v4_instance_context(authority)
        .with_platform_storage(storage)
        .with_local_origin_addr("127.0.0.1:8787".parse().unwrap())
        .with_external_ingress(
            vec!["example.com".to_owned(), "app.example.com".to_owned()],
            Some(bound.clone()),
        ));

    for (token, body, status) in [
        (
            "read-token",
            r#"{"transport":"external_proxy","origin":"https://example.com"}"#,
            StatusCode::FORBIDDEN,
        ),
        (
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"https://unlisted.example.com"}"#,
            StatusCode::FORBIDDEN,
        ),
        (
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"https://example.com/app"}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            "deployer-token",
            r#"{"transport":"external_proxy","name":"app"}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"https://example.com","name":"app"}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            "deployer-token",
            r#"{"name":"app"}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            "deployer-token",
            r#"{"transport":"managed_gateway","name":"app"}"#,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(put_origin(&path, token, body))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{body}");
    }

    let response = router
        .clone()
        .oneshot(put_origin(
            &path,
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"https://example.com:443/"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first = json_body(response).await;
    assert_eq!(first["result"]["transport"], "external_proxy");
    assert_eq!(first["result"]["origin"], "https://example.com");
    assert_eq!(first["result"]["ingress_available"], false);
    assert!(first["result"].get("name").is_none());
    let response = router
        .clone()
        .oneshot(put_origin(
            &path,
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"https://example.com"}"#,
        ))
        .await
        .unwrap();
    let repeated = json_body(response).await;
    assert_eq!(repeated["result"]["id"], first["result"]["id"]);
    assert_eq!(
        repeated["result"]["created_on"],
        first["result"]["created_on"]
    );
    let response = router
        .clone()
        .oneshot(put_origin(
            &other_path,
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"http://example.com:8080"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let endpoints = json_body(
        router
            .clone()
            .oneshot(authed(&endpoints_path))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(endpoints["result"].as_array().unwrap().len(), 1);
    bound.store(true, Ordering::Release);
    let endpoints = json_body(
        router
            .clone()
            .oneshot(authed(&endpoints_path))
            .await
            .unwrap(),
    )
    .await;
    let endpoints = endpoints["result"].as_array().unwrap();
    assert_eq!(endpoints.len(), 2);
    let external = endpoints
        .iter()
        .find(|endpoint| endpoint["transport"] == "external_proxy")
        .unwrap();
    assert_eq!(external["url"], "https://example.com/");
    assert_eq!(external["scope"], "public_network");
    let current = json_body(router.clone().oneshot(authed(&path)).await.unwrap()).await;
    assert_eq!(current["result"]["ingress_available"], true);

    for _ in 0..2 {
        let response = router
            .clone()
            .oneshot(admin(Method::DELETE, &path, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(json_body(response).await["result"].is_null());
    }
    let current = json_body(router.clone().oneshot(authed(&path)).await.unwrap()).await;
    assert!(current["result"].is_null());
    let response = router
        .clone()
        .oneshot(put_origin(
            &other_path,
            "deployer-token",
            r#"{"transport":"external_proxy","origin":"http://example.com:8080"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn authenticated_vendor_status_and_upgrade_surfaces() {
    let state = state();
    let router = app(state);

    for uri in [
        "/open-compute/capabilities",
        "/open-compute/system/status",
        "/open-compute/upgrade/check",
        "/accounts/account/open-compute/capabilities",
        "/accounts/account/open-compute/system/status",
        "/accounts/account/open-compute/upgrade/check",
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

#[tokio::test]
async fn d1_migration_routes_authenticate_and_resolve_authority_before_body_parsing() {
    let router = app(state());
    let uri = "/accounts/account/open-compute/d1/databases/db/migrations";
    let unauthenticated = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("not-json"))
        .unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(unauthenticated)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let unresolved = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("not-json"))
        .unwrap();
    assert_eq!(
        router.clone().oneshot(unresolved).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    assert_eq!(
        router
            .oneshot(authed(&format!("{uri}?unexpected=true")))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
}
