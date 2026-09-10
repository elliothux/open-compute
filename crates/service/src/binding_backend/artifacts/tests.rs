use super::*;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::{ArtifactsConfig, DataConfig};

fn fixture() -> (
    tempfile::TempDir,
    ArtifactApiState,
    open_compute_core::AccountId,
) {
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
    let account = storage.identity().default_account_id;
    let api = ArtifactApiState::new(storage, ArtifactsConfig::default()).unwrap();
    api.create_namespace(account, "apps", 1_000).unwrap();
    (temp, api, account)
}

#[test]
fn worker_list_uses_cursor_and_omits_remote_while_get_returns_handle_info() {
    let (_temp, api, account) = fixture();
    for name in ["alpha", "beta"] {
        let body =
            serde_json::Map::from_iter([("name".to_owned(), Value::String(name.to_owned()))]);
        let created = create(&api, account, "apps", &body, 2_000).unwrap();
        assert!(created["token"].as_str().unwrap().starts_with("art_v1_"));
        assert!(created.get("tokenExpiresAt").is_some());
    }
    let page = list(
        &api,
        account,
        "apps",
        &serde_json::Map::from_iter([("limit".to_owned(), Value::from(1))]),
    )
    .unwrap();
    assert_eq!(page["repos"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], 2);
    assert!(page["repos"][0].get("remote").is_none());
    let cursor = page["cursor"].as_str().unwrap();
    let page = list(
        &api,
        account,
        "apps",
        &serde_json::Map::from_iter([
            ("limit".to_owned(), Value::from(1)),
            ("cursor".to_owned(), Value::String(cursor.to_owned())),
        ]),
    )
    .unwrap();
    assert_eq!(page["repos"].as_array().unwrap().len(), 1);
    assert!(page.get("cursor").is_none());

    let body = serde_json::Map::from_iter([("name".to_owned(), Value::String("alpha".to_owned()))]);
    let info = get(&api, account, "apps", &body).unwrap();
    assert!(
        info["remote"]
            .as_str()
            .unwrap()
            .ends_with("/git/apps/alpha.git")
    );
}

#[test]
fn worker_errors_preserve_fixed_codes() {
    let (_temp, api, account) = fixture();
    let error = list(
        &api,
        account,
        "apps",
        &serde_json::Map::from_iter([("limit".to_owned(), Value::from(0))]),
    )
    .unwrap_err();
    assert_eq!(error.code, "INVALID_INPUT");
    assert_eq!(error.numeric, INVALID_INPUT);

    let invalid_url = ArtifactBindingError::from_import(PlatformError::new(
        ErrorCode::ArtifactUnavailable,
        "test",
    ));
    assert_eq!(
        (invalid_url.code, invalid_url.numeric),
        ("INVALID_URL", INVALID_URL)
    );
    let auth = ArtifactBindingError::from_import(PlatformError::new(
        ErrorCode::BindingPermissionDenied,
        "test",
    ));
    assert_eq!(
        (auth.code, auth.numeric),
        ("REMOTE_AUTH_REQUIRED", REMOTE_AUTH_REQUIRED)
    );
}

#[tokio::test]
async fn import_rejects_credentials_before_reserving_repository_metadata() {
    let (_temp, api, account) = fixture();
    let error = api
        .import_repository(ImportRepositoryRequest {
            account,
            namespace: "apps".to_owned(),
            name: "unsafe".to_owned(),
            remote: "https://user:secret@example.com/repo.git".to_owned(),
            branch: None,
            depth: None,
            description: String::new(),
            read_only: false,
            now_ms: 2_000,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PathInvalid);
    assert!(api.list_repositories(account, "apps").unwrap().is_empty());
}
