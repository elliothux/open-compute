use super::*;

#[tokio::test]
async fn kv_backup_objects_are_host_scoped_immutable_and_verified() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("backup.sqlite");
    let payload = b"sqlite-backup";
    write_mode(&staged, std::str::from_utf8(payload).unwrap(), 0o600);
    let digest = hex::encode(Sha256::digest(payload));
    let relative = "backups/kv/account/resource/backup/data.sqlite";

    assert_eq!(
        store.kv_backup_key(relative).unwrap(),
        format!("system/{relative}")
    );
    for invalid in ["", "/backups/kv/x", "artifacts/x", "backups/kv/../x"] {
        assert_eq!(
            store.kv_backup_key(invalid).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }
    assert_eq!(
        store
            .put_kv_backup_file(relative, temp.path(), &digest, payload.len() as u64)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .put_kv_backup_file(relative, &staged, &digest, payload.len() as u64 + 1)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .put_kv_backup_file(relative, &staged, &"11".repeat(32), payload.len() as u64)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    let key = store
        .put_kv_backup_file(relative, &staged, &digest, payload.len() as u64)
        .await
        .unwrap();
    let mut restored = Vec::new();
    store
        .download_kv_backup(&key, &digest, payload.len() as u64, &mut restored)
        .await
        .unwrap();
    assert_eq!(restored, payload);
    assert_eq!(
        store
            .download_kv_backup(
                "system/artifacts/x",
                &digest,
                payload.len() as u64,
                &mut Vec::new()
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );

    mock.set_fault(Fault::CorruptMetadata);
    assert_eq!(
        store
            .download_kv_backup(&key, &digest, payload.len() as u64, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::CorruptBody);
    assert_eq!(
        store
            .download_kv_backup(&key, &digest, payload.len() as u64, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::DeleteFail);
    assert_eq!(
        store.delete_kv_backup(&key).await.unwrap_err().code(),
        ErrorCode::ObjectStorageUnavailable
    );
    mock.set_fault(Fault::None);
    store.delete_kv_backup(&key).await.unwrap();
    assert_eq!(
        store
            .delete_kv_backup("system/artifacts/x")
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
}
