use super::{
    IDEMPOTENCY_TTL_MS, create, create_fingerprint, current_named_resource,
    normalize_bucket_create_content_type, put_idempotency_key, valid_bucket_name,
};
use crate::cloudflare_v4::{V4RequestContext, V4Role};
use crate::health::HealthCoordinator;
use crate::http::HttpState;
use crate::metrics::MetricsRegistry;
use crate::r2_api::R2ApiState;
use axum::body::Body;
use axum::http::{HeaderValue, Request, header};
use open_compute_artifacts::{
    MapEnv, MockS3, R2ObjectStore, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, PlatformConfig, R2Config, RequestId, ResourceAvailability, ResourceId,
    ResourceState, SystemClock,
};
use open_compute_storage::{
    PlatformStorage, R2_SCHEMA_VERSION, R2BucketRepository, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRecord, ResourceRepository,
};
use open_compute_workers::ResourcePins;
use std::sync::Arc;
use std::time::Duration;

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
    let api = R2ApiState::new(
        storage.clone(),
        objects,
        ResourcePins::new(),
        R2Config {
            max_object_bytes: 1024 * 1024,
            max_staging_bytes: 2 * 1024 * 1024,
            ..R2Config::default()
        },
        Duration::from_secs(1),
    );
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd")
            .expect("metrics registry"),
    );
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
