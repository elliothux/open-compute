use super::*;
use axum::body::to_bytes;
use axum::http::StatusCode;
use open_compute_artifacts::{
    MapEnv, MockS3, R2SsecKey, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{
    AccountId, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, RequestId, SystemClock,
    WorkerId,
};
use open_compute_core::{BindingId, BindingKind};
use open_compute_storage::{
    NewDeployment, NewDeploymentBinding, R2BucketRepository, R2MultipartPartRecord,
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
    deployment: DeploymentId,
    descriptor: [u8; 32],
    resource: ResourceId,
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
        R2ObjectStore::new(S3ArtifactClient::connect(&config, &credentials, 1024 * 1024).unwrap());
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
    let deployment = DeploymentId::generate();
    let binding = BindingId::generate();
    let descriptor = [9_u8; 32];
    workers
        .insert_staging_deployment(
            &deployment_input(account, worker.id, deployment),
            &open_compute_storage::NewDeploymentProducts {
                bindings: &[NewDeploymentBinding {
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
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 13).unwrap();
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
        deployment,
        descriptor,
        resource,
    }
}

fn deployment_input(
    account_id: AccountId,
    worker_id: WorkerId,
    deployment_id: DeploymentId,
) -> NewDeployment {
    NewDeployment {
        id: deployment_id,
        account_id,
        worker_id,
        content_kind: open_compute_storage::DeploymentContentKind::Worker,
        artifact_sha256: Some([1; 32]),
        artifact_size: Some(1),
        artifact_schema_version: Some(1),
        main_module: Some("index.js".to_owned()),
        worker_code_sha256: [2; 32],
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
        .header(
            "x-open-compute-deployment-id",
            fixture.deployment.to_string(),
        )
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

#[tokio::test]
async fn backend_local_capacity_and_framing_failures_release_owned_resources() {
    let fixture = fixture().await;
    assert!(format!("{:?}", fixture.service).contains("R2BindingService"));

    let wrong_method = axum::extract::Request::builder()
        .method("GET")
        .uri(format!("/internal/bindings/v1/r2/{}/head", fixture.binding))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        fixture.service.handle(wrong_method).await.status(),
        StatusCode::BAD_REQUEST
    );

    let oversized_header = Body::from(
        u32::try_from(MAX_METADATA_BYTES + 1)
            .unwrap()
            .to_be_bytes()
            .to_vec(),
    );
    let response = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            oversized_header,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture.pins.count(fixture.resource), 0);

    let response = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("large", &vec![0; 1024 * 1024 + 1], serde_json::json!({})),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(fixture.pins.count(fixture.resource), 0);

    let used = Arc::new(AtomicU64::new(0));
    let mut reservation = StagingReservation::new(used.clone(), 1, None);
    reservation.add(1).unwrap();
    assert_eq!(
        reservation.add(1).unwrap_err().code(),
        ErrorCode::R2Overloaded
    );
    drop(reservation);
    assert_eq!(used.load(Ordering::Acquire), 0);

    let gate = OperationGate::new(1);
    let held = gate
        .acquire(fixture.resource, Duration::from_secs(1))
        .await
        .unwrap();
    let saturated = gate
        .acquire(fixture.resource, Duration::from_millis(1))
        .await;
    assert!(matches!(saturated, Err(ref error) if error.code() == ErrorCode::R2Overloaded));
    drop(held);

    let seeded = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("corrupt", b"body", serde_json::json!({})),
        ))
        .await;
    assert_eq!(seeded.status(), StatusCode::OK);
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::CorruptMetadata);
    let response = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(serde_json::json!({"key": "corrupt"}).to_string()),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn private_protocol_round_trips_stream_range_metadata_cursor_and_delete() {
    let fixture = fixture().await;
    let put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "folder/a + %.txt",
                b"hello world",
                serde_json::json!({
                    "httpMetadata": {"contentType": "text/plain"},
                    "customMetadata": {"author": "Elliot"},
                    "checksum": {
                        "algorithm": "md5",
                        "hex": "5eb63bbbe01eeed093cb22bb8f5acdc3"
                    },
                    "storageClass": "Standard"
                }),
            ),
        ))
        .await;
    assert_eq!(put.status(), StatusCode::OK);
    let put = body_json(put).await;
    assert_eq!(put["size"], 11);
    assert_eq!(put["customMetadata"]["author"], "Elliot");
    assert!(
        std::fs::read_dir(fixture.storage.data_dir().root().join("r2-staging"))
            .unwrap()
            .next()
            .is_none()
    );

    let head = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"folder/a + %.txt"}"#),
        ))
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        body_json(head).await["httpMetadata"]["contentType"],
        "text/plain"
    );

    let get = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(r#"{"key":"folder/a + %.txt","options":{"range":{"offset":6,"length":5}}}"#),
        ))
        .await;
    assert_eq!(fixture.pins.count(fixture.resource), 1);
    let frame = to_bytes(get.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(fixture.pins.count(fixture.resource), 0);
    let header_len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    let metadata: serde_json::Value = serde_json::from_slice(&frame[4..4 + header_len]).unwrap();
    assert_eq!(metadata["meta"]["size"], 11);
    assert_eq!(metadata["meta"]["range"]["length"], 5);
    assert_eq!(&frame[4 + header_len..], b"world");

    let second = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("folder/b", b"second", serde_json::json!({})),
        ))
        .await;
    assert_eq!(second.status(), StatusCode::OK);
    let list = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"prefix":"folder/","limit":1,"include":[]}"#),
        ))
        .await;
    let list = body_json(list).await;
    assert!(list["truncated"].as_bool().unwrap());
    let cursor = list["cursor"].as_str().unwrap();
    assert!(!cursor.contains("folder"));
    let next = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "prefix": "folder/",
                    "limit": 1,
                    "include": [],
                    "cursor": cursor
                }))
                .unwrap(),
            ),
        ))
        .await;
    assert_eq!(next.status(), StatusCode::OK);

    let tampered = format!("{cursor}x");
    let invalid = fixture
        .service
        .handle(request(
            &fixture,
            "list",
            JSON_CONTENT_TYPE,
            Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "prefix": "folder/",
                    "limit": 1,
                    "include": [],
                    "cursor": tampered
                }))
                .unwrap(),
            ),
        ))
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2CursorInvalid.as_str()
    );

    let deleted = fixture
        .service
        .handle(request(
            &fixture,
            "delete",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"keys":["folder/a + %.txt","folder/b"]}"#),
        ))
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn private_protocol_fails_closed_before_mutation_and_releases_cancelled_stream() {
    let fixture = fixture().await;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(
            fixture.storage.identity().default_account_id,
            fixture.resource,
        )
        .unwrap();
    fixture.mock.put_raw(
        &format!(
            "{}objects/{}",
            bucket.physical_prefix,
            hex::encode(Sha256::digest(b"provider-only"))
        ),
        b"unowned".to_vec(),
    );
    for _ in 0..2 {
        let provider_only = fixture
            .service
            .handle(request(
                &fixture,
                "head",
                JSON_CONTENT_TYPE,
                Body::from(r#"{"key":"provider-only"}"#),
            ))
            .await;
        assert_eq!(provider_only.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            provider_only.headers().get(ERROR_HEADER).unwrap(),
            ErrorCode::R2ObjectMetadataInvalid.as_str()
        );
    }
    let bad_md5 = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("key", b"value", serde_json::json!({"md5": [0, 1]})),
        ))
        .await;
    assert_eq!(bad_md5.status(), StatusCode::BAD_REQUEST);

    let wrong_descriptor = axum::extract::Request::builder()
        .method("POST")
        .uri(format!("/internal/bindings/v1/r2/{}/head", fixture.binding))
        .header("content-type", JSON_CONTENT_TYPE)
        .header(
            "x-open-compute-deployment-id",
            fixture.deployment.to_string(),
        )
        .header("x-open-compute-descriptor-sha256", "00".repeat(32))
        .header(
            "x-open-compute-request-id",
            uuid::Uuid::now_v7().hyphenated().to_string(),
        )
        .body(Body::from(r#"{"key":"key"}"#))
        .unwrap();
    let rejected = fixture.service.handle(wrong_descriptor).await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(fixture.pins.count(fixture.resource), 0);
}

#[tokio::test]
async fn provider_failures_are_secret_safe_and_mutation_response_loss_is_reconciled() {
    let fixture = fixture().await;
    let seeded = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("failure-key", b"abcdef", serde_json::json!({})),
        ))
        .await;
    assert_eq!(seeded.status(), StatusCode::OK);

    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::MidstreamReset);
    let interrupted = fixture
        .service
        .handle(request(
            &fixture,
            "get",
            FRAME_CONTENT_TYPE,
            Body::from(r#"{"key":"failure-key","options":{}}"#),
        ))
        .await;
    assert_eq!(interrupted.status(), StatusCode::OK);
    assert!(
        to_bytes(interrupted.into_body(), 1024 * 1024)
            .await
            .is_err()
    );
    assert_eq!(fixture.pins.count(fixture.resource), 0);

    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::PutResponseLoss);
    let unknown_put = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("lost-put", b"value", serde_json::json!({})),
        ))
        .await;
    assert_eq!(unknown_put.status(), StatusCode::OK);
    let body = to_bytes(unknown_put.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["key"], "lost-put");

    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    let for_delete = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("lost-delete", b"value", serde_json::json!({})),
        ))
        .await;
    assert_eq!(for_delete.status(), StatusCode::OK);
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::DeleteResponseLoss);
    let recovered_delete = fixture
        .service
        .handle(request(
            &fixture,
            "delete",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"keys":["lost-delete"]}"#),
        ))
        .await;
    assert_eq!(recovered_delete.status(), StatusCode::NO_CONTENT);
    let deleted = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"lost-delete"}"#),
        ))
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    fixture.mock.set_fault(open_compute_artifacts::Fault::Auth);
    let unavailable = fixture
        .service
        .handle(request(
            &fixture,
            "head",
            JSON_CONTENT_TYPE,
            Body::from(r#"{"key":"failure-key"}"#),
        ))
        .await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        unavailable.headers().get(ERROR_HEADER).unwrap(),
        ErrorCode::R2ProviderUnavailable.as_str()
    );
}

