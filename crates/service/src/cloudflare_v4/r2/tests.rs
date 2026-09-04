use super::{
    IDEMPOTENCY_TTL_MS, create, create_fingerprint, current_named_resource,
    normalize_bucket_create_content_type, put_idempotency_key, valid_bucket_name,
};
use crate::cloudflare_v4::{V4RequestContext, V4Role};
use crate::health::HealthCoordinator;
use crate::http::HttpState;
use crate::metrics::MetricsRegistry;
use crate::r2_api::R2ApiState;
use crate::r2_backend::R2BindingService;
use axum::body::Body;
use axum::body::to_bytes;
use axum::http::{HeaderMap, HeaderValue, Request, header};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use open_compute_artifacts::{
    MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, PlatformConfig, PlatformId, R2Config, RequestId, ResourceAvailability,
    ResourceId, ResourceState, SecretString, SystemClock,
};
use open_compute_storage::{
    PlatformStorage, R2_SCHEMA_VERSION, R2BucketRepository, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRecord, ResourceRepository,
};
use open_compute_workers::ResourcePins;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt as _;

fn resource(name: &str, state: ResourceState) -> ResourceRecord {
    ResourceRecord {
        id: ResourceId::generate(),
        account_id: AccountId::generate(),
        kind: BindingKind::R2Bucket,
        name: name.to_owned(),
        state,
        availability: ResourceAvailability::Healthy,
        availability_code: None,
        spec_generation: 1,
        driver_schema_version: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
        deleted_at_ms: (state == ResourceState::Tombstoned).then_some(1),
    }
}

#[test]
fn bucket_names_match_the_pinned_wrangler_contract() {
    assert!(valid_bucket_name("abc"));
    assert!(valid_bucket_name(&format!("a{}z", "b".repeat(61))));
    assert!(!valid_bucket_name("ab"));
    assert!(!valid_bucket_name(&format!("a{}z", "b".repeat(62))));
    assert!(!valid_bucket_name("Upper"));
    assert!(!valid_bucket_name("-leading"));
    assert!(!valid_bucket_name("trailing-"));
}

#[test]
fn bucket_create_accepts_the_fetch_string_json_content_type() {
    let mut request = Request::builder()
        .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
        .body(Body::empty())
        .unwrap();
    normalize_bucket_create_content_type(&mut request).unwrap();
    assert_eq!(
        request.headers()[header::CONTENT_TYPE],
        HeaderValue::from_static("application/json")
    );

    let mut duplicate = Request::builder()
        .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    assert!(normalize_bucket_create_content_type(&mut duplicate).is_err());
}

#[test]
fn put_by_name_recovery_ignores_tombstones_and_selects_creating_resource() {
    let tombstone = resource("reused-name", ResourceState::Tombstoned);
    let creating = resource("reused-name", ResourceState::Creating);
    let creating_id = creating.id;
    let selected = current_named_resource(vec![tombstone, creating], "reused-name")
        .expect("creating resource remains recoverable");
    assert_eq!(selected.id, creating_id);
    assert_eq!(selected.state, ResourceState::Creating);
}

struct Fixture {
    _temp: tempfile::TempDir,
    _mock: MockS3,
    storage: Arc<PlatformStorage>,
    state: HttpState,
    account_id: AccountId,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temporary data directory");
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
        .expect("platform storage"),
    );
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
    .expect("S3 config")
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&s3, &env).expect("S3 credentials");
    let objects = R2ObjectStore::new(
        S3ArtifactClient::connect(&s3, &credentials, 1024 * 1024).expect("S3 client"),
    );
    let pins = ResourcePins::new();
    let r2_config = R2Config {
        max_object_bytes: 1024 * 1024,
        max_staging_bytes: 2 * 1024 * 1024,
        ..R2Config::default()
    };
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd")
            .expect("metrics registry"),
    );
    let binding = Arc::new(
        R2BindingService::new(
            storage.clone(),
            pins.clone(),
            objects.clone(),
            r2_config.clone(),
        )
        .expect("R2 binding service")
        .with_metrics(metrics.clone()),
    );
    let api = R2ApiState::new(
        storage.clone(),
        objects,
        pins,
        r2_config,
        Duration::from_secs(1),
    )
    .with_binding(binding);
    let state =
        HttpState::for_test(HealthCoordinator::new(), metrics, false, None).with_r2_api(api);
    let account_id = storage.identity().default_account_id;
    Fixture {
        _temp: temp,
        _mock: mock,
        storage,
        state,
        account_id,
    }
}

fn context() -> V4RequestContext {
    V4RequestContext {
        role: V4Role::Deployer,
        request_id: RequestId::generate(),
    }
}

