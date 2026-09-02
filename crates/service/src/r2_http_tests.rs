use super::*;
use crate::health::HealthCoordinator;
use crate::http;
use crate::metrics::MetricsRegistry;
use crate::r2_backend::R2BindingService;
use axum::body::to_bytes;
use axum::http::Request;
use open_compute_artifacts::{
    MapEnv, MockS3, R2PutOptions, R2UploadSource, S3ArtifactClient, UserObjectKey, hash_bytes,
    resolve_s3_credentials_with,
};
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{PlatformConfig, SecretString, SystemClock};
use tempfile::TempDir;
use tower::ServiceExt as _;

struct Fixture {
    _temp: TempDir,
    _mock: MockS3,
    storage: Arc<PlatformStorage>,
    objects: R2ObjectStore,
    api: R2ApiState,
    router: Router,
    account: AccountId,
    metrics: Arc<MetricsRegistry>,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let mock = MockS3::spawn("open-compute").await;
    let s3 = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
bucket = "open-compute"
prefix = "system/"
r2_prefix = "tenant/r2/"
connect_timeout_ms = 100
request_timeout_ms = 1000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&s3, &env).unwrap();
    let objects =
        R2ObjectStore::new(S3ArtifactClient::connect(&s3, &credentials, 1024 * 1024).unwrap());
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let pins = ResourcePins::new();
    let binding = Arc::new(
        R2BindingService::new(
            storage.clone(),
            pins.clone(),
            objects.clone(),
            R2Config {
                max_object_bytes: 1024 * 1024,
                max_staging_bytes: 2 * 1024 * 1024,
                ..R2Config::default()
            },
        )
        .unwrap(),
    );
    let api = R2ApiState::new(
        storage.clone(),
        objects.clone(),
        pins,
        R2Config {
            max_object_bytes: 1024 * 1024,
            max_staging_bytes: 2 * 1024 * 1024,
            ..R2Config::default()
        },
        Duration::from_secs(1),
    )
    .with_metrics(metrics.clone())
    .with_binding(binding);
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics.clone(),
        false,
        Some(SecretString::new("admin-secret")),
    )
    .with_r2_api(api.clone());
    Fixture {
        _temp: temp,
        _mock: mock,
        storage,
        objects,
        api,
        router: http::admin_router(state),
        account,
        metrics,
    }
}

fn request(method: &str, uri: &str, body: impl Serialize, key: Option<&str>) -> Request<Body> {
    request_auth(method, uri, body, key, Some("admin-secret"))
}

fn request_auth(
    method: &str,
    uri: &str,
    body: impl Serialize,
    key: Option<&str>,
    token: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_HEADER, key);
    }
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn json(response: Response) -> (StatusCode, serde_json::Value, Vec<u8>) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    let value = serde_json::from_slice(&bytes).unwrap();
    (status, value, bytes)
}

