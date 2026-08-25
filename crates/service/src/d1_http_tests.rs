use super::*;
use crate::health::HealthCoordinator;
use crate::http;
use crate::metrics::MetricsRegistry;
use axum::body::to_bytes;
use axum::http::Request;
use open_compute_artifacts::{MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with};
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{D1Config, PlatformConfig, SystemClock};
use open_compute_storage::{D1QueryLimits, D1Statement, D1Value};
use serde_json::{Value, json};
use sha2::Digest as _;
use tempfile::TempDir;
use tower::ServiceExt as _;

struct Fixture {
    _temp: TempDir,
    _mock: MockS3,
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
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
                free_space_hard_bytes: 268_435_456,
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
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 1
connect_timeout_ms = 100
request_timeout_ms = 500
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&s3, &env).unwrap();
    let client = S3ArtifactClient::connect(&s3, &credentials, 512 * 1024).unwrap();
    let pins = ResourcePins::new();
    let config = D1Config {
        database_quota_bytes: 256 * 1024 * 1024,
        ..D1Config::default()
    };
    let backend = Arc::new(D1BindingService::new(
        storage.clone(),
        pins.clone(),
        config.clone(),
    ));
    let api = D1ApiState::new(
        storage.clone(),
        ArtifactStore::new(client),
        pins.clone(),
        backend,
        config,
        Duration::from_millis(10),
    );
    assert_eq!(api.reconcile_pending().await.unwrap(), 0);
    assert!(format!("{api:?}").contains("D1ApiState"));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let state = HttpState::for_test(HealthCoordinator::new(), metrics.clone(), false, None)
        .with_d1_api(api);
    Fixture {
        _temp: temp,
        _mock: mock,
        storage,
        pins,
        router: http::admin_router(state),
        account,
        metrics,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn request(method: &str, uri: &str, body: Value, key: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_HEADER, key);
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn response_json(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn create_database(fixture: &Fixture, name: &str, key: &str) -> ResourceId {
    let uri = format!("/v1/accounts/{}/d1/databases", fixture.account);
    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("POST", &uri, json!({"name": name}), Some(key)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["resourceId"].as_str().unwrap().parse().unwrap()
}

fn engine(fixture: &Fixture, resource: ResourceId) -> D1Engine {
    let record = D1DatabaseRepository::new(fixture.storage.db())
        .get(fixture.account, resource)
        .unwrap();
    let path = D1Paths::open(fixture.storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, fixture.account, resource)
        .unwrap();
    D1Engine::from_record(path, &record).unwrap()
}

#[tokio::test]
async fn database_migration_backup_restore_and_delete_round_trip() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/d1/databases", fixture.account);
    let source = create_database(&fixture, "primary", "create-primary").await;
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({"name": "primary"}),
                Some("create-primary"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let (_, listed) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &collection, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["databases"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains("storageKey"));
    let source_uri = format!("{collection}/{source}");
    let (status, fetched) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &source_uri, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["database"]["resource"]["name"], "primary");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "PATCH",
                &source_uri,
                json!({"name": "renamed"}),
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let sql = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT); PRAGMA user_version = 1";
    let digest = hex::encode(sha2::Sha256::digest(sql.as_bytes()));
    let migrations = format!("{collection}/{source}/migrations/apply");
    let migration_body = json!({"migrations": [{
        "id": 1, "name": "0001_notes.sql", "sha256": digest, "sql": sql
    }]});
    let (status, applied) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &migrations,
                migration_body.clone(),
                Some("migrate-notes"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(applied["migrations"].as_array().unwrap().len(), 1);
    let migration_list = format!("{collection}/{source}/migrations");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &migration_list, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &migrations,
                migration_body,
                Some("migrate-notes"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let source_engine = engine(&fixture, source);
    source_engine
        .exec(
            "INSERT INTO notes(body) VALUES ('kept')",
            D1QueryLimits::query(&D1Config::default()).unwrap(),
        )
        .unwrap();

    let backups = format!("{collection}/{source}/backups");
    let (status, backup_body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backups,
                Value::Null,
                Some("backup-primary"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let backup_id = backup_body["backup"]["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backups,
                Value::Null,
                Some("backup-primary"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let (_, backup_list) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &backups, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(backup_list["backups"].as_array().unwrap().len(), 1);
    assert!(!backup_list.to_string().contains("objectKey"));

    let restore = format!("{collection}:restore");
    let (status, restored_body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore,
                json!({"backupId": backup_id, "newName": "restored"}),
                Some("restore-primary"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore,
                json!({"backupId": backup_id, "newName": "restored"}),
                Some("restore-primary"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let restored: ResourceId = restored_body["resourceId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(restored, source);
    let restored_engine = engine(&fixture, restored);
    let result = restored_engine
        .query(
            &D1Statement {
                sql: "SELECT body FROM notes".to_owned(),
                params: vec![],
            },
            D1QueryLimits::query(&D1Config::default()).unwrap(),
        )
        .unwrap();
    assert_eq!(result.rows, vec![vec![D1Value::Text("kept".to_owned())]]);
    assert_eq!(restored_engine.migrations().unwrap().len(), 1);

    let restored_record = D1DatabaseRepository::new(fixture.storage.db())
        .get(fixture.account, restored)
        .unwrap();
    let restored_path = D1Paths::open(fixture.storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&restored_record.storage_key, fixture.account, restored)
        .unwrap();
    let connection = rusqlite::Connection::open(restored_path).unwrap();
    connection
        .execute(
            "UPDATE __open_compute_meta SET value = ?1 WHERE key = 'resource_id'",
            [b"wrong".as_slice()],
        )
        .unwrap();
    drop(connection);
    let corrupt_migrations = format!("{collection}/{restored}/migrations");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &corrupt_migrations, Value::Null, None,))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        ResourceRepository::new(fixture.storage.db())
            .get(fixture.account, restored)
            .unwrap()
            .availability,
        open_compute_core::ResourceAvailability::Unavailable
    );

    let pin = fixture.pins.try_pin(source).unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &source_uri,
                Value::Null,
                Some("delete-one")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    drop(pin);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &source_uri,
                Value::Null,
                Some("delete-two")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &source_uri,
                Value::Null,
                Some("delete-three"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn control_validation_is_bounded_and_sanitized() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/d1/databases", fixture.account);
    for input in [
        request("POST", &collection, json!({"name": "missing-key"}), None),
        request(
            "POST",
            &collection,
            json!({"unknown": true}),
            Some("bad-body"),
        ),
        request(
            "POST",
            "/v1/accounts/invalid/d1/databases",
            json!({"name": "x"}),
            Some("bad-account"),
        ),
    ] {
        let (status, body) =
            response_json(fixture.router.clone().oneshot(input).await.unwrap()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            !body
                .to_string()
                .contains(fixture.storage.data_dir().root().to_str().unwrap())
        );
    }

    let missing = ResourceId::generate();
    for input in [
        request("GET", &format!("{collection}/{missing}"), Value::Null, None),
        request(
            "PATCH",
            &format!("{collection}/{missing}"),
            json!({"name": "missing"}),
            None,
        ),
        request(
            "DELETE",
            &format!("{collection}/{missing}"),
            Value::Null,
            Some("delete-missing"),
        ),
        request(
            "GET",
            &format!("{collection}/{missing}/migrations"),
            Value::Null,
            None,
        ),
        request(
            "GET",
            "/v1/accounts/invalid/d1/databases/not-an-id",
            Value::Null,
            None,
        ),
        request(
            "POST",
            &format!("{collection}/{missing}/backups"),
            Value::Null,
            Some("missing-backup"),
        ),
        request(
            "GET",
            &format!("{collection}/{missing}/backups"),
            Value::Null,
            None,
        ),
        request(
            "POST",
            &format!("{collection}:restore"),
            json!({"backupId": "missing", "newName": "missing"}),
            Some("missing-restore"),
        ),
        request(
            "POST",
            "/v1/accounts/invalid/d1/databases:restore",
            Value::Null,
            Some("invalid-account-restore"),
        ),
        request(
            "POST",
            &format!("{collection}:restore"),
            json!({"unknown": true}),
            Some("invalid-body-restore"),
        ),
    ] {
        let response = fixture.router.clone().oneshot(input).await.unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ));
    }

    let no_api = HttpState::for_test(
        HealthCoordinator::new(),
        fixture.metrics.clone(),
        false,
        None,
    );
    let unavailable = request("GET", &collection, Value::Null, None);
    assert!(authorized_api(&no_api, &unavailable).is_none());
    assert_eq!(
        unauthorized_or_unavailable(&no_api, &unavailable, RequestId::generate()).status(),
        StatusCode::NOT_FOUND
    );

    let oversized = Request::builder()
        .body(Body::from(vec![b'x'; MAX_JSON_BODY + 1]))
        .unwrap();
    assert_eq!(
        read_json::<CreateDatabaseBody>(oversized)
            .await
            .err()
            .unwrap()
            .code(),
        ErrorCode::LimitInvalid
    );
    let mut invalid_key = request("POST", &collection, Value::Null, Some("has space"));
    assert_eq!(
        idempotency_key(&invalid_key).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    let known_request = RequestId::generate();
    invalid_key.extensions_mut().insert(known_request);
    assert_eq!(request_id(&invalid_key), known_request);
    assert!(now_ms() > 0);
}

#[tokio::test]
async fn migration_idempotency_failure_matrix_is_stable() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/d1/databases", fixture.account);
    let resource = create_database(&fixture, "migration-failures", "migration-failures").await;
    let uri = format!("{collection}/{resource}/migrations/apply");

    let bad_digest = json!({"migrations": [{
        "id": 1, "name": "bad.sql", "sha256": "bad", "sql": "SELECT 1"
    }]});
    for _ in 0..2 {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "POST",
                    &uri,
                    bad_digest.clone(),
                    Some("bad-digest"),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }

    let denied_sql = "ATTACH DATABASE ':memory:' AS other";
    let denied = json!({"migrations": [{
        "id": 1,
        "name": "denied.sql",
        "sha256": hex::encode(sha2::Sha256::digest(denied_sql.as_bytes())),
        "sql": denied_sql
    }]});
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("POST", &uri, denied, Some("denied-migration")))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let running_body = json!({"migrations": [{
        "id": 1,
        "name": "running.sql",
        "sha256": hex::encode(sha2::Sha256::digest(b"CREATE TABLE running(id INTEGER)")),
        "sql": "CREATE TABLE running(id INTEGER)"
    }]});
    let canonical = serde_json::to_vec(&running_body).unwrap();
    let fingerprint = fixture.storage.crypto().fingerprint_request(&canonical);
    WorkerRepository::new(fixture.storage.db())
        .reserve_idempotency(
            fixture.account,
            &format!("d1-migrations:{resource}"),
            "running-migration",
            fixture.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            now_ms(),
            now_ms() + 1_000,
        )
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &uri,
                running_body,
                Some("running-migration"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn backup_failure_replay_and_corrupt_restore_fail_closed() {
    let fixture = fixture().await;
    let source = create_database(&fixture, "backup-faults", "create-backup-faults").await;
    engine(&fixture, source)
        .exec(
            "CREATE TABLE data(value TEXT)",
            D1QueryLimits::query(&D1Config::default()).unwrap(),
        )
        .unwrap();
    let backup_uri = format!(
        "/v1/accounts/{}/d1/databases/{source}/backups",
        fixture.account
    );

    fixture
        ._mock
        .set_fault(open_compute_artifacts::Fault::ServerError);
    for _ in 0..2 {
        let (status, body) = response_json(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "POST",
                    &backup_uri,
                    Value::Null,
                    Some("failed-backup"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "S3_UNAVAILABLE");
    }
    let failed = D1DatabaseRepository::new(fixture.storage.db())
        .list_backups(fixture.account, source)
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].state, D1BackupState::Failed);
    let restore_uri = format!("/v1/accounts/{}/d1/databases:restore", fixture.account);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore_uri,
                json!({"backupId": failed[0].id.clone(), "newName": "not-ready"}),
                Some("restore-not-ready"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    fixture
        ._mock
        .set_fault(open_compute_artifacts::Fault::PutResponseLoss);
    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backup_uri,
                Value::Null,
                Some("ready-backup"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let backup_id = body["backup"]["id"].as_str().unwrap();

    fixture
        ._mock
        .set_fault(open_compute_artifacts::Fault::CorruptBody);
    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore_uri,
                json!({"backupId": backup_id, "newName": "must-not-publish"}),
                Some("corrupt-restore"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "ARTIFACT_INTEGRITY_ERROR");
    let rendered = fixture
        .metrics
        .render(&open_compute_core::PlatformStatus::starting());
    assert!(rendered.contains("d1_backup_total{outcome=\"failure\"} 2"));
    assert!(rendered.contains("d1_backup_total{outcome=\"success\"} 1"));
}
