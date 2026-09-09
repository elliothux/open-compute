use super::*;
use axum::body::to_bytes;
use axum::http::StatusCode;
use open_compute_artifacts::{
    MapEnv, MockS3, ObjectBackend, R2SsecKey, resolve_s3_credentials_with,
};
use open_compute_core::config::{DataConfig, MetricsConfig};
use open_compute_core::{
    AccountId, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, RequestId, SystemClock,
    WorkerId,
};
use open_compute_core::{BindingId, BindingKind};
use open_compute_storage::{
    NewVersion, NewVersionBinding, R2BucketRepository, R2MultipartPartRecord,
    R2MultipartRepository, R2MultipartState, R2MultipartUploadRecord, R2ObjectRecord,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository, WorkerRepository,
};
use open_compute_workers::R2ResourceDriver;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;

struct Fixture {
    _temp: tempfile::TempDir,
    mock: MockS3,
    service: R2BindingService,
    objects: R2ObjectStore,
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    binding: BindingId,
    version: VersionId,
    descriptor: [u8; 32],
    resource: ResourceId,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &DataConfig {
                path: root.clone(),
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
    let mock = MockS3::spawn("bucket").await;
    let config = open_compute_core::S3Config {
        endpoint: mock.endpoint.clone(),
        bucket: "bucket".to_owned(),
        ..open_compute_core::S3Config::default()
    };
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    let objects =
        R2ObjectStore::new(ObjectBackend::connect_s3(&config, &credentials, 1024 * 1024).unwrap());
    let account = storage.identity().default_account_id;
    let resource = ResourceId::generate();
    let fingerprint = storage.crypto().fingerprint_request(b"r2-backend-test");
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::R2Bucket,
                name: "objects",
                idempotency_key: "r2-backend-test",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: resource,
                driver_schema_version: open_compute_storage::R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1000,
            },
            1_000_000,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource_record) = reservation else {
        unreachable!()
    };
    let r2_config = R2Config {
        max_object_bytes: 1024 * 1024,
        max_staging_bytes: 2 * 1024 * 1024,
        operation_timeout_ms: 1000,
        ..R2Config::default()
    };
    R2ResourceDriver::new(&storage, objects.clone(), r2_config.clone())
        .create(&resource_record)
        .await
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource, 11)
        .unwrap();

    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "r2-worker", RequestId::generate(), 12, 1_000_000)
        .unwrap();
    let version = VersionId::generate();
    let binding = BindingId::generate();
    let descriptor = [9_u8; 32];
    workers
        .insert_staging_version(
            &version_input(account, worker.id, version),
            &open_compute_storage::NewVersionProducts {
                bindings: &[NewVersionBinding {
                    id: binding,
                    name: "BUCKET".to_owned(),
                    kind: BindingKind::R2Bucket,
                    resource_id: resource,
                    resource_spec_generation: 1,
                    capability_version: 1,
                    permissions_json: serde_json::to_vec(&CanonicalPermissions::default()).unwrap(),
                    config_json: serde_json::to_vec(&CanonicalBindingConfig::default()).unwrap(),
                    descriptor_sha256: descriptor,
                }],
                ..Default::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 13).unwrap();
    let pins = ResourcePins::new();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let service = R2BindingService::new(storage.clone(), pins.clone(), objects.clone(), r2_config)
        .unwrap()
        .with_metrics(metrics);
    Fixture {
        _temp: temp,
        mock,
        service,
        objects,
        storage,
        pins,
        binding,
        version,
        descriptor,
        resource,
    }
}

fn version_input(account_id: AccountId, worker_id: WorkerId, version_id: VersionId) -> NewVersion {
    NewVersion {
        id: version_id,
        account_id,
        worker_id,
        content_kind: open_compute_storage::VersionContentKind::Worker,
        artifact_sha256: Some([1; 32]),
        artifact_size: Some(1),
        artifact_schema_version: Some(1),
        main_module: Some("index.js".to_owned()),
        worker_code_sha256: [2; 32],
        compatibility_date: "2026-09-08".into(),
        compatibility_flags: Vec::new(),
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        request_id: RequestId::generate(),
        now_ms: 12,
    }
}

fn request(
    fixture: &Fixture,
    operation: &str,
    content_type: &str,
    body: Body,
) -> axum::extract::Request {
    axum::extract::Request::builder()
        .method("POST")
        .uri(format!(
            "/internal/bindings/v1/r2/{}/{operation}",
            fixture.binding
        ))
        .header("content-type", content_type)
        .header("x-open-compute-version-id", fixture.version.to_string())
        .header(
            "x-open-compute-descriptor-sha256",
            hex::encode(fixture.descriptor),
        )
        .header(
            "x-open-compute-request-id",
            uuid::Uuid::now_v7().hyphenated().to_string(),
        )
        .body(body)
        .unwrap()
}

fn put_frame(key: &str, bytes: &[u8], options: impl Serialize) -> Body {
    let header = serde_json::to_vec(&serde_json::json!({"key": key, "options": options})).unwrap();
    let mut frame = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(&header);
    frame.extend_from_slice(bytes);
    Body::from(frame)
}

async fn body_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

mod backend_local_capacity_and_framing_failures_release_owned_resources;

mod private_protocol_round_trips_stream_range_metadata_cursor_and_delete;

mod private_protocol_fails_closed_before_mutation_and_releases_cancelled_stream;

mod provider_failures_are_secret_safe_and_mutation_response_loss_is_reconciled;

mod object_authority_reconciles_every_current_put_and_delete_observation;

mod multipart_tests;

mod staging_tests;