async fn create_bucket(fixture: &Fixture, name: &str, key: &str) -> (ResourceId, Vec<u8>) {
    let uri = format!("/operator/api/v1/accounts/{}/r2/buckets", fixture.account);
    let (status, body, bytes) = json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &uri,
                serde_json::json!({"name": name}),
                Some(key),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    (
        body["bucket"]["resourceId"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        bytes,
    )
}

#[test]
fn delete_query_is_strict() {
    assert!(!parse_force(None).unwrap());
    assert!(!parse_force(Some("force=false")).unwrap());
    assert!(parse_force(Some("force=true")).unwrap());
    assert_eq!(
        parse_force(Some("force=1")).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
}

#[tokio::test]
async fn control_boundary_and_helper_failures_are_stable_and_bounded() {
    let fixture = fixture().await;
    let collection = format!("/operator/api/v1/accounts/{}/r2/buckets", fixture.account);
    let (resource, _) = create_bucket(&fixture, "errors", "create-errors").await;
    let item = format!("{collection}/{resource}");

    let (status, body, _) = json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &collection, serde_json::json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["buckets"].as_array().unwrap().len(), 1);

    for (method, uri, body, key, expected) in [
        (
            "GET",
            "/operator/api/v1/accounts/bad/r2/buckets".to_owned(),
            serde_json::json!({}),
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            "GET",
            format!("{collection}/bad"),
            serde_json::json!({}),
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            "GET",
            format!("{collection}/{}", ResourceId::generate()),
            serde_json::json!({}),
            None,
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            collection.clone(),
            serde_json::json!({"name": "missing-key"}),
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            "DELETE",
            format!("{item}?force=maybe"),
            serde_json::json!({}),
            Some("bad-query"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "DELETE",
            item.clone(),
            serde_json::json!({}),
            None,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = fixture
            .router
            .clone()
            .oneshot(request(method, &uri, body, key))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{method} {uri}");
    }

    let invalid_json = Request::builder()
        .method("PATCH")
        .uri(&item)
        .header("content-type", "application/json")
        .header("authorization", "Bearer admin-secret")
        .body(Body::from("{"))
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(invalid_json)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let oversized = read_json::<CreateBucketBody>(
        Request::builder()
            .body(Body::from(vec![b'x'; MAX_JSON_BODY + 1]))
            .unwrap(),
    )
    .await;
    assert!(matches!(oversized, Err(ref error) if error.code() == ErrorCode::LimitInvalid));

    for key in ["", "has space", &"x".repeat(129)] {
        let request = Request::builder()
            .header(IDEMPOTENCY_HEADER, key)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            idempotency_key(&request).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }
    assert!(parse_account("bad").is_err());
    assert!(parse_ids(&fixture.account.to_string(), "bad").is_err());
    let mut with_id = Request::new(Body::empty());
    let fixed_id = RequestId::generate();
    with_id.extensions_mut().insert(fixed_id);
    assert_eq!(request_id(&with_id), fixed_id);

    let held = ForceDeleteGuard::acquire(&fixture.api).await.unwrap();
    let mut saturated = fixture.api.clone();
    saturated.delete_drain_timeout = Duration::from_millis(1);
    let overloaded = ForceDeleteGuard::acquire(&saturated).await;
    assert!(matches!(overloaded, Err(ref error) if error.code() == ErrorCode::R2Overloaded));
    drop(held);
    assert!(
        fixture
            .metrics
            .render(&HealthCoordinator::new().snapshot())
            .contains("r2_force_delete_remaining_batches 0")
    );

    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let locked = http::admin_router(HttpState::for_test(
        HealthCoordinator::new(),
        metrics.clone(),
        false,
        Some(SecretString::new("admin-secret")),
    ));
    assert_eq!(
        locked
            .clone()
            .oneshot(request_auth(
                "GET",
                &collection,
                serde_json::json!({}),
                None,
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let authorized_missing = Request::builder()
        .method("GET")
        .uri(&collection)
        .header("authorization", "Bearer admin-secret")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        locked.oneshot(authorized_missing).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    for (code, status) in [
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::ResourceReferenced, StatusCode::CONFLICT),
        (ErrorCode::AdminAuthRequired, StatusCode::UNAUTHORIZED),
        (ErrorCode::LimitInvalid, StatusCode::BAD_REQUEST),
        (ErrorCode::R2ResultUnknown, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::ResourceInvariantViolation,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        assert_eq!(
            error_response(PlatformError::new(code, "safe"), RequestId::generate()).status(),
            status
        );
    }

    struct FailSerialize;
    impl Serialize for FailSerialize {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("expected"))
        }
    }
    assert_eq!(
        json_response(&FailSerialize, StatusCode::OK).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn control_api_replays_create_hides_locator_and_recovers_force_delete() {
    let fixture = fixture().await;
    let collection = format!("/operator/api/v1/accounts/{}/r2/buckets", fixture.account);
    let (resource, first_bytes) = create_bucket(&fixture, "images", "create-images").await;
    let replay = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            &collection,
            serde_json::json!({"name": "images"}),
            Some("create-images"),
        ))
        .await
        .unwrap();
    let (status, _, replay_bytes) = json(replay).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay_bytes, first_bytes);
    assert!(!String::from_utf8(first_bytes).unwrap().contains("physical"));

    let item = format!("{collection}/{resource}");
    let (status, body, _) = json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &item, serde_json::json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bucket"]["maxObjectBytes"], 1024 * 1024);
    let renamed = fixture
        .router
        .clone()
        .oneshot(request(
            "PATCH",
            &item,
            serde_json::json!({"name": "renamed"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);

    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(fixture.account, resource)
        .unwrap();
    let locator = fixture
        .objects
        .locator(resource, &bucket.physical_prefix)
        .unwrap();
    let key = UserObjectKey::parse("same/key").unwrap();
    let stage = fixture
        .storage
        .data_dir()
        .root()
        .join("r2-control-test-upload");
    std::fs::write(&stage, b"value").unwrap();
    fixture
        .objects
        .put_file(
            &locator,
            &key,
            &R2UploadSource {
                path: stage,
                length: 5,
                checksums: hash_bytes(b"value"),
                version: uuid::Uuid::now_v7().hyphenated().to_string(),
            },
            &R2PutOptions::default(),
            None,
        )
        .await
        .unwrap();

    let refused = fixture
        .router
        .clone()
        .oneshot(request(
            "DELETE",
            &item,
            serde_json::json!({}),
            Some("delete-refused"),
        ))
        .await
        .unwrap();
    let (status, body, _) = json(refused).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "R2_BUCKET_NOT_EMPTY");
    assert_eq!(
        ResourceRepository::new(fixture.storage.db())
            .get(fixture.account, resource)
            .unwrap()
            .state,
        ResourceState::Ready
    );

    let deleted = fixture
        .router
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("{item}?force=true"),
            serde_json::json!({}),
            Some("delete-force"),
        ))
        .await
        .unwrap();
    let (status, _, deleted_bytes) = json(deleted).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        ResourceRepository::new(fixture.storage.db())
            .get(fixture.account, resource)
            .unwrap()
            .state,
        ResourceState::Tombstoned
    );
    assert!(
        fixture
            .objects
            .read_identity(&locator)
            .await
            .unwrap()
            .is_none()
    );
    assert!(fixture.objects.is_empty(&locator).await.unwrap());

    let replay = fixture
        .router
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("{item}?force=true"),
            serde_json::json!({}),
            Some("delete-force"),
        ))
        .await
        .unwrap();
    let (status, _, replay_bytes) = json(replay).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay_bytes, deleted_bytes);
    let conflict = fixture
        .router
        .clone()
        .oneshot(request(
            "DELETE",
            &item,
            serde_json::json!({}),
            Some("delete-force"),
        ))
        .await
        .unwrap();
    let (status, body, _) = json(conflict).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "IDEMPOTENCY_CONFLICT");

    let (replacement, _) = create_bucket(&fixture, "renamed", "recreate-renamed").await;
    assert_ne!(replacement, resource);
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 0);
}