fn assert_put_reservation_complete(fixture: &Fixture, name: &str) {
    let api = fixture.state.r2_api().expect("R2 API");
    let now = super::now_ms().expect("clock");
    let key = put_idempotency_key(api, fixture.account_id, name).expect("PUT idempotency key");
    let fingerprint = create_fingerprint(api, fixture.account_id, name).expect("fingerprint");
    let reservation = ResourceRepository::new(fixture.storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: fixture.account_id,
                kind: BindingKind::R2Bucket,
                name,
                idempotency_key: &key,
                fingerprint_key_id: fixture.storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: now,
                expires_at_ms: now + IDEMPOTENCY_TTL_MS,
            },
            fixture
                .storage
                .hardening()
                .max_resources_per_kind_per_account,
        )
        .expect("reservation read");
    assert!(matches!(
        reservation,
        ResourceCreateReservation::Complete(_)
    ));
}

#[tokio::test]
async fn put_by_name_completes_crash_recovery_recreates_and_concurrent_reservations() {
    let fixture = fixture().await;
    let api = fixture.state.r2_api().expect("R2 API");
    let name = "crash-recovery";
    let now = super::now_ms().expect("clock");
    let key = put_idempotency_key(api, fixture.account_id, name).expect("initial generation key");
    let fingerprint = create_fingerprint(api, fixture.account_id, name).expect("fingerprint");
    let reservation = ResourceRepository::new(fixture.storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: fixture.account_id,
                kind: BindingKind::R2Bucket,
                name,
                idempotency_key: &key,
                fingerprint_key_id: fixture.storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: now,
                expires_at_ms: now + IDEMPOTENCY_TTL_MS,
            },
            fixture
                .storage
                .hardening()
                .max_resources_per_kind_per_account,
        )
        .expect("initial reservation");
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("fresh PUT must reserve")
    };
    api.resource_driver()
        .reconcile(&resource)
        .await
        .expect("driver reconcile before crash");
    ResourceRepository::new(fixture.storage.db())
        .mark_ready(resource.id, now)
        .expect("mark ready before crash");

    let retry = create(
        &fixture.state,
        context(),
        fixture.account_id,
        name.to_owned(),
        true,
    )
    .await;
    assert!(retry.status().is_success());
    assert_put_reservation_complete(&fixture, name);

    let resources = ResourceRepository::new(fixture.storage.db());
    let first_id = resource.id;
    resources
        .begin_delete(fixture.account_id, first_id, now + 1)
        .expect("begin delete");
    R2BucketRepository::new(fixture.storage.db())
        .mark_delete_started(first_id, now + 1)
        .expect("record delete attempt");
    resources
        .mark_tombstoned(fixture.account_id, first_id, RequestId::generate(), now + 2)
        .expect("tombstone first generation");
    let second_generation_key =
        put_idempotency_key(api, fixture.account_id, name).expect("second generation key");
    assert_ne!(key, second_generation_key);
    let recreated = create(
        &fixture.state,
        context(),
        fixture.account_id,
        name.to_owned(),
        true,
    )
    .await;
    assert!(recreated.status().is_success());
    let second_id = ResourceRepository::new(fixture.storage.db())
        .list(fixture.account_id, Some(BindingKind::R2Bucket))
        .expect("resource catalog")
        .into_iter()
        .find(|resource| resource.name == name && resource.state == ResourceState::Ready)
        .expect("recreated live bucket")
        .id;
    assert_ne!(first_id, second_id);
    assert_put_reservation_complete(&fixture, name);

    let (left, right) = tokio::join!(
        create(
            &fixture.state,
            context(),
            fixture.account_id,
            "concurrent-put".to_owned(),
            true,
        ),
        create(
            &fixture.state,
            context(),
            fixture.account_id,
            "concurrent-put".to_owned(),
            true,
        )
    );
    assert!(left.status().is_success());
    assert!(right.status().is_success());
    assert_put_reservation_complete(&fixture, "concurrent-put");
}

