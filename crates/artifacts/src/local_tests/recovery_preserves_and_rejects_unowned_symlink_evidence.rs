use super::*;

#[tokio::test]
async fn recovery_preserves_and_rejects_unowned_symlink_evidence() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/recovery/evidence").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"value")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let outside = fixture
        .config
        .path
        .parent()
        .unwrap()
        .join("outside-evidence");
    fs::write(&outside, b"preserve").unwrap();
    let partial = fixture
        .object_file(&key)
        .parent()
        .unwrap()
        .join(format!(".partial-{}", uuid::Uuid::now_v7()));
    std::os::unix::fs::symlink(&outside, &partial).unwrap();
    assert_eq!(
        fixture.backend.recover().await.unwrap_err(),
        BackendError::Corrupt
    );
    assert!(partial.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&outside).unwrap(), b"preserve");
}