#[tokio::test]
async fn lifecycle_reconcile_converges_create_delete_and_marker_crash_boundaries() {
    let fixture = fixture().await;
    let before_marker = reserve_bucket(&fixture, "before-marker", "crash-create-1", 100);
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        ResourceRepository::new(fixture.storage.db())
            .get(fixture.account, before_marker.id)
            .unwrap()
            .state,
        ResourceState::Ready
    );

    let after_marker = reserve_bucket(&fixture, "after-marker", "crash-create-2", 110);
    fixture.api.driver().create(&after_marker).await.unwrap();
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        ResourceRepository::new(fixture.storage.db())
            .get(fixture.account, after_marker.id)
            .unwrap()
            .state,
        ResourceState::Ready
    );

    let (deleting, _) = create_bucket(&fixture, "deleting", "crash-delete-1").await;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(fixture.account, deleting)
        .unwrap();
    let locator = fixture
        .objects
        .locator(deleting, &bucket.physical_prefix)
        .unwrap();
    for (index, value) in [b"one".as_slice(), b"two".as_slice()]
        .into_iter()
        .enumerate()
    {
        let path = fixture
            .storage
            .data_dir()
            .root()
            .join(format!("recovery-{index}"));
        std::fs::write(&path, value).unwrap();
        fixture
            .objects
            .put_file(
                &locator,
                &UserObjectKey::parse(&format!("key-{index}")).unwrap(),
                &R2UploadSource {
                    path,
                    length: value.len() as u64,
                    checksums: hash_bytes(value),
                    version: uuid::Uuid::now_v7().hyphenated().to_string(),
                },
                &R2PutOptions::default(),
                None,
            )
            .await
            .unwrap();
    }
    ResourceRepository::new(fixture.storage.db())
        .begin_delete(fixture.account, deleting, 120)
        .unwrap();
    R2BucketRepository::new(fixture.storage.db())
        .mark_delete_started(deleting, 121)
        .unwrap();
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    assert!(fixture.objects.is_empty(&locator).await.unwrap());
    assert!(
        fixture
            .objects
            .read_identity(&locator)
            .await
            .unwrap()
            .is_none()
    );

    let (marker_gone, _) = create_bucket(&fixture, "marker-gone", "crash-delete-2").await;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(fixture.account, marker_gone)
        .unwrap();
    let locator = fixture
        .objects
        .locator(marker_gone, &bucket.physical_prefix)
        .unwrap();
    ResourceRepository::new(fixture.storage.db())
        .begin_delete(fixture.account, marker_gone, 130)
        .unwrap();
    R2BucketRepository::new(fixture.storage.db())
        .mark_delete_started(marker_gone, 131)
        .unwrap();
    fixture.api.driver().finalize_delete(&bucket).await.unwrap();
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    assert!(
        fixture
            .objects
            .read_identity(&locator)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        ResourceRepository::new(fixture.storage.db())
            .get(fixture.account, marker_gone)
            .unwrap()
            .state,
        ResourceState::Tombstoned
    );
}