#[tokio::test]
async fn startup_reconciliation_finishes_creating_and_deleting_r2_generations() {
    let fixture = fixture().await;
    let api = fixture.state.r2_api().unwrap();
    assert!(format!("{api:?}").contains("R2ApiState"));
    let resources = ResourceRepository::new(fixture.storage.db());
    let now = super::now_ms().unwrap();
    let reserve = |kind, name: &str, schema| {
        resources
            .reserve_create(
                &ReserveResourceCreate {
                    account_id: fixture.account_id,
                    kind,
                    name,
                    idempotency_key: name,
                    fingerprint_key_id: fixture.storage.crypto().fingerprint_key_id(),
                    request_fingerprint: &[4; 32],
                    resource_id: ResourceId::generate(),
                    driver_schema_version: schema,
                    request_id: RequestId::generate(),
                    now_ms: now,
                    expires_at_ms: now + IDEMPOTENCY_TTL_MS,
                },
                1_000_000,
            )
            .unwrap()
    };
    let ResourceCreateReservation::Reserved(r2) =
        reserve(BindingKind::R2Bucket, "startup-r2", R2_SCHEMA_VERSION)
    else {
        panic!("fresh R2 reservation")
    };
    let ResourceCreateReservation::Reserved(kv) =
        reserve(BindingKind::KvNamespace, "startup-kv", 1)
    else {
        panic!("fresh KV reservation")
    };

    assert_eq!(api.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        resources.get(fixture.account_id, r2.id).unwrap().state,
        ResourceState::Ready
    );
    assert_eq!(
        resources.get(fixture.account_id, kv.id).unwrap().state,
        ResourceState::Creating
    );

    resources
        .begin_delete(fixture.account_id, r2.id, now + 1)
        .unwrap();
    assert_eq!(api.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        resources.get(fixture.account_id, r2.id).unwrap().state,
        ResourceState::Tombstoned
    );
    assert_eq!(api.reconcile_pending().await.unwrap(), 0);
}

