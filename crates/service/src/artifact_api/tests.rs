use super::*;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::DataConfig;
use open_compute_storage::PlatformStorage;

fn storage(temp: &tempfile::TempDir) -> Arc<PlatformStorage> {
    let root = temp.path().join("data");
    Arc::new(
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
    )
}

#[test]
fn repository_token_syntax_requires_lowercase_hex() {
    assert!(token_secret("art_v1_0123456789abcdef0123456789abcdef01234567").is_ok());
    assert!(token_secret("art_v1_0123456789ABCDEF0123456789abcdef01234567").is_err());
}

#[test]
fn startup_reconciles_incomplete_create_and_delete_states() {
    let temp = tempfile::tempdir().unwrap();
    let storage = storage(&temp);
    let account = storage.identity().default_account_id;
    let catalog = CloudflareArtifactsRepository::new(storage.db());
    let namespace = catalog.ensure_namespace(account, "apps", None, 1).unwrap();
    let complete = catalog
        .reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: "complete",
                description: "",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms: 2,
            },
        )
        .unwrap();
    let missing = catalog
        .reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: "missing",
                description: "",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms: 3,
            },
        )
        .unwrap();
    let deleting = catalog
        .reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: "deleting",
                description: "",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms: 4,
            },
        )
        .unwrap();
    let git = GitRepositoryStore::open(
        storage.data_dir().artifact_git_dir(),
        storage.data_dir().artifact_quarantine_dir(),
        1024 * 1024,
    )
    .unwrap();
    git.initialize(complete.id, "main").unwrap();
    git.initialize(deleting.id, "main").unwrap();
    catalog
        .finish_repository_create(deleting.id, true, 5)
        .unwrap();
    catalog.begin_delete_repository(deleting.id, 6).unwrap();

    ArtifactApiState::new(Arc::clone(&storage), ArtifactsConfig::default()).unwrap();

    let records = catalog.list_live_repositories().unwrap();
    assert_eq!(
        records
            .iter()
            .find(|record| record.id == complete.id)
            .unwrap()
            .state,
        ArtifactRepositoryState::Ready
    );
    assert_eq!(
        records
            .iter()
            .find(|record| record.id == missing.id)
            .unwrap()
            .state,
        ArtifactRepositoryState::Failed
    );
    assert!(!records.iter().any(|record| record.id == deleting.id));
    assert!(!git.path(deleting.id).exists());
}

#[tokio::test]
async fn api_admission_tokens_fork_and_lease_drain_are_fenced() {
    let temp = tempfile::tempdir().unwrap();
    let storage = storage(&temp);
    let account = storage.identity().default_account_id;
    let config = ArtifactsConfig {
        public_origin: "https://artifacts.example.test".to_owned(),
        max_concurrent_requests: 1,
        lease_drain_timeout_ms: 1,
        ..ArtifactsConfig::default()
    };
    let api = ArtifactApiState::new(storage, config).unwrap();
    assert_eq!(api.max_request_bytes(), 256 * 1024 * 1024);
    let permit = api.admit_git().unwrap();
    assert_eq!(
        api.admit_git().unwrap_err().code(),
        ErrorCode::AdmissionBusy
    );
    drop(permit);

    api.create_namespace(account, "apps", 1).unwrap();
    let source = api
        .create_repository(
            account,
            "apps",
            CreateRepositoryRequest {
                name: "source",
                description: "source repo",
                default_branch: "main",
                read_only: false,
            },
            2,
        )
        .unwrap();
    assert_eq!(
        api.repository_for_binding(account, "apps", "source")
            .unwrap()
            .id,
        source.id
    );
    assert_eq!(api.list_namespaces(account).unwrap().len(), 1);
    assert_eq!(api.list_repositories(account, "apps").unwrap().len(), 1);
    assert_eq!(
        api.remote("apps", "source"),
        "https://artifacts.example.test/git/apps/source.git"
    );
    assert_eq!(api.object_count(source.id).unwrap(), 0);
    drop(api.admit_git_mutation(source.id).unwrap());

    assert_eq!(
        api.issue_token(
            account,
            "apps",
            "source",
            ArtifactTokenScope::Read,
            Some(59),
            3,
        )
        .unwrap_err()
        .code(),
        ErrorCode::LimitInvalid
    );
    let token = api
        .issue_token(
            account,
            "apps",
            "source",
            ArtifactTokenScope::Read,
            Some(60),
            3,
        )
        .unwrap();
    assert_eq!(
        api.authenticate_git(&source, &token.plaintext, false, 4)
            .unwrap(),
        token.record.id
    );
    assert_eq!(
        api.authenticate_git(&source, &token.plaintext, true, 4)
            .unwrap_err()
            .code(),
        ErrorCode::BindingPermissionDenied
    );
    assert!(
        api.revoke_token_value(account, "apps", "source", &token.record.id.to_string(), 5,)
            .unwrap()
    );
    assert!(
        !api.revoke_token_value(account, "apps", "source", &token.record.id.to_string(), 6,)
            .unwrap()
    );
    assert_eq!(api.list_tokens(account, "apps", "source").unwrap().len(), 1);

    let fork = api
        .fork_repository(
            account,
            "apps",
            ForkRepositoryRequest {
                source_name: "source",
                target_name: "fork",
                description: None,
                read_only: Some(true),
                default_branch_only: false,
            },
            7,
        )
        .unwrap();
    assert!(fork.read_only);
    assert_eq!(api.object_count(fork.id).unwrap(), 0);

    assert_eq!(
        api.read_object(account, "apps", "source", "bad")
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        api.commit_log(account, "apps", "source", None, 0, 0)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        api.import_repository(ImportRepositoryRequest {
            account,
            namespace: "apps".to_owned(),
            name: "unsafe".to_owned(),
            remote: "http://127.0.0.1/repo.git".to_owned(),
            branch: None,
            depth: None,
            description: String::new(),
            read_only: false,
            now_ms: 8,
        })
        .await
        .unwrap_err()
        .code(),
        ErrorCode::PathInvalid
    );

    let (_, lease) = api
        .repository_with_lease(account, "apps", "source")
        .unwrap();
    assert_eq!(
        api.delete_repository(account, "apps", "source", 9)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceUnavailable
    );
    drop(lease);
    api.delete_repository(account, "apps", "source", 10)
        .unwrap();
    api.delete_repository(account, "apps", "fork", 11).unwrap();
}