fn reserve_bucket(
    fixture: &Fixture,
    name: &str,
    key: &str,
    now_ms: i64,
) -> open_compute_storage::ResourceRecord {
    let fingerprint = fixture.storage.crypto().fingerprint_request(key.as_bytes());
    let reservation = ResourceRepository::new(fixture.storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: fixture.account,
                kind: BindingKind::R2Bucket,
                name,
                idempotency_key: key,
                fingerprint_key_id: fixture.storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms,
                expires_at_ms: now_ms + 1_000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("unexpected reservation")
    };
    resource
}

#[tokio::test]
async fn operator_object_put_get_metadata_and_delete_round_trip() {
    let fixture = fixture().await;
    let (resource, _) =
        create_bucket(&fixture, "operator-objects", "create-operator-objects").await;
    let object_uri = format!(
        "/operator/api/v1/accounts/{}/r2/buckets/{resource}/objects/hello.txt",
        fixture.account
    );
    let put_request = Request::builder()
        .method("PUT")
        .uri(&object_uri)
        .header("authorization", "Bearer admin-secret")
        .header("idempotency-key", "put-hello")
        .header("content-type", "application/octet-stream")
        .body(Body::from(b"operator-body".as_slice()))
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(put_request)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let meta_uri = format!("{object_uri}?metadata=true");
    let (status, body, _) = json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &meta_uri, serde_json::json!(null), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["key"], "hello.txt");
    assert_eq!(body["size"], 13);

    let get_response = fixture
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&object_uri)
                .header("authorization", "Bearer admin-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let bytes = to_bytes(get_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"operator-body");

    let list_uri = format!(
        "/operator/api/v1/accounts/{}/r2/buckets/{resource}/objects?prefix=hello&limit=1",
        fixture.account
    );
    let (status, listed, _) = json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &list_uri, serde_json::json!(null), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["objects"][0]["key"], "hello.txt");
    assert_eq!(listed["truncated"], false);

    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &object_uri,
                serde_json::json!(null),
                Some("delete-hello")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &meta_uri, serde_json::json!(null), None))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn operator_bucket_catalog_filters_sorts_and_paginates_deterministically() {
    let fixture = fixture().await;
    for (name, key) in [
        ("alpha", "catalog-alpha"),
        ("beta", "catalog-beta"),
        ("gamma", "catalog-gamma"),
    ] {
        create_bucket(&fixture, name, key).await;
    }
    let base = format!("/operator/api/v1/accounts/{}/r2/buckets", fixture.account);
    let (status, first, _) = json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{base}?sort=name&direction=asc&limit=1"),
                serde_json::json!(null),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["buckets"][0]["name"], "alpha");
    assert_eq!(first["listComplete"], false);
    let cursor = first["cursor"].as_str().unwrap();

    let (status, second, _) = json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{base}?sort=name&direction=asc&limit=1&cursor={cursor}"),
                serde_json::json!(null),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["buckets"][0]["name"], "beta");

    for query in [
        "search=alpha&status=ready&sort=createdAt&direction=desc&limit=2",
        "search=%20%20&sort=updatedAt&direction=asc",
    ] {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "GET",
                    &format!("{base}?{query}"),
                    serde_json::json!(null),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "query={query}"
        );
    }
    for query in [
        "unknown=1".to_owned(),
        "sort=invalid".to_owned(),
        "direction=invalid".to_owned(),
        "cursor=not-base64".to_owned(),
        format!("sort=createdAt&direction=asc&cursor={cursor}"),
    ] {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "GET",
                    &format!("{base}?{query}"),
                    serde_json::json!(null),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
            "query={query}"
        );
    }
}
