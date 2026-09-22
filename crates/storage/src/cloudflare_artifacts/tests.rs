use super::*;
use crate::PlatformStorage;
use crate::workers::EffectiveResourceLimits;
use crate::{NewVersion, NewVersionProducts, VersionContentKind, WorkerRepository};
use open_compute_core::clock::SystemClock;
use open_compute_core::config::DataConfig;
use open_compute_core::{BindingId, CanonicalPermissions, RequestId, VersionId};

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

#[test]
fn deleted_worker_revokes_artifact_namespace_binding() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let catalog = CloudflareArtifactsRepository::new(storage.db());
    let namespace = catalog.ensure_namespace(account, "apps", None, 1).unwrap();
    let workers = WorkerRepository::new(storage.db());
    let request = RequestId::generate();
    let (worker, _) = workers
        .create_worker(account, "artifact-owner", request, 2, 1_000_000)
        .unwrap();
    let version = VersionId::generate();
    let binding = NewVersionArtifactBinding {
        id: BindingId::generate(),
        name: "ARTIFACTS".into(),
        namespace_id: namespace.id,
        namespace_generation: 1,
        capability_version: 1,
        permissions: CanonicalPermissions::default(),
        descriptor_sha256: [7; 32],
    };
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([8; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [9; 32],
                compatibility_date: "2026-09-08".into(),
                compatibility_flags: Vec::new(),
                resource_limits: EffectiveResourceLimits::standard_defaults(),
                vars: std::collections::BTreeMap::new(),
                secrets: std::collections::BTreeMap::new(),
                request_id: request,
                now_ms: 3,
            },
            &NewVersionProducts {
                artifact_bindings: std::slice::from_ref(&binding),
                ..NewVersionProducts::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 4).unwrap();
    assert!(
        catalog
            .authorize_binding(binding.id, version, &binding.descriptor_sha256)
            .is_ok()
    );

    workers
        .delete_worker(account, worker.id, &[version], request, 5)
        .unwrap();
    assert!(
        catalog
            .authorize_binding(binding.id, version, &binding.descriptor_sha256)
            .is_err()
    );
}
