use super::*;
use crate::PlatformStorage;
use open_compute_core::clock::SystemClock;
use open_compute_core::config::DataConfig;

fn storage() -> (tempfile::TempDir, PlatformStorage) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &SystemClock,
    )
    .unwrap();
    (temp, storage)
}

#[test]
fn lifecycle_tokens_and_same_name_recreation_are_fenced() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let catalog = CloudflareArtifactsRepository::new(storage.db());
    let namespace = catalog
        .ensure_namespace(account, "apps", None, 1_000)
        .unwrap();
    assert_eq!(
        catalog
            .ensure_namespace(account, "apps", None, 2_000)
            .unwrap(),
        namespace
    );

    let creating = catalog
        .reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: "site",
                description: "first",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms: 2_000,
            },
        )
        .unwrap();
    let ready = catalog
        .finish_repository_create(creating.id, true, 3_000)
        .unwrap();
    let token_id = ArtifactTokenId::generate();
    let digest = [7_u8; 32];
    catalog
        .create_token(
            &ready,
            NewArtifactToken {
                id: token_id,
                digest: &digest,
                scope: ArtifactTokenScope::Read,
                expires_at_ms: 100_000,
                now_ms: 4_000,
                max_tokens: 2,
            },
        )
        .unwrap();
    assert_eq!(
        catalog
            .authenticate_token(ready.id, &digest, false, 5_000)
            .unwrap(),
        token_id
    );
    assert_eq!(
        catalog
            .authenticate_token(ready.id, &digest, true, 5_000)
            .unwrap_err()
            .code(),
        ErrorCode::BindingPermissionDenied
    );
    assert_eq!(
        catalog
            .authenticate_token(ready.id, &[8_u8; 32], false, 5_000)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );

    catalog.begin_delete_repository(ready.id, 6_000).unwrap();
    catalog.finish_delete_repository(ready.id, 7_000).unwrap();
    assert_eq!(
        catalog
            .authenticate_token(ready.id, &digest, false, 8_000)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    let replacement = catalog
        .reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: "site",
                description: "second",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms: 9_000,
            },
        )
        .unwrap();
    assert_ne!(replacement.id, ready.id);
}

#[test]
fn invalid_names_jurisdiction_and_transitions_fail_closed() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let catalog = CloudflareArtifactsRepository::new(storage.db());
    assert_eq!(
        catalog
            .ensure_namespace(account, "../escape", None, 1)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        catalog
            .ensure_namespace(account, "_hidden", None, 1)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        catalog
            .ensure_namespace(account, "apps", Some("eu"), 1)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let namespace = catalog.ensure_namespace(account, "apps", None, 1).unwrap();
    let repository = catalog
        .reserve_repository(
            &namespace,
            NewArtifactRepository {
                name: "repo",
                description: "",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: ArtifactRepositoryState::Creating,
                now_ms: 2,
            },
        )
        .unwrap();
    assert!(catalog.begin_delete_repository(repository.id, 3).is_err());
    let ready = catalog
        .finish_repository_create(repository.id, true, 4)
        .unwrap();
    let deleting = catalog.begin_delete_repository(ready.id, 5).unwrap();
    assert_eq!(deleting.state, ArtifactRepositoryState::Deleting);
    let restored = catalog.cancel_delete_repository(ready.id, 6).unwrap();
    assert_eq!(restored.state, ArtifactRepositoryState::Ready);
    assert!(restored.generation > ready.generation);
}
