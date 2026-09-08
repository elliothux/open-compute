use super::*;

#[tokio::test]
async fn artifact_store_integrity_and_existing_file_paths() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = b"artifact-body";
    let digest = hex::encode(Sha256::digest(payload));
    let artifact = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(Bytes::copy_from_slice(payload))]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();

    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("staged");
    write_mode(&staged, std::str::from_utf8(payload).unwrap(), 0o600);
    assert_eq!(
        store
            .put_verified_file(&staged, &digest, payload.len() as u64)
            .await
            .unwrap(),
        artifact
    );

    let wrong_size = ArtifactRef::new(1, &digest, payload.len() as u64 + 1).unwrap();
    assert_eq!(
        store.head(&wrong_size).await.unwrap_err().code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .download_verified(&wrong_size, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    let wrong_digest = ArtifactRef::new(1, &"11".repeat(32), payload.len() as u64).unwrap();
    mock.put_raw(&wrong_digest.physical_key("system/"), payload.to_vec());
    assert_eq!(
        store
            .download_verified(&wrong_digest, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    mock.set_fault(Fault::CorruptBody);
    assert_eq!(
        store
            .download_verified(&artifact, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::ServerError);
    assert_eq!(
        store
            .put_verified(
                stream::iter(vec![Ok::<Bytes, IoError>(Bytes::copy_from_slice(b"new"))]),
                &hex::encode(Sha256::digest(b"new")),
                3,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageUnavailable
    );
    assert_eq!(
        store
            .put_verified_file(&staged, &digest, payload.len() as u64)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageUnavailable
    );

    mock.set_fault(Fault::None);
    let maximum = 64 * 1024_u64;
    let large_digest = "22".repeat(32);
    let large = ArtifactRef::new(1, &large_digest, maximum).unwrap();
    mock.put_raw(
        &large.physical_key("system/"),
        vec![b'x'; maximum as usize + 1],
    );
    assert_eq!(
        store
            .download_verified(&large, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
}
