use super::*;

#[tokio::test]
async fn concurrent_put_precondition_races_verify_the_existing_winner() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"concurrent-stream-race");
    let digest = hex::encode(Sha256::digest(&payload));
    mock.synchronize_next_heads(2);
    let first = store.put_verified(
        stream::iter(vec![Ok::<Bytes, IoError>(payload.clone())]),
        &digest,
        payload.len() as u64,
    );
    let second = store.put_verified(
        stream::iter(vec![Ok::<Bytes, IoError>(payload.clone())]),
        &digest,
        payload.len() as u64,
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), second.unwrap());

    let file_payload = b"concurrent-file-race";
    let file_digest = hex::encode(Sha256::digest(file_payload));
    let dir = TempDir::new().unwrap();
    let staged = dir.path().join("staged");
    write_mode(&staged, std::str::from_utf8(file_payload).unwrap(), 0o600);
    mock.synchronize_next_heads(2);
    let first = store.put_verified_file(&staged, &file_digest, file_payload.len() as u64);
    let second = store.put_verified_file(&staged, &file_digest, file_payload.len() as u64);
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), second.unwrap());
}
