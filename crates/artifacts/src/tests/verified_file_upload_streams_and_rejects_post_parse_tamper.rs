use super::*;

#[tokio::test]
async fn verified_file_upload_streams_and_rejects_post_parse_tamper() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("staged");
    let payload = vec![b'x'; 48 * 1024];
    write_mode(&path, std::str::from_utf8(&payload).unwrap(), 0o600);
    let digest = hex::encode(Sha256::digest(&payload));
    let artifact = store
        .put_verified_file(&path, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(store.open(&artifact).await.unwrap().as_ref(), payload);

    fs::write(&path, vec![b'y'; 48 * 1024]).unwrap();
    let error = store
        .put_verified_file(&path, &digest, payload.len() as u64)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ArtifactIntegrityError);
}
