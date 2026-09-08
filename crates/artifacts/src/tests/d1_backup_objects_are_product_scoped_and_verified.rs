use super::*;

#[tokio::test]
async fn d1_backup_objects_are_product_scoped_and_verified() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("data.sqlite");
    let payload = b"d1-sqlite-backup";
    write_mode(&staged, std::str::from_utf8(payload).unwrap(), 0o600);
    let digest = hex::encode(Sha256::digest(payload));
    let relative = "backups/d1/resource/backup/data.sqlite";
    let kv_relative = "backups/kv/resource/backup/data.sqlite";

    assert_eq!(
        store.d1_backup_key(relative).unwrap(),
        format!("system/{relative}")
    );
    assert_eq!(
        store.kv_backup_key(relative).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        store.d1_backup_key(kv_relative).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );

    let key = store
        .put_d1_backup_file(relative, &staged, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(
        store
            .download_kv_backup(&key, &digest, payload.len() as u64, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let mut restored = Vec::new();
    store
        .download_d1_backup(&key, &digest, payload.len() as u64, &mut restored)
        .await
        .unwrap();
    assert_eq!(restored, payload);

    let manifest = Bytes::from_static(br#"{"schema":1}"#);
    let manifest_key = store
        .put_d1_backup_manifest("backups/d1/resource/backup/manifest.json", manifest.clone())
        .await
        .unwrap();
    assert_eq!(store.d1_backup_manifest_key(&key).unwrap(), manifest_key);
    assert_eq!(
        store.get_d1_backup_manifest(&manifest_key).await.unwrap(),
        manifest
    );
    assert_eq!(
        store.kv_backup_manifest_key(&key).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );

    store.delete_d1_backup(&key).await.unwrap();
    assert_eq!(
        store.delete_kv_backup(&key).await.unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
}