async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn bucket_routes_cover_create_cursor_filter_headers_and_delete() {
    let fixture = fixture().await;
    let authority = crate::cloudflare_v4::accounts::AccountAuthority::new(
        PlatformId::generate(),
        fixture.account_id,
        1,
    );
    let public_account = authority.public_id().to_owned();
    let app = crate::http::admin_router(
        fixture
            .state
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let collection = format!("/client/v4/accounts/{public_account}/r2/buckets");

    for name in ["bucket-one", "bucket-two", "bucket-three"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&collection)
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
                    .body(Body::from(format!(r#"{{"name":"{name}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "create {name}");
    }
    let put = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("{collection}/bucket-four"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header("cf-r2-storage-class", "Standard")
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{collection}?per_page=2&order=name&direction=asc"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header("cf-r2-jurisdiction", "default")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = json(first).await;
    assert_eq!(first["result_info"]["count"], 2);
    let cursor = first["result_info"]["cursor"].as_str().unwrap();
    assert!(!cursor.is_empty());
    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{collection}?per_page=2&order=name&direction=asc&cursor={cursor}"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(json(second).await["result_info"]["count"], 2);

    let filtered = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{collection}?name_contains=three&start_after=bucket-two&direction=desc"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(filtered.status(), StatusCode::OK);

    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{collection}/bucket-one"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(json(fetched).await["result"]["name"], "bucket-one");

    for request in [
        Request::builder()
            .uri(format!("{collection}?per_page=0"))
            .header(header::AUTHORIZATION, "Bearer read-token")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri(format!("{collection}?cursor=bad&start_after=bucket-one"))
            .header(header::AUTHORIZATION, "Bearer read-token")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri(&collection)
            .header(header::AUTHORIZATION, "Bearer read-token")
            .header("cf-r2-jurisdiction", "invalid")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method(Method::PUT)
            .uri(format!("{collection}/bucket-five"))
            .header(header::AUTHORIZATION, "Bearer deployer-token")
            .header("cf-r2-storage-class", "InfrequentAccess")
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert!(!response.status().is_success());
    }

    let object = format!("{collection}/bucket-one/objects/path/to/object.txt");
    let put_object = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(&object)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::CONTENT_LANGUAGE, "en")
                .header(header::CACHE_CONTROL, "public, max-age=60")
                .header("cf-r2-storage-class", "Standard")
                .header("cf-r2-data-catalog-check", "false")
                .header(header::CONTENT_LENGTH, "11")
                .body(Body::from("hello world"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put_object.status(), StatusCode::OK);
    let etag = json(put_object).await["result"]["etag"]
        .as_str()
        .unwrap()
        .to_owned();

    let get_object = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&object)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_object.status(), StatusCode::OK);
    assert_eq!(get_object.headers()[header::CONTENT_TYPE], "text/plain");
    assert_eq!(
        to_bytes(get_object.into_body(), 1024).await.unwrap(),
        "hello world"
    );

    let not_modified = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&object)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header(header::IF_NONE_MATCH, format!("\"{etag}\""))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);

    let unsupported_list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{collection}/bucket-one/objects?prefix=path&per_page=10"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsupported_list.status(), StatusCode::NOT_IMPLEMENTED);

    for request in [
        Request::builder()
            .method(Method::PUT)
            .uri(format!("{object}?query=true"))
            .header(header::AUTHORIZATION, "Bearer deployer-token")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method(Method::PUT)
            .uri(format!("{collection}/bucket-one/objects/invalid"))
            .header(header::AUTHORIZATION, "Bearer deployer-token")
            .header("cf-r2-data-catalog-check", "invalid")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri(format!("{object}?query=true"))
            .header(header::AUTHORIZATION, "Bearer read-token")
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let deleted_object = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(&object)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header("cf-r2-data-catalog-check", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_object.status(), StatusCode::OK);
    let missing_object = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&object)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_object.status(), StatusCode::NOT_FOUND);

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("{collection}/bucket-one"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let missing = app
        .oneshot(
            Request::builder()
                .uri(format!("{collection}/bucket-one"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bucket_header_query_and_cursor_validation_is_closed_and_signed() {
    use base64::Engine as _;

    let fixture = fixture().await;
    let api = fixture.state.r2_api().unwrap();

    let mut headers = HeaderMap::new();
    assert!(super::jurisdiction(&headers).is_ok());
    headers.insert("cf-r2-jurisdiction", HeaderValue::from_static("eu"));
    assert_eq!(
        super::jurisdiction(&headers),
        Err(super::V4Error::Unsupported)
    );
    headers.insert("cf-r2-jurisdiction", HeaderValue::from_static("fedramp"));
    assert_eq!(
        super::jurisdiction(&headers),
        Err(super::V4Error::Unsupported)
    );
    headers.insert("cf-r2-jurisdiction", HeaderValue::from_static("unknown"));
    assert_eq!(
        super::jurisdiction(&headers),
        Err(super::V4Error::InvalidRequest)
    );

    let mut duplicates = HeaderMap::new();
    duplicates.append("cf-r2-storage-class", HeaderValue::from_static("Standard"));
    duplicates.append("cf-r2-storage-class", HeaderValue::from_static("Standard"));
    assert_eq!(
        super::header_text(&duplicates, "cf-r2-storage-class"),
        Err(super::V4Error::InvalidRequest)
    );
    assert_eq!(
        super::header_text(&HeaderMap::new(), "missing").unwrap(),
        None
    );

    let empty = Request::builder().uri("/").body(Body::empty()).unwrap();
    let default = super::bucket_list_query(&empty).unwrap();
    assert_eq!(default.per_page, 20);
    assert_eq!(default.direction, None);
    for query in [
        "?per_page=not-a-number",
        "?per_page=1001",
        "?order=created_at",
        "?direction=sideways",
        "?unknown=true",
    ] {
        let request = Request::builder()
            .uri(format!("/{query}"))
            .body(Body::empty())
            .unwrap();
        assert!(super::bucket_list_query(&request).is_err(), "{query}");
    }

    let query = super::BucketListQuery {
        name_contains: Some("bucket".to_owned()),
        start_after: None,
        cursor: None,
        per_page: 2,
        direction: Some("asc".to_owned()),
    };
    let cursor = super::encode_cursor(api, fixture.account_id, &query, "bucket-one").unwrap();
    assert_eq!(
        super::decode_cursor(api, fixture.account_id, &query, &cursor).unwrap(),
        "bucket-one"
    );
    for invalid in [
        "missing-separator".to_owned(),
        format!("{cursor}.extra"),
        cursor.replacen('.', ".not-base64.", 1),
        format!("{cursor}x"),
    ] {
        assert!(
            super::decode_cursor(api, fixture.account_id, &query, &invalid).is_err(),
            "{invalid}"
        );
    }
    let different = super::BucketListQuery {
        name_contains: None,
        start_after: None,
        cursor: None,
        per_page: 3,
        direction: Some("desc".to_owned()),
    };
    assert!(super::decode_cursor(api, fixture.account_id, &different, &cursor).is_err());
    assert!(super::decode_cursor(api, AccountId::generate(), &query, &cursor).is_err());

    let encode_payload = |payload: super::BucketCursor| {
        let bytes = serde_json::to_vec(&payload).unwrap();
        let signature = api.storage().crypto().sign_r2_cursor(&bytes);
        let base64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}", base64.encode(bytes), base64.encode(signature))
    };
    let payload = |version, last_name: &str, expires_at_ms| super::BucketCursor {
        version,
        account_id: fixture.account_id.to_string(),
        name_contains: query.name_contains.clone(),
        per_page: query.per_page,
        direction: query.direction.clone(),
        last_name: last_name.to_owned(),
        expires_at_ms,
    };
    for invalid in [
        payload(2, "bucket-one", super::now_ms().unwrap() + 60_000),
        payload(1, "BAD", super::now_ms().unwrap() + 60_000),
        payload(1, "bucket-one", 0),
    ] {
        assert!(
            super::decode_cursor(api, fixture.account_id, &query, &encode_payload(invalid),)
                .is_err()
        );
    }
}
