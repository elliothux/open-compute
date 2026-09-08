use super::*;

#[tokio::test]
async fn bound_local_authority_mismatch_does_not_initialize_a_new_root() {
    let directory = TempDir::new().unwrap();
    let path = write_config(directory.path(), "");
    let loaded = load_fixture_platform_config(&path);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let connected =
        crate::object_storage::connect_object_backend(&loaded.config, storage.identity()).unwrap();
    open_compute_artifacts::preflight_object_storage(
        &connected.backend,
        storage.identity().platform_id,
        open_compute_core::StartupId::generate(),
    )
    .await
    .unwrap();
    storage
        .bind_object_authority(
            connected.backend.kind(),
            &connected.backend.authority_sha256(),
        )
        .unwrap();
    drop(connected);
    drop(storage);

    let (discovered, discovered_platform) =
        crate::object_storage::discover_snapshot_backend(&loaded.config, "unused")
            .await
            .unwrap();
    assert_eq!(
        discovered.kind(),
        open_compute_core::ObjectStorageKind::Local
    );
    drop(discovered);

    let rebound = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    assert_eq!(rebound.identity().platform_id, discovered_platform);
    let mut partial_binding = rebound.identity().clone();
    partial_binding.object_backend_kind = Some(open_compute_core::ObjectStorageKind::Local);
    partial_binding.object_authority_sha256 = None;
    let Err(error) =
        crate::object_storage::connect_object_backend(&loaded.config, &partial_binding)
    else {
        panic!("one-sided authority binding must fail");
    };
    assert_eq!(error.code(), ErrorCode::ObjectStorageIntegrityError);
    let mut wrong_kind = rebound.identity().clone();
    wrong_kind.object_backend_kind = Some(open_compute_core::ObjectStorageKind::S3);
    let Err(error) = crate::object_storage::connect_object_backend(&loaded.config, &wrong_kind)
    else {
        panic!("backend-kind mismatch must fail");
    };
    assert_eq!(error.code(), ErrorCode::ObjectStorageAuthorityMismatch);
    let mut wrong_authority = rebound.identity().clone();
    wrong_authority.object_authority_sha256 = Some([0x5a; 32]);
    let Err(error) =
        crate::object_storage::connect_object_backend(&loaded.config, &wrong_authority)
    else {
        panic!("authority digest mismatch must fail");
    };
    assert_eq!(error.code(), ErrorCode::ObjectStorageAuthorityMismatch);
    drop(rebound);

    let mut changed = loaded.config;
    let replacement = directory.path().join("replacement-objects");
    let open_compute_core::ObjectStorageConfig::Local(local) = &mut changed.object_storage else {
        panic!("local backend fixture");
    };
    local.path = replacement.clone();
    let reopened = open_compute_storage::PlatformStorage::bootstrap(
        &changed.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let Err(error) = crate::object_storage::connect_object_backend(&changed, reopened.identity())
    else {
        panic!("a bound platform must not initialize a replacement authority");
    };
    assert_eq!(error.code(), ErrorCode::ObjectStorageIntegrityError);
    assert!(!replacement.exists());
}
