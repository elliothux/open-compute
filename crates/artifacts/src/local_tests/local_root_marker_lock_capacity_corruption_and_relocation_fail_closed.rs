use super::*;

#[tokio::test]
async fn local_root_marker_lock_capacity_corruption_and_relocation_fail_closed() {
    let fixture = Fixture::new();
    assert!(matches!(
        ObjectBackend::open_local(&fixture.config, fixture.platform_id, LIMIT),
        Err(error) if error.code() == ErrorCode::DataDirInUse
    ));
    let key = ObjectKey::new("system/security/value").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"verified")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let object = fixture.object_file(&key);
    let hardlink = object.with_file_name("evidence-hardlink");
    fs::hard_link(&object, &hardlink).unwrap();
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    fs::remove_file(hardlink).unwrap();

    let mut full = fixture.config.clone();
    full.path = fixture.config.path.parent().unwrap().join("capacity-root");
    full.free_space_hard_bytes = u64::MAX;
    let full_backend = ObjectBackend::open_local(&full, PlatformId::generate(), LIMIT).unwrap();
    assert_eq!(
        full_backend
            .put(
                &ObjectKey::new("system/full").unwrap(),
                ObjectSource::Bytes(Bytes::from_static(b"x")),
                options(PutMode::Replace),
            )
            .await
            .unwrap_err(),
        BackendError::Capacity
    );
    drop(full_backend);

    let Fixture {
        _temp,
        mut config,
        platform_id,
        backend,
    } = fixture;
    let fingerprint = backend.authority_sha256();
    drop(backend);
    assert!(ObjectBackend::open_local(&config, PlatformId::generate(), LIMIT).is_err());
    let moved = config.path.parent().unwrap().join("moved-objects");
    fs::rename(&config.path, &moved).unwrap();
    config.path = moved;
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    assert_eq!(reopened.authority_sha256(), fingerprint);
    drop(reopened);
    let evidence = config.path.join("unexpected-evidence");
    fs::write(&evidence, b"preserve-for-operator").unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(ObjectBackend::open_local(&config, platform_id, LIMIT).is_err());
    assert_eq!(fs::read(&evidence).unwrap(), b"preserve-for-operator");
    drop(_temp);
}
