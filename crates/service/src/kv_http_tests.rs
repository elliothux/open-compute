use super::*;
use crate::health::HealthCoordinator;
use crate::http;
use crate::kv_backend::SqliteKvBindingExecutor;
use crate::metrics::MetricsRegistry;
use axum::body::to_bytes;
use axum::http::Request;
use open_compute_artifacts::{
    Fault, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{PlatformConfig, ResourceAvailability, SecretString, SystemClock};
use open_compute_storage::{KvBackupState, KvPutOptions, ResourceRepository};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt as _;

struct Fixture {
    _temp: TempDir,
    _mock: MockS3,
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    router: Router,
    account: AccountId,
}

async fn fixture() -> Fixture {
    fixture_with_resource_limit(1_000).await
}

async fn fixture_with_resource_limit(max_resources_per_account: u32) -> Fixture {
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
    let executor = Arc::new(SqliteKvBindingExecutor::new(
        storage.clone(),
        Arc::new(SystemClock),
    ));
    let api = KvApiState::new(
        storage.clone(),
        ArtifactStore::new(client),
        pins.clone(),
        executor,
        KvConfig {
            namespace_quota_bytes: 256 * 1024 * 1024,
            ..KvConfig::default()
        },
        max_resources_per_account,
        Duration::from_millis(10),
    );
    assert!(format!("{api:?}").contains("KvApiState"));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("admin-secret")),
    )
    .with_kv_api(api);
    Fixture {
        _temp: temp,
        _mock: mock,
        storage,
        pins,
        router: http::admin_router(state),
        account,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn request(method: &str, uri: &str, body: Value, key: Option<&str>) -> Request<Body> {
    request_auth(method, uri, &body, key, Some("admin-secret"))
}

fn request_auth(
    method: &str,
    uri: &str,
    body: &Value,
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

async fn response_json(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn create_namespace(fixture: &Fixture, name: &str, key: &str) -> ResourceId {
    let uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces",
        fixture.account
    );
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

#[tokio::test]
async fn namespace_control_crud_replays_and_fences_pinned_delete() {
    let fixture = fixture().await;
    let uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces",
        fixture.account
    );
    let resource = create_namespace(&fixture, "cache", "create-cache").await;

    let replay = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            &uri,
            json!({"name": "cache"}),
            Some("create-cache"),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);

    let (status, listed) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &uri, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["namespaces"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains("storage_key"));

    let item = format!("{uri}/{resource}");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &item, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let renamed = fixture
        .router
        .clone()
        .oneshot(request("PATCH", &item, json!({"name": "renamed"}), None))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);

    let pin = fixture.pins.try_pin(resource).unwrap();
    let blocked = fixture
        .router
        .clone()
        .oneshot(request("DELETE", &item, Value::Null, Some("delete-cache")))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    drop(pin);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &item,
                Value::Null,
                Some("delete-cache-2"),
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
                &item,
                Value::Null,
                Some("delete-cache-2"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let recreated = create_namespace(&fixture, "renamed", "recreate-cache").await;
    assert_ne!(recreated, resource);
}

#[tokio::test]
async fn online_backup_restore_and_retention_round_trip_namespace_data() {
    let fixture = fixture_with_resource_limit(1).await;
    let source = create_namespace(&fixture, "source", "create-source").await;
    let record = KvNamespaceRepository::new(fixture.storage.db())
        .get(fixture.account, source)
        .unwrap();
    let database = KvPaths::open(fixture.storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, fixture.account, source)
        .unwrap();
    KvEngine::from_record(database, &record)
        .unwrap()
        .put(
            "hello",
            b"world",
            &KvPutOptions {
                expires_at_ms: Some(i64::MAX),
                metadata_json: Some(br#"{"kind":"backup"}"#.to_vec()),
            },
            1_000,
        )
        .unwrap();

    let backup_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces/{source}/backups",
        fixture.account
    );
    let (status, backup_body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backup_uri,
                Value::Null,
                Some("backup-source"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let backup_id = backup_body["backup"]["id"].as_str().unwrap().to_owned();
    let (status, replayed_backup) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backup_uri,
                Value::Null,
                Some("backup-source"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed_backup["backup"]["id"], backup_id);

    let list_uri = format!("/operator/api/v1/accounts/{}/kv/backups", fixture.account);
    let (status, backups) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &list_uri, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(backups["backups"].as_array().unwrap().len(), 1);
    assert!(!backups.to_string().contains("objectKey"));

    let source_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces/{source}",
        fixture.account
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &source_uri,
                Value::Null,
                Some("delete-source"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );

    let restore_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces:restore",
        fixture.account
    );
    let first = fixture.router.clone().oneshot(request(
        "POST",
        &restore_uri,
        json!({"backupId": backup_id, "newName": "restored-a"}),
        Some("restore-source-a"),
    ));
    let second = fixture.router.clone().oneshot(request(
        "POST",
        &restore_uri,
        json!({"backupId": backup_id, "newName": "restored-b"}),
        Some("restore-source-b"),
    ));
    let (first, second) = tokio::join!(first, second);
    let first = response_json(first.unwrap()).await;
    let second = response_json(second.unwrap()).await;
    let (restored_body, restored_name, restore_key) = match (first, second) {
        ((StatusCode::CREATED, body), (StatusCode::TOO_MANY_REQUESTS, _)) => {
            (body, "restored-a", "restore-source-a")
        }
        ((StatusCode::TOO_MANY_REQUESTS, _), (StatusCode::CREATED, body)) => {
            (body, "restored-b", "restore-source-b")
        }
        statuses => panic!("one concurrent restore must win the only quota slot: {statuses:?}"),
    };
    let restored: ResourceId = restored_body["resourceId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(restored, source);
    let record = KvNamespaceRepository::new(fixture.storage.db())
        .get(fixture.account, restored)
        .unwrap();
    let database = KvPaths::open(fixture.storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, fixture.account, restored)
        .unwrap();
    let restored_engine = KvEngine::from_record(database, &record).unwrap();
    let entry = restored_engine.get("hello", 1_000).unwrap().unwrap();
    assert_eq!(entry.value, b"world");
    assert_eq!(
        entry.metadata_json.as_deref(),
        Some(br#"{"kind":"backup"}"#.as_slice())
    );
    restored_engine
        .put("after-restore", b"keep", &KvPutOptions::default(), 2_000)
        .unwrap();
    let (status, replayed_restore) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore_uri,
                json!({"backupId": backup_id, "newName": restored_name}),
                Some(restore_key),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed_restore["resourceId"], restored.to_string());
    let backup_staging_entries = std::fs::read_dir(fixture.storage.data_dir().backup_staging_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(
        backup_staging_entries.is_empty(),
        "restore left backup staging entries: {backup_staging_entries:?}"
    );
    assert_eq!(
        restored_engine
            .get("after-restore", 2_001)
            .unwrap()
            .unwrap()
            .value,
        b"keep"
    );

    let delete_backup_uri = format!("{list_uri}/{backup_id}");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &delete_backup_uri,
                Value::Null,
                Some("delete-backup"),
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
                &delete_backup_uri,
                Value::Null,
                Some("delete-backup"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn control_validation_and_s3_failure_are_sanitized() {
    let fixture = fixture().await;
    let uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces",
        fixture.account
    );
    for request in [
        request("POST", &uri, json!({"name": "missing-key"}), None),
        request("POST", &uri, json!({"unknown": true}), Some("invalid-body")),
        request(
            "POST",
            "/operator/api/v1/accounts/invalid/kv/namespaces",
            json!({"name": "x"}),
            Some("x"),
        ),
    ] {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    let source = create_namespace(&fixture, "s3-failure", "create-s3-failure").await;
    fixture._mock.set_fault(Fault::ServerError);
    let backup_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces/{source}/backups",
        fixture.account
    );
    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backup_uri,
                Value::Null,
                Some("backup-failure"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("sqlite"));
    let failed = KvNamespaceRepository::new(fixture.storage.db())
        .list_backups(fixture.account)
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].state, KvBackupState::Failed);
    let failed_id = failed[0].id.clone();
    let (replay_status, replay_body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backup_uri,
                Value::Null,
                Some("backup-failure"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(replay_body["error"]["code"], body["error"]["code"]);
    let replayed = KvNamespaceRepository::new(fixture.storage.db())
        .list_backups(fixture.account)
        .unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].id, failed_id);
    let resource = ResourceRepository::new(fixture.storage.db())
        .get(fixture.account, source)
        .unwrap();
    assert_eq!(resource.availability, ResourceAvailability::Healthy);
}

#[tokio::test]
async fn control_auth_not_found_restore_and_error_mapping_boundaries() {
    let fixture = fixture().await;
    let metrics =
        || Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let no_api = http::admin_router(HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        None,
    ));
    let valid_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces",
        fixture.account
    );
    assert_eq!(
        no_api
            .oneshot(request_auth("GET", &valid_uri, &Value::Null, None, None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let protected = http::admin_router(HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    ));
    assert_eq!(
        protected
            .oneshot(request_auth("GET", &valid_uri, &Value::Null, None, None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let protected_create = http::admin_router(HttpState::for_test(
        HealthCoordinator::new(),
        metrics(),
        false,
        Some(SecretString::new("admin-secret")),
    ));
    assert_eq!(
        protected_create
            .oneshot(request_auth(
                "POST",
                &valid_uri,
                &json!({"name": "x"}),
                Some("unauthorized-create"),
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &valid_uri,
                json!({"name": ""}),
                Some("invalid-name"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );

    let missing = ResourceId::generate();
    let invalid_requests = [
        request(
            "GET",
            "/operator/api/v1/accounts/invalid/kv/namespaces",
            Value::Null,
            None,
        ),
        request(
            "GET",
            &format!(
                "/operator/api/v1/accounts/{}/kv/namespaces/invalid",
                fixture.account
            ),
            Value::Null,
            None,
        ),
        request(
            "GET",
            &format!(
                "/operator/api/v1/accounts/{}/kv/namespaces/{missing}",
                fixture.account
            ),
            Value::Null,
            None,
        ),
        request(
            "PATCH",
            &format!(
                "/operator/api/v1/accounts/{}/kv/namespaces/invalid",
                fixture.account
            ),
            json!({"name": "x"}),
            None,
        ),
        request(
            "DELETE",
            &format!(
                "/operator/api/v1/accounts/{}/kv/namespaces/{missing}",
                fixture.account
            ),
            Value::Null,
            None,
        ),
        request(
            "POST",
            &format!(
                "/operator/api/v1/accounts/{}/kv/namespaces/invalid/backups",
                fixture.account
            ),
            Value::Null,
            Some("bad-resource"),
        ),
        request(
            "GET",
            "/operator/api/v1/accounts/invalid/kv/backups",
            Value::Null,
            None,
        ),
        request(
            "POST",
            "/operator/api/v1/accounts/invalid/kv/namespaces:restore",
            json!({"backupId": "missing", "newName": "x"}),
            Some("bad-account"),
        ),
        request(
            "POST",
            &format!(
                "/operator/api/v1/accounts/{}/kv/namespaces:restore",
                fixture.account
            ),
            json!({"unknown": true}),
            Some("bad-restore-body"),
        ),
        request(
            "DELETE",
            &format!(
                "/operator/api/v1/accounts/{}/kv/backups/missing",
                fixture.account
            ),
            Value::Null,
            None,
        ),
    ];
    for request in invalid_requests {
        let status = fixture
            .router
            .clone()
            .oneshot(request)
            .await
            .unwrap()
            .status();
        assert!(matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ));
    }

    let restore_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces:restore",
        fixture.account
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore_uri,
                json!({"backupId": "missing", "newName": "x"}),
                Some("missing-backup"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    let source = create_namespace(&fixture, "not-ready", "create-not-ready").await;
    let creating_id = uuid::Uuid::now_v7().hyphenated().to_string();
    KvNamespaceRepository::new(fixture.storage.db())
        .create_backup(source, &creating_id, 1, "not-ready", &[4; 32], 10)
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore_uri,
                json!({"backupId": creating_id, "newName": "x"}),
                Some("not-ready-backup"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let backup_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces/{source}/backups",
        fixture.account
    );
    let (_, backup) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &backup_uri,
                Value::Null,
                Some("restore-download-failure"),
            ))
            .await
            .unwrap(),
    )
    .await;
    fixture._mock.set_fault(Fault::ServerError);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &restore_uri,
                json!({
                    "backupId": backup["backup"]["id"],
                    "newName": "download-failure"
                }),
                Some("restore-download-failure"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    let oversized = Request::builder()
        .body(Body::from(vec![b'x'; MAX_JSON_BODY + 1]))
        .unwrap();
    let Err(too_large) = read_json::<CreateNamespaceBody>(oversized).await else {
        panic!("oversized control body was accepted")
    };
    assert_eq!(too_large.code(), ErrorCode::LimitInvalid);
    for (code, status) in [
        (ErrorCode::ResourceNameConflict, StatusCode::CONFLICT),
        (ErrorCode::KvBusy, StatusCode::SERVICE_UNAVAILABLE),
        (ErrorCode::KvCorrupt, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        assert_eq!(
            error_response(PlatformError::new(code, "test"), RequestId::generate()).status(),
            status
        );
    }
    assert_eq!(
        hash_file(fixture._temp.path()).unwrap_err().code(),
        ErrorCode::Internal
    );
}

#[tokio::test]
async fn operator_value_put_get_and_delete_round_trip() {
    let fixture = fixture().await;
    let resource = create_namespace(&fixture, "values", "create-values").await;
    let value_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces/{resource}/values/hello",
        fixture.account
    );

    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "PUT",
                &value_uri,
                json!({"value": "world", "metadata": {"source": "operator"}, "expirationTtl": 60}),
                Some("put-hello"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &value_uri, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["value"], "world");

    let keys_uri = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces/{resource}/keys?prefix=h&limit=1",
        fixture.account
    );
    let (status, keys) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &keys_uri, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys["keys"][0]["name"], "hello");
    assert_eq!(keys["keys"][0]["metadata"]["source"], "operator");

    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &value_uri,
                Value::Null,
                Some("delete-hello")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &value_uri, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["value"], Value::Null);

    assert!(operator_kv_value_put_path(&value_uri));
    assert!(!operator_kv_value_put_path(
        "/operator/api/v1/accounts/x/kv/namespaces/y/values/"
    ));
}

#[tokio::test]
async fn operator_namespace_catalog_filters_sorts_and_paginates_deterministically() {
    let fixture = fixture().await;
    for (name, key) in [
        ("alpha", "catalog-alpha"),
        ("beta", "catalog-beta"),
        ("gamma", "catalog-gamma"),
    ] {
        create_namespace(&fixture, name, key).await;
    }
    let base = format!(
        "/operator/api/v1/accounts/{}/kv/namespaces",
        fixture.account
    );
    let (status, first) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{base}?sort=name&direction=asc&limit=1"),
                Value::Null,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["namespaces"][0]["resource"]["name"], "alpha");
    assert_eq!(first["listComplete"], false);
    let cursor = first["cursor"].as_str().unwrap();

    let (status, second) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{base}?sort=name&direction=asc&limit=1&cursor={cursor}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["namespaces"][0]["resource"]["name"], "beta");

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
                    Value::Null,
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
                    Value::Null,
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
