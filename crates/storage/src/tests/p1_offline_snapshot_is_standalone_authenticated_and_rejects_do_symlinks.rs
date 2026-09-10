use super::*;

#[test]
fn p1_offline_snapshot_is_standalone_authenticated_and_rejects_do_symlinks() {
    let (tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let do_root = storage
        .data_dir()
        .prepare_durable_object_storage(&storage.identity().platform_id.to_string(), "workerd test")
        .unwrap();
    let do_file = do_root.join("state.bin");
    fs::write(&do_file, b"opaque-do-state").unwrap();
    fs::set_permissions(&do_file, fs::Permissions::from_mode(0o600)).unwrap();
    let outside = tmp.path().join("outside");
    fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, do_root.join("forbidden-link")).unwrap();
    storage
        .bind_object_authority(ObjectStorageKind::Local, &[0xdd; 32])
        .unwrap();
    let artifacts = crate::CloudflareArtifactsRepository::new(storage.db());
    let namespace = artifacts
        .ensure_namespace(storage.identity().default_account_id, "apps", None, 1)
        .unwrap();
    let repository = artifacts
        .reserve_repository(
            &namespace,
            crate::NewArtifactRepository {
                name: "source",
                description: "",
                default_branch: "main",
                read_only: false,
                source: None,
                initial_state: crate::ArtifactRepositoryState::Creating,
                now_ms: 1,
            },
        )
        .unwrap();
    artifacts
        .finish_repository_create(repository.id, true, 2)
        .unwrap();
    let git = storage
        .data_dir()
        .artifact_git_dir()
        .join(format!("{}.git", repository.id));
    for relative in ["", "objects", "refs"] {
        let path = git.join(relative);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (name, bytes) in [
        ("HEAD", b"ref: refs/heads/main\n".as_slice()),
        ("config", b"[core]\n\tbare = true\n".as_slice()),
        ("description", b"snapshot fixture\n".as_slice()),
    ] {
        let path = git.join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&config).unwrap();
    let key = crate::inspect_master_key(&config).unwrap();
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let hardening = HardeningConfig::default();
    let mut request = crate::PreparePlatformSnapshotRequest {
        snapshot_id: &snapshot_id,
        label: "p1-test",
        created_at_ms: 1,
        release: p1_release_identity(),
        master_key_fingerprint: key.fingerprint(),
        object_backend_kind: ObjectStorageKind::Local,
        object_authority_fingerprint: &"d".repeat(64),
        r2_prefix_fingerprint: &"e".repeat(64),
        config_policy_sha256: &"f".repeat(64),
        object_prefix: &format!(
            "system/snapshots/v1/{}/{snapshot_id}/objects/",
            crate::inspect_control_db(&data_dir.control_db_path(), 5_000)
                .unwrap()
                .1
                .platform_id
        ),
        hardening: &hardening,
        sqlite_busy_timeout_ms: 5_000,
    };
    let wrong_fingerprint = "0".repeat(64);
    let mut wrong_key_request = request.clone();
    wrong_key_request.master_key_fingerprint = &wrong_fingerprint;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &wrong_key_request)
            .unwrap_err()
            .code(),
        ErrorCode::MasterKeyMismatch
    );

    let mut wrong_schema_request = request.clone();
    wrong_schema_request.release.control_schema_version += 1;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &wrong_schema_request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );

    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    let staging = data_dir
        .backup_staging_dir()
        .join(format!("platform-{snapshot_id}"));
    assert!(
        !staging.exists(),
        "staging entries: {:?}",
        fs::read_dir(&staging)
            .map(|entries| entries
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>())
            .ok()
    );
    fs::remove_file(do_root.join("forbidden-link")).unwrap();
    request.release.control_schema_version = 8;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    request.release.control_schema_version =
        u32::try_from(crate::migrations::current_schema_version()).unwrap();
    let mut prepared = crate::prepare_platform_snapshot(&data_dir, &request).unwrap();
    assert!(prepared.manifest.files.iter().any(|file| {
        file.role == open_compute_core::SnapshotFileRole::DurableObjectFile
            && file.restore_path.ends_with("state.bin")
    }));
    assert!(prepared.manifest.files.iter().any(|file| {
        file.role == open_compute_core::SnapshotFileRole::ArtifactGitFile
            && file.restore_path.ends_with("/HEAD")
    }));
    crate::sign_snapshot_manifest(&mut prepared.manifest, &key).unwrap();
    crate::verify_snapshot_manifest_mac(&prepared.manifest, &key).unwrap();
    prepared.manifest.label.push('x');
    assert_eq!(
        crate::verify_snapshot_manifest_mac(&prepared.manifest, &key)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    let largest_file = prepared
        .manifest
        .files
        .iter()
        .map(|file| file.size)
        .max()
        .unwrap();
    let total_bytes = prepared.manifest.totals.bytes;
    drop(prepared);

    let file_limited = HardeningConfig {
        max_snapshot_file_bytes: largest_file - 1,
        max_snapshot_total_bytes: total_bytes,
        ..HardeningConfig::default()
    };
    request.hardening = &file_limited;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    let staging = data_dir
        .backup_staging_dir()
        .join(format!("platform-{snapshot_id}"));
    assert!(
        !staging.exists(),
        "staging entries: {:?}",
        fs::read_dir(&staging)
            .map(|entries| entries
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>())
            .ok()
    );

    let total_limited = HardeningConfig {
        max_snapshot_file_bytes: largest_file,
        max_snapshot_total_bytes: total_bytes - 1,
        ..HardeningConfig::default()
    };
    request.hardening = &total_limited;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
}
