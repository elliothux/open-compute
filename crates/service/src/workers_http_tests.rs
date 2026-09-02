use super::*;
use crate::asset_backend::serve_asset_plan;
use axum::body::HttpBody as _;
use bytes::Bytes;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, MockS3, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{MetricsConfig, PlatformConfig, StorageConfig, SystemClock};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{
    NewVersion, PlatformStorage, VersionAssetsRepository, VersionContentKind, WorkerRepository,
};
use open_compute_workers::{CanonicalBundle, ModuleInput, ModuleType};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

pub(super) async fn worker_api_fixture() -> (TempDir, MockS3, WorkerApiState, AccountId) {
    let temp = TempDir::new().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let mock = MockS3::spawn("open-compute").await;
    let config = PlatformConfig::from_toml_str(&format!(
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
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 1500
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
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    let client = S3ArtifactClient::connect(&config, &credentials, 64 * 1024).unwrap();
    let api = WorkerApiState::new(
        storage,
        ArtifactStore::new(client),
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
        VersionPins::new(),
        BundleLimits::default(),
        Duration::from_millis(10),
    );
    (temp, mock, api, account)
}

pub(super) fn authorized_http_state(api: WorkerApiState) -> HttpState {
    HttpState::for_test(
        crate::HealthCoordinator::new(),
        Arc::new(
            crate::MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap(),
        ),
        false,
        Some(SecretString::new("account-admin")),
    )
    .with_worker_api(api)
}

#[tokio::test]
async fn default_account_discovery_is_authenticated_and_uses_persisted_identity() {
    use crate::{HealthCoordinator, MetricsRegistry};
    use open_compute_core::MetricsConfig;
    use tower::ServiceExt;

    let (_temp, _s3, api, account) = worker_api_fixture().await;
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap()),
        false,
        Some(SecretString::new("account-admin")),
    )
    .with_worker_api(api);
    let router = control_router().with_state(state);
    for (token, expected_status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some("wrong"), StatusCode::UNAUTHORIZED),
        (Some("account-admin"), StatusCode::OK),
    ] {
        let mut request = Request::builder().uri("/v1/account");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("account-admin"));
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if expected_status == StatusCode::OK {
            assert_eq!(value, serde_json::json!({ "accountId": account }));
        } else {
            assert!(!String::from_utf8_lossy(&body).contains(&account.to_string()));
        }
    }
}

