use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use open_compute_artifacts::{MapEnv, ObjectBackend, resolve_s3_credentials_with};
use open_compute_core::{
    BindingKind, ImagesConfig, MetricsConfig, PlatformConfig, RequestId, ResourceId,
    ResponseCacheConfig, SecretString,
};
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, DurableObjectRepository, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRepository,
};
use tower::ServiceExt as _;

#[tokio::test]
async fn composed_cache_images_authority_reports_capacity_and_collects_empty_store() {
    let (_temp, mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let worker = open_compute_storage::WorkerRepository::new(storage.db())
        .create_worker(account, "cache-worker", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let namespace_id = ResourceId::generate();
    let ResourceCreateReservation::Reserved(namespace) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::DoNamespace,
                name: "cache-do",
                idempotency_key: "cache-do",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &[3; 32],
                resource_id: namespace_id,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 2,
                expires_at_ms: i64::MAX,
            },
            100,
        )
        .unwrap()
    else {
        panic!("resource reservation");
    };
    DurableObjectRepository::new(&storage)
        .ensure_namespace(&namespace, worker.id, "CacheObject")
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(namespace_id, 3)
        .unwrap();
    let mut object_id = [9; 32];
    object_id[..8].copy_from_slice(&open_compute_core::durable_object_namespace_prefix(
        namespace_id,
    ));
    let connection =
        rusqlite::Connection::open(storage.data_dir().root().join("control.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO do_objects(namespace_resource_id, object_id, generation, state, \
             created_at_ms, updated_at_ms, deleted_at_ms) VALUES (?1, ?2, 1, 'ready', 4, 4, NULL)",
            rusqlite::params![namespace_id.to_string(), hex::encode(object_id)],
        )
        .unwrap();
    let s3 = PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{}"
bucket = "open-compute"
prefix = "system/"
connect_timeout_ms = 100
request_timeout_ms = 1000
"#,
        mock.endpoint
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone();
    let credentials = resolve_s3_credentials_with(
        &s3,
        &MapEnv::new()
            .with("S3_ACCESS_KEY_ID", "test-access")
            .with("S3_SECRET_ACCESS_KEY", "test-secret"),
    )
    .unwrap();
    let artifacts =
        ArtifactStore::new(ObjectBackend::connect_s3(&s3, &credentials, 1024 * 1024).unwrap());
    let cache = Arc::new(
        CacheManager::open(storage.data_dir().root(), ResponseCacheConfig::default()).unwrap(),
    );
    let images = Arc::new(ImageBindingService::new(
        storage.clone(),
        ImagesConfig::default(),
    ));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let api = CacheImagesApiState::new(
        storage.clone(),
        cache,
        images,
        artifacts,
        WorkersConfig::default(),
        Arc::new(SnapshotPins::empty()),
        metrics,
    );

    assert!(format!("{api:?}").contains("CacheImagesApiState"));
    assert_eq!(api.cache_stats().unwrap().entries, 0);
    let capacity = api.image_capacity().unwrap();
    assert_eq!(capacity.active_sessions, 0);
    assert_eq!(capacity.active_transforms, 0);
    assert!(capacity.max_concurrency > 0);
    assert_eq!(api.garbage_collect().await.unwrap(), 0);

    let state = state
        .with_v4_tokens(
            SecretString::new("deployer-token"),
            SecretString::new("read-token"),
        )
        .with_platform_storage(storage)
        .with_cache_images_api(api);
    let public_account = state
        .cloudflare_v4_account()
        .unwrap()
        .public_id()
        .to_owned();
    let app = crate::http::admin_router(state);
    let worker_endpoints =
        format!("/client/v4/accounts/{public_account}/open-compute/workers/cache-worker/endpoints");
    let durable_objects =
        format!("/client/v4/accounts/{public_account}/open-compute/durable-objects");
    let mut public_namespace = None;
    for path in [
        "/client/v4/open-compute/capabilities",
        "/client/v4/open-compute/system/status",
        "/client/v4/open-compute/scheduler",
        "/client/v4/open-compute/cache",
        "/client/v4/open-compute/images/capacity",
        &worker_endpoints,
        &durable_objects,
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    for operation in ["pause", "resume", "repair"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/client/v4/open-compute/scheduler/{operation}"))
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    let invalid_cache_query = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/cache?unexpected=true")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_cache_query.status(), StatusCode::BAD_REQUEST);
    let invalid_gc_body = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/client/v4/open-compute/cache/garbage-collection")
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_gc_body.status(), StatusCode::BAD_REQUEST);
    for path in [
        "/client/v4/open-compute/cache".to_owned(),
        "/client/v4/open-compute/images/capacity".to_owned(),
        "/client/v4/open-compute/system/status".to_owned(),
        worker_endpoints,
        durable_objects.clone(),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header(header::AUTHORIZATION, "Bearer read-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(value["success"], true, "{path}");
        if path == durable_objects {
            assert_eq!(value["result"].as_array().unwrap().len(), 1);
            assert_eq!(value["result"][0]["class_name"], "CacheObject");
            public_namespace = value["result"][0]["id"].as_str().map(str::to_owned);
        }
    }
    let public_namespace = public_namespace.unwrap();
    let objects_path = format!(
        "/client/v4/accounts/{public_account}/open-compute/durable-objects/{public_namespace}/objects"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(objects_path)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(value["result"].as_array().unwrap().len(), 1);
    assert_eq!(value["result"][0]["namespace_id"], public_namespace);
    let collected = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/client/v4/open-compute/cache/garbage-collection")
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(collected.status(), StatusCode::OK);
}
