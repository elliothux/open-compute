use super::*;

#[tokio::test]
async fn p1_snapshot_layout_commits_manifest_last_and_verifies_exact_bytes() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    let platform = PlatformId::generate();
    let store = SnapshotObjectStore::new(client.clone(), platform);
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let prefix = store.object_prefix(&snapshot_id).unwrap();
    let key = format!("{prefix}000000.bin");
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source.bin");
    write_mode(&source, "snapshot-bytes", 0o600);
    let payload = fs::read(&source).unwrap();
    let digest = hex::encode(Sha256::digest(&payload));
    store
        .put_file(&key, &source, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert!(store.list_committed().await.unwrap().is_empty());
    store
        .verify_file(&key, &digest, payload.len() as u64)
        .await
        .unwrap();
    let restored = temp.path().join("restored.bin");
    store
        .download_file(&key, &restored, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(fs::read(restored).unwrap(), payload);

    let manifest = br#"{"schema_version":1}"#;
    let manifest_key = store
        .put_manifest(&snapshot_id, manifest, 1024)
        .await
        .unwrap();
    assert_eq!(manifest_key, store.manifest_key(&snapshot_id).unwrap());
    assert_eq!(
        store.get_manifest(&snapshot_id, 1024).await.unwrap(),
        manifest
    );
    assert_eq!(store.list_committed().await.unwrap().len(), 1);
    let discovered = SnapshotObjectStore::discover(client, &snapshot_id)
        .await
        .unwrap();
    assert_eq!(
        discovered.get_manifest(&snapshot_id, 1024).await.unwrap(),
        manifest
    );
    assert!(
        store
            .put_manifest(&snapshot_id, b"different", 1024)
            .await
            .is_err()
    );
    let incomplete_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let incomplete_key = format!("{}000000.bin", store.object_prefix(&incomplete_id).unwrap());
    store
        .put_file(&incomplete_key, &source, &digest, payload.len() as u64)
        .await
        .unwrap();
    let cleanup = store
        .cleanup_incomplete(SystemTime::now() + Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(cleanup.prefixes, 1);
    assert_eq!(cleanup.objects, 1);
    assert_eq!(cleanup.bytes, payload.len() as u64);
    assert!(
        store
            .verify_file(&incomplete_key, &digest, payload.len() as u64)
            .await
            .is_err()
    );
    assert_eq!(store.list_committed().await.unwrap().len(), 1);
    mock.set_fault(Fault::CorruptBody);
    assert!(
        store
            .verify_file(&key, &digest, payload.len() as u64)
            .await
            .is_err()
    );
}