#[tokio::test]
async fn object_authority_reconciles_every_current_put_and_delete_observation() {
    let fixture = fixture().await;
    let account = fixture.storage.identity().default_account_id;
    let bucket = R2BucketRepository::new(fixture.storage.db())
        .get(account, fixture.resource)
        .unwrap();
    let locator = fixture
        .objects
        .locator(bucket.resource.id, &bucket.physical_prefix)
        .unwrap();
    let binding = BindingRepository::new(fixture.storage.db())
        .authorize(fixture.binding, fixture.deployment, &fixture.descriptor)
        .unwrap();
    let repo = R2ObjectRepository::new(fixture.storage.db());
    let timeout = Duration::from_secs(1);

    let absent = UserObjectKey::parse("authority-absent").unwrap();
    assert!(
        fixture
            .service
            .committed_object(&binding, &locator, &absent, timeout)
            .await
            .unwrap()
            .is_none()
    );
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &absent, timeout)
        .await
        .unwrap();
    let pending_absent = R2ObjectRecord {
        resource_id: fixture.resource,
        account_id: account,
        object_key: absent.as_str().to_owned(),
        object_version: uuid::Uuid::now_v7().to_string(),
        ssec_key_md5: None,
        ssec_envelope: None,
    };
    repo.begin_put(&pending_absent, 20).unwrap();
    assert_eq!(
        fixture
            .service
            .ensure_no_object_mutation(&binding, &absent)
            .unwrap_err()
            .code(),
        ErrorCode::R2ProviderUnavailable
    );
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &absent, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, absent.as_str())
            .unwrap()
            .is_none()
    );

    let seeded = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame("authority-existing", b"body", serde_json::json!({})),
        ))
        .await;
    assert_eq!(seeded.status(), StatusCode::OK);
    let key = UserObjectKey::parse("authority-existing").unwrap();
    let committed = repo
        .get(account, fixture.resource, key.as_str())
        .unwrap()
        .unwrap();
    let metadata = fixture
        .objects
        .head(&locator, &key, None)
        .await
        .unwrap()
        .unwrap();
    fixture
        .service
        .finish_object_put(&binding, &key, &metadata)
        .unwrap();
    let mut wrong = metadata.clone();
    wrong.key = "wrong".to_owned();
    assert_eq!(
        fixture
            .service
            .finish_object_put(&binding, &key, &wrong)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    assert_eq!(
        objects::validate_object_record(&committed, &wrong)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );

    fixture
        .service
        .begin_object_put(&binding, &key, &uuid::Uuid::now_v7().to_string(), None)
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, key.as_str())
            .unwrap()
            .is_none()
    );

    let encrypted = fixture
        .service
        .handle(request(
            &fixture,
            "put",
            FRAME_CONTENT_TYPE,
            put_frame(
                "authority-encrypted",
                b"secret",
                serde_json::json!({"ssecKey": "cd".repeat(32)}),
            ),
        ))
        .await;
    assert_eq!(encrypted.status(), StatusCode::OK);
    let encrypted_key = UserObjectKey::parse("authority-encrypted").unwrap();
    let replacement_ssec = R2SsecKey::parse_hex(&"ef".repeat(32)).unwrap();
    fixture
        .service
        .begin_object_put(
            &binding,
            &encrypted_key,
            &uuid::Uuid::now_v7().to_string(),
            Some(&replacement_ssec),
        )
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &encrypted_key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, encrypted_key.as_str())
            .unwrap()
            .is_none()
    );

    fixture
        .service
        .begin_object_put(
            &binding,
            &encrypted_key,
            &uuid::Uuid::now_v7().to_string(),
            Some(&replacement_ssec),
        )
        .unwrap();
    fixture
        .mock
        .set_fault(open_compute_artifacts::Fault::ServerError);
    assert_eq!(
        fixture
            .service
            .reconcile_object_key(&binding, &locator, &encrypted_key, timeout)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ProviderUnavailable
    );
    fixture.mock.set_fault(open_compute_artifacts::Fault::None);
    repo.cancel_put(account, fixture.resource, encrypted_key.as_str())
        .unwrap();

    let ssec = R2SsecKey::parse_hex(&"ab".repeat(32)).unwrap();
    fixture
        .service
        .begin_object_put(
            &binding,
            &key,
            &uuid::Uuid::now_v7().to_string(),
            Some(&ssec),
        )
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, key.as_str())
            .unwrap()
            .is_none()
    );

    repo.begin_delete(account, fixture.resource, &[key.as_str().to_owned()], 21)
        .unwrap();
    fixture
        .service
        .reconcile_object_key(&binding, &locator, &key, timeout)
        .await
        .unwrap();
    assert!(
        repo.get_mutation(account, fixture.resource, key.as_str())
            .unwrap()
            .is_none()
    );
    assert!(
        repo.get(account, fixture.resource, key.as_str())
            .unwrap()
            .is_some()
    );

    fixture
        .service
        .begin_object_put(&binding, &key, &uuid::Uuid::now_v7().to_string(), None)
        .unwrap();
    fixture
        .objects
        .delete(&locator, std::slice::from_ref(&key))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .reconcile_object_key(&binding, &locator, &key, timeout)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    repo.cancel_put(account, fixture.resource, key.as_str())
        .unwrap();

    let inconsistent = R2ObjectRecord {
        ssec_key_md5: Some(ssec.md5_base64()),
        ..committed.clone()
    };
    assert_eq!(
        objects::open_object_ssec(&fixture.storage, &inconsistent)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );
    let (sealed_md5, sealed) =
        objects::seal_object_ssec(&fixture.storage, &binding, "sealed-version", Some(&ssec))
            .unwrap();
    let wrong_md5 = R2ObjectRecord {
        object_version: "sealed-version".to_owned(),
        ssec_key_md5: sealed_md5.map(|_| "wrong".to_owned()),
        ssec_envelope: sealed,
        ..committed
    };
    assert_eq!(
        objects::open_object_ssec(&fixture.storage, &wrong_md5)
            .unwrap_err()
            .code(),
        ErrorCode::R2ObjectMetadataInvalid
    );

    let batch = UserObjectKey::parse("authority-batch").unwrap();
    repo.begin_put(
        &R2ObjectRecord {
            resource_id: fixture.resource,
            account_id: account,
            object_key: batch.as_str().to_owned(),
            object_version: uuid::Uuid::now_v7().to_string(),
            ssec_key_md5: None,
            ssec_envelope: None,
        },
        22,
    )
    .unwrap();
    assert_eq!(
        objects::reconcile_bucket_objects(&fixture.storage, &fixture.objects, &bucket, timeout)
            .await
            .unwrap(),
        1
    );
}

#[path = "r2_backend_tests/multipart.rs"]
mod multipart_tests;