#[tokio::test]
async fn assets_only_upload_resume_finalize_and_handler_share_one_authority() {
    use tower::ServiceExt;

    let (_temp, mock, api, account) = worker_api_fixture().await;
    let (worker, _route) = WorkerRepository::new(api.storage.db())
        .create_worker(account, "asset-site", RequestId::generate(), 1, 100)
        .unwrap();
    let bytes = b"<main>static</main>";
    let digest = hex::encode(Sha256::digest(bytes));
    let state = authorized_http_state(api.clone());
    let router = control_router().with_state(state.clone());
    let collection = format!(
        "/v1/accounts/{account}/workers/{}/version-uploads",
        worker.id
    );
    let create = serde_json::json!({
        "contentKind": "assets_only",
        "manifest": {
            "schemaVersion": 1,
            "entries": [{
                "path": "/index.html",
                "sha256": digest,
                "size": bytes.len(),
                "contentType": "text/html; charset=utf-8"
            }]
        },
        "routing": {
            "schemaVersion": 1,
            "runWorkerFirst": false,
            "htmlHandling": "auto-trailing-slash",
            "notFoundHandling": "404-page",
            "headers": [],
            "redirects": []
        }
    });
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&collection)
                .header(header::AUTHORIZATION, "Bearer account-admin")
                .header(IDEMPOTENCY_HEADER, "asset-create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&create).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    let upload = created["id"].as_str().unwrap();
    assert_eq!(created["status"], "open");
    assert_eq!(
        created["objects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|object| object["verified"] == true)
            .count(),
        1
    );

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("{collection}/{upload}/objects/{digest}"))
                .header(header::AUTHORIZATION, "Bearer account-admin")
                .header(header::CONTENT_LENGTH, bytes.len())
                .body(Body::from(bytes.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let replayed_put = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("{collection}/{upload}/objects/{digest}"))
                .header(header::AUTHORIZATION, "Bearer account-admin")
                .header(header::CONTENT_LENGTH, bytes.len())
                .body(Body::from(bytes.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replayed_put.status(), StatusCode::OK);
    let finalized_body = serde_json::json!({
        "promote": true
    });
    let finalize_request = || {
        Request::builder()
            .method("POST")
            .uri(format!("{collection}/{upload}/finalize"))
            .header(header::AUTHORIZATION, "Bearer account-admin")
            .header(IDEMPOTENCY_HEADER, "asset-finalize")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&finalized_body).unwrap()))
            .unwrap()
    };
    let response = router.clone().oneshot(finalize_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let first = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(value["version"]["contentKind"], "assets_only");
    assert_eq!(value["version"]["artifactSha256"], serde_json::Value::Null);
    assert_eq!(value["promoted"], true);
    let version_id: VersionId = value["version"]["id"].as_str().unwrap().parse().unwrap();
    let replay = router.clone().oneshot(finalize_request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(
        to_bytes(replay.into_body(), 64 * 1024).await.unwrap(),
        first
    );

    let version = WorkerRepository::new(api.storage.db())
        .get_version(account, worker.id, version_id)
        .unwrap();
    let stored = VersionAssetsRepository::new(api.storage.db())
        .get(version_id)
        .unwrap()
        .unwrap();
    let manifest: open_compute_workers::AssetManifestV1 =
        serde_json::from_slice(&stored.manifest_json).unwrap();
    let routing: open_compute_workers::AssetRoutingConfigV1 =
        serde_json::from_slice(&stored.routing_config_json).unwrap();
    let plan = open_compute_workers::plan_asset_response(
        &manifest,
        &routing,
        open_compute_workers::AssetRequest {
            method: "GET",
            path: "/",
            query: None,
            host: "localhost",
            sec_fetch_mode: None,
            if_none_match: None,
            has_authorization: false,
            has_range: false,
        },
    )
    .unwrap();
    let response = serve_asset_plan(&api.storage, &api.artifacts, None, &version, plan)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let etag = response.headers()[header::ETAG].clone();
    assert_eq!(
        to_bytes(response.into_body(), 64 * 1024).await.unwrap(),
        bytes.as_slice()
    );

    let conditional = open_compute_workers::plan_asset_response(
        &manifest,
        &routing,
        open_compute_workers::AssetRequest {
            method: "GET",
            path: "/",
            query: None,
            host: "localhost",
            sec_fetch_mode: None,
            if_none_match: etag.to_str().ok(),
            has_authorization: false,
            has_range: false,
        },
    )
    .unwrap();
    let response = serve_asset_plan(&api.storage, &api.artifacts, None, &version, conditional)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert!(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .is_empty()
    );

    let redirect = open_compute_workers::plan_asset_response(
        &manifest,
        &routing,
        open_compute_workers::AssetRequest {
            method: "GET",
            path: "/index.html",
            query: None,
            host: "localhost",
            sec_fetch_mode: None,
            if_none_match: None,
            has_authorization: false,
            has_range: false,
        },
    )
    .unwrap();
    let response = serve_asset_plan(&api.storage, &api.artifacts, None, &version, redirect)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/");
    assert_eq!(mock.object_count(), 2);
}

#[test]
fn host_and_route_validation_are_canonical_and_bounded() {
    assert_eq!(canonical_hostname("EXAMPLE.com.").unwrap(), "example.com");
    assert!(canonical_hostname("").is_err());
    assert!(canonical_hostname(&"x".repeat(254)).is_err());
    assert!(canonical_hostname("bad/path").is_err());
    assert!(canonical_hostname("example.com:443").is_err());
    assert_eq!(
        canonical_request_host("EXAMPLE.com:443").unwrap(),
        "example.com"
    );
    assert!(canonical_request_host("bad host").is_err());
    assert!(validate_route_parts("/api/", Some("Named_1")).is_ok());
    assert!(validate_route_parts("relative", None).is_err());
    assert!(validate_route_parts("/bad?query", None).is_err());
    assert!(validate_route_parts("/", Some("bad-name")).is_err());
    assert!(validate_route_parts("/", Some("")).is_err());
    assert!(validate_route_parts("/", Some("9bad")).is_err());
}

#[tokio::test]
async fn request_metadata_json_ids_and_idempotency_are_strict() {
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let version = VersionId::generate();
    assert_eq!(parse_account(&account.to_string()).unwrap(), account);
    assert_eq!(
        parse_ids(&account.to_string(), &worker.to_string()).unwrap(),
        (account, worker)
    );
    assert_eq!(
        parse_version_ids(
            &account.to_string(),
            &worker.to_string(),
            &version.to_string()
        )
        .unwrap(),
        (account, worker, version)
    );
    assert!(parse_account("bad").is_err());
    assert!(parse_ids(&account.to_string(), "bad").is_err());
    assert!(parse_version_ids(&account.to_string(), &worker.to_string(), "bad").is_err());

    let valid = Request::builder()
        .header(IDEMPOTENCY_HEADER, "key-1")
        .header(VERSION_METADATA_HEADER, r#"{"mainModule":"index.js"}"#)
        .body(Body::from(r#"{"name":"worker"}"#))
        .unwrap();
    assert_eq!(idempotency_key(&valid).unwrap(), "key-1");
    let metadata = version_metadata(&valid).unwrap();
    assert_eq!(metadata.main_module, "index.js");
    let body: CreateWorkerBody = read_json(valid, MAX_JSON_BODY).await.unwrap();
    assert_eq!(body.name, "worker");

    let missing = Request::new(Body::empty());
    assert!(idempotency_key(&missing).is_err());
    assert!(version_metadata(&missing).is_err());
    let oversized_metadata = Request::builder()
        .header(
            VERSION_METADATA_HEADER,
            "x".repeat(MAX_VERSION_METADATA_HEADER_BYTES + 1),
        )
        .body(Body::empty())
        .unwrap();
    let Err(error) = version_metadata(&oversized_metadata) else {
        panic!("oversized version metadata unexpectedly accepted");
    };
    assert_eq!(error.code(), ErrorCode::LimitInvalid);
    let whitespace = Request::builder()
        .header(IDEMPOTENCY_HEADER, "has space")
        .body(Body::empty())
        .unwrap();
    assert!(idempotency_key(&whitespace).is_err());
    let too_long = Request::builder()
        .header(IDEMPOTENCY_HEADER, "x".repeat(129))
        .body(Body::empty())
        .unwrap();
    assert!(idempotency_key(&too_long).is_err());

    let invalid_json = Request::new(Body::from(b"{".as_slice()));
    assert!(
        read_json::<CreateWorkerBody>(invalid_json, 32)
            .await
            .is_err()
    );
    let oversized_json = Request::new(Body::from(vec![b'x'; 33]));
    assert!(
        read_json::<CreateWorkerBody>(oversized_json, 32)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn staged_upload_is_bounded_canonical_and_cleaned_on_drop() {
    let dir = TempDir::new().unwrap();
    let limits = BundleLimits::default();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() {} };".to_vec(),
        }],
        limits,
    )
    .unwrap();
    let bytes = bundle.into_bytes();
    let staged = stage_bundle(
        Body::from(bytes.clone()),
        dir.path().to_path_buf(),
        limits,
        bytes.len(),
    )
    .await
    .unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    drop(staged);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

    let Err(too_large) = stage_bundle(
        Body::from(bytes.clone()),
        dir.path().to_path_buf(),
        limits,
        bytes.len() - 1,
    )
    .await
    else {
        panic!("oversized bundle unexpectedly staged");
    };
    assert_eq!(too_large.code(), ErrorCode::BundleTooLarge);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

    let Err(invalid) = stage_bundle(
        Body::from(b"not-a-bundle".as_slice()),
        dir.path().to_path_buf(),
        limits,
        1024,
    )
    .await
    else {
        panic!("invalid bundle unexpectedly staged");
    };
    assert_eq!(invalid.code(), ErrorCode::BundleInvalid);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);

    let missing_dir = dir.path().join("missing");
    let Err(create_error) = stage_bundle(Body::from(bytes), missing_dir, limits, usize::MAX).await
    else {
        panic!("missing staging directory unexpectedly accepted");
    };
    assert_eq!(create_error.code(), ErrorCode::DiskHardLimit);

    let failed_body = Body::from_stream(futures::stream::once(async {
        Err::<Bytes, std::io::Error>(std::io::Error::other("stream failed"))
    }));
    let Err(stream_error) =
        stage_bundle(failed_body, dir.path().to_path_buf(), limits, usize::MAX).await
    else {
        panic!("failed body stream unexpectedly staged");
    };
    assert_eq!(stream_error.code(), ErrorCode::BundleInvalid);
}

#[tokio::test]
async fn response_helpers_map_codes_and_hold_pin_until_body_drop() {
    let request_id = RequestId::generate();
    let mappings = [
        (ErrorCode::AdminAuthRequired, StatusCode::UNAUTHORIZED),
        (ErrorCode::AccountNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::WorkerNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::EntrypointNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::WorkerNameConflict, StatusCode::CONFLICT),
        (ErrorCode::RouteConflict, StatusCode::CONFLICT),
        (ErrorCode::VersionNotReady, StatusCode::CONFLICT),
        (ErrorCode::VersionReferenced, StatusCode::CONFLICT),
        (ErrorCode::BundleTooLarge, StatusCode::PAYLOAD_TOO_LARGE),
        (ErrorCode::AssetLimitExceeded, StatusCode::PAYLOAD_TOO_LARGE),
        (ErrorCode::AssetUploadConflict, StatusCode::CONFLICT),
        (ErrorCode::AssetUploadIncomplete, StatusCode::CONFLICT),
        (
            ErrorCode::BundleRuntimeInvalid,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::CompatibilityUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::AssetConfigUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::RuntimeUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            ErrorCode::ArtifactUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            ErrorCode::AssetStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            ErrorCode::ResourceLimitExceeded,
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
        (
            ErrorCode::RuntimeResultUnknown,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            ErrorCode::VersionInvariantViolation,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            ErrorCode::AssetIntegrityError,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (ErrorCode::ConfigInvalid, StatusCode::BAD_REQUEST),
    ];
    for (code, status) in mappings {
        let response = error_response(PlatformError::new(code, "safe"), request_id);
        assert_eq!(response.status(), status);
    }

    let ok = result_response(Ok(serde_json::json!({"ok": true})), request_id);
    assert_eq!(ok.status(), StatusCode::OK);
    let created = idempotent_response(
        Ok(br#"{"ok":true}"#.to_vec()),
        StatusCode::CREATED,
        request_id,
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(
        created.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let conflict = idempotent_response(
        Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "conflict",
        )),
        StatusCode::CREATED,
        request_id,
    );
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    assert_eq!(
        replayed_failure(br#"{"code":"WORKER_NOT_FOUND"}"#).code(),
        ErrorCode::WorkerNotFound
    );
    for code in [
        ErrorCode::QuotaExceeded,
        ErrorCode::AdmissionBusy,
        ErrorCode::StoragePressure,
        ErrorCode::PlatformUnavailable,
    ] {
        let persisted = serde_json::to_vec(&serde_json::json!({ "code": code.as_str() })).unwrap();
        assert_eq!(replayed_failure(&persisted).code(), code);
    }
    assert_eq!(replayed_failure(b"invalid").code(), ErrorCode::Internal);
    assert_eq!(ErrorCode::from_stable_str("UNKNOWN"), None);
    assert_eq!(
        ErrorCode::from_stable_str("ROUTE_NOT_FOUND"),
        Some(ErrorCode::RouteNotFound)
    );
    assert_eq!(internal().code(), ErrorCode::Internal);
    assert_eq!(
        idempotency_ref_id(AccountId::generate(), "scope", "key").len(),
        64
    );

    let version = VersionId::generate();
    let pins = VersionPins::new();
    let pin = pins.pin(version).unwrap();
    let response = pin_response((StatusCode::OK, "body").into_response(), pin);
    assert_eq!(pins.count(version), 1);
    assert!(!response.body().is_end_stream());
    assert_eq!(response.body().size_hint().exact(), Some(4));
    let mut body = response.into_body();
    assert_eq!(
        body.frame().await.unwrap().unwrap().into_data().unwrap(),
        Bytes::from_static(b"body")
    );
    assert_eq!(pins.count(version), 0);
    assert!(body.frame().await.is_none());
}

#[test]
fn request_id_extension_is_preserved_or_generated() {
    let expected = RequestId::generate();
    let mut request = Request::new(Body::empty());
    request.extensions_mut().insert(expected);
    assert_eq!(request_id(&request), expected);
    assert_ne!(request_id(&Request::new(Body::empty())), expected);
}

#[tokio::test]
async fn idempotent_helpers_replay_running_failed_async_and_version_refs() {
    let (_temp, _mock, api, account) = worker_api_fixture().await;
    let scope = "coverage/sync";
    let canonical = br#"{"value":1}"#;
    let key = "running";
    let mut input = Vec::new();
    input.extend_from_slice(scope.as_bytes());
    input.push(0);
    input.extend_from_slice(canonical);
    let fingerprint = api.storage.crypto().fingerprint_request(&input);
    let repo = WorkerRepository::new(api.storage.db());
    assert_eq!(
        repo.reserve_idempotency(
            account,
            scope,
            key,
            api.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            now_ms(),
            now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
        )
        .unwrap(),
        open_compute_storage::IdempotencyReservation::Reserved
    );
    assert_eq!(
        run_idempotent(
            &api,
            account,
            scope,
            key,
            canonical,
            RequestId::generate(),
            None,
            || Ok(serde_json::json!({"unexpected": true})),
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );

    let failed = || {
        run_idempotent(
            &api,
            account,
            "coverage/failure",
            "failed",
            b"failure",
            RequestId::generate(),
            None,
            || Err(PlatformError::new(ErrorCode::WorkerNotFound, "missing")),
        )
    };
    assert_eq!(failed().unwrap_err().code(), ErrorCode::WorkerNotFound);
    assert_eq!(failed().unwrap_err().code(), ErrorCode::WorkerNotFound);

    let async_scope = "coverage/async";
    let async_canonical = b"async";
    let async_key = "complete";
    let first = run_idempotent_async(
        &api,
        account,
        async_scope,
        async_key,
        async_canonical,
        RequestId::generate(),
        None,
        || async { Ok(serde_json::json!({"ok": true})) },
    )
    .await
    .unwrap();
    let second = run_idempotent_async(
        &api,
        account,
        async_scope,
        async_key,
        async_canonical,
        RequestId::generate(),
        None,
        || async { panic!("completed async operation must replay") },
    )
    .await
    .unwrap();
    assert_eq!(first, second);

    let (worker, _) = repo
        .create_worker(
            account,
            "ref-worker",
            RequestId::generate(),
            now_ms(),
            1_000_000,
        )
        .unwrap();
    let version = VersionId::generate();
    repo.insert_staging_version(
        &NewVersion {
            id: version,
            account_id: account,
            worker_id: worker.id,
            content_kind: VersionContentKind::Worker,
            artifact_sha256: Some([7; 32]),
            artifact_size: Some(7),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [8; 32],
            compatibility_date: "2026-08-30".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: RequestId::generate(),
            now_ms: now_ms(),
        },
        &open_compute_storage::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    let sync_ref = run_idempotent(
        &api,
        account,
        "coverage/sync-ref",
        "sync-ref",
        b"sync-ref",
        RequestId::generate(),
        Some(version),
        || Ok(serde_json::json!({"versionId": version})),
    )
    .unwrap();
    assert!(
        String::from_utf8(sync_ref)
            .unwrap()
            .contains(&version.to_string())
    );
    let body = run_idempotent_async(
        &api,
        account,
        "coverage/ref",
        "ref",
        b"ref",
        RequestId::generate(),
        Some(version),
        || async { Ok(serde_json::json!({"versionId": version})) },
    )
    .await
    .unwrap();
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains(&version.to_string())
    );

    let async_running_scope = "coverage/async-running";
    let async_running_key = "running";
    let mut async_input = Vec::new();
    async_input.extend_from_slice(async_running_scope.as_bytes());
    async_input.push(0);
    async_input.extend_from_slice(b"running");
    let async_fingerprint = api.storage.crypto().fingerprint_request(&async_input);
    repo.reserve_idempotency(
        account,
        async_running_scope,
        async_running_key,
        api.storage.crypto().fingerprint_key_id(),
        &async_fingerprint,
        now_ms(),
        now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
    )
    .unwrap();
    assert_eq!(
        run_idempotent_async(
            &api,
            account,
            async_running_scope,
            async_running_key,
            b"running",
            RequestId::generate(),
            None,
            || async { Ok(serde_json::json!({"unexpected": true})) },
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );

    for _ in 0..2 {
        assert_eq!(
            run_idempotent_async(
                &api,
                account,
                "coverage/async-failure",
                "failed",
                b"failed",
                RequestId::generate(),
                None,
                || async { Err(PlatformError::new(ErrorCode::VersionNotReady, "not ready",)) },
            )
            .await
            .unwrap_err()
            .code(),
            ErrorCode::VersionNotReady
        );
    }

    let delete_version = VersionId::generate();
    repo.insert_staging_version(
        &NewVersion {
            id: delete_version,
            account_id: account,
            worker_id: worker.id,
            content_kind: VersionContentKind::Worker,
            artifact_sha256: Some([9; 32]),
            artifact_size: Some(9),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [10; 32],
            compatibility_date: "2026-08-30".into(),
            compatibility_flags: Vec::new(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: RequestId::generate(),
            now_ms: now_ms(),
        },
        &open_compute_storage::NewVersionProducts::default(),
        1_000_000,
    )
    .unwrap();
    repo.mark_rejected(
        delete_version,
        open_compute_storage::VersionState::Staging,
        ErrorCode::BundleInvalid,
        now_ms(),
    )
    .unwrap();
    let deleted = run_version_delete(
        &api,
        account,
        worker.id,
        delete_version,
        "delete-complete",
        RequestId::generate(),
    )
    .await
    .unwrap();
    let replayed = run_version_delete(
        &api,
        account,
        worker.id,
        delete_version,
        "delete-complete",
        RequestId::generate(),
    )
    .await
    .unwrap();
    assert_eq!(deleted, replayed);

    let missing_version = VersionId::generate();
    for _ in 0..2 {
        assert_eq!(
            run_version_delete(
                &api,
                account,
                worker.id,
                missing_version,
                "delete-failed",
                RequestId::generate(),
            )
            .await
            .unwrap_err()
            .code(),
            ErrorCode::VersionNotFound
        );
    }

    let running_version = VersionId::generate();
    let running_scope = format!("version.delete/{}/{}", worker.id, running_version);
    let running_canonical = serde_json::to_vec(&serde_json::json!({
        "workerId": worker.id,
        "versionId": running_version,
    }))
    .unwrap();
    let mut running_input = Vec::new();
    running_input.extend_from_slice(running_scope.as_bytes());
    running_input.push(0);
    running_input.extend_from_slice(&running_canonical);
    let running_fingerprint = api.storage.crypto().fingerprint_request(&running_input);
    repo.reserve_idempotency(
        account,
        &running_scope,
        "delete-running",
        api.storage.crypto().fingerprint_key_id(),
        &running_fingerprint,
        now_ms(),
        now_ms().saturating_add(IDEMPOTENCY_TTL_MS),
    )
    .unwrap();
    assert_eq!(
        run_version_delete(
            &api,
            account,
            worker.id,
            running_version,
            "delete-running",
            RequestId::generate(),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
}

#[tokio::test]
async fn worker_and_route_failures_replay_after_storage_restart_without_mutation() {
    let (temp, _mock, api, account) = worker_api_fixture().await;
    let artifacts = api.artifacts.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for (scope, key) in [
        ("worker.create", "worker-quota"),
        ("worker.route.create", "route-quota"),
    ] {
        let calls = calls.clone();
        let error = run_idempotent(
            &api,
            account,
            scope,
            key,
            b"quota-request",
            RequestId::generate(),
            None,
            || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "account quota was exceeded",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::QuotaExceeded);
        assert_eq!(
            error_response(error, RequestId::generate()).status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);

    drop(api);
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let restarted = WorkerApiState::new(
        storage,
        artifacts,
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
        VersionPins::new(),
        BundleLimits::default(),
        Duration::from_millis(10),
    );
    for (scope, key) in [
        ("worker.create", "worker-quota"),
        ("worker.route.create", "route-quota"),
    ] {
        let error = run_idempotent(
            &restarted,
            account,
            scope,
            key,
            b"quota-request",
            RequestId::generate(),
            None,
            || -> Result<serde_json::Value, PlatformError> {
                panic!("replayed failure must not execute the mutation")
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::QuotaExceeded);
        assert_eq!(
            error_response(error, RequestId::generate()).status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn removed_operator_api_is_neutral_without_runtime_state() {
    use crate::http::admin_router;
    use open_compute_storage::WorkerRepository;
    use tower::ServiceExt;

    let (_temp, _mock, api, account) = worker_api_fixture().await;
    WorkerRepository::new(api.storage.db())
        .create_worker(account, "listed", RequestId::generate(), 1, 100)
        .unwrap();
    let router = admin_router(authorized_http_state(api));
    for path in [
        "/operator/api/v1/account".to_owned(),
        format!("/operator/api/v1/accounts/{account}/workers"),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header(header::AUTHORIZATION, "Bearer account-admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "path={path}");
    }
}

#[tokio::test]
async fn operator_worker_catalog_filters_sorts_and_paginates_deterministically() {
    use crate::http::admin_router;
    use tower::ServiceExt;

    let (_temp, _mock, api, account) = worker_api_fixture().await;
    let repo = WorkerRepository::new(api.storage.db());
    let alpha = repo
        .create_worker(account, "alpha", RequestId::generate(), 10, 100)
        .unwrap()
        .0;
    repo.create_worker(account, "beta", RequestId::generate(), 20, 100)
        .unwrap();
    repo.create_worker(account, "gamma", RequestId::generate(), 30, 100)
        .unwrap();
    let router = admin_router(authorized_http_state(api));
    let base = format!("/operator/api/v1/accounts/{account}/workers");

    let get = |uri: String| {
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, "Bearer account-admin")
            .body(Body::empty())
            .unwrap()
    };
    let first = router
        .clone()
        .oneshot(get(format!("{base}?sort=name&direction=asc&limit=1")))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: serde_json::Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(first["workers"][0]["id"], alpha.id.to_string());
    assert_eq!(first["listComplete"], false);
    let cursor = first["cursor"].as_str().unwrap();

    let second = router
        .clone()
        .oneshot(get(format!(
            "{base}?sort=name&direction=asc&limit=1&cursor={cursor}"
        )))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    for query in [
        "search=alpha&deployed=false&sort=createdAt&direction=desc&limit=2",
        "search=%20%20&sort=updatedAt&direction=asc",
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(get(format!("{base}?{query}")))
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
            router
                .clone()
                .oneshot(get(format!("{base}?{query}")))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
            "query={query}"
        );
    }
}
