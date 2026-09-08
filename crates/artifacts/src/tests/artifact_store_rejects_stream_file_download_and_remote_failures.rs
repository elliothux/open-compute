use super::*;

#[tokio::test]
async fn artifact_store_rejects_stream_file_download_and_remote_failures() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let empty_digest = hex::encode(Sha256::digest([]));

    let stream_error = store
        .put_verified(
            stream::iter(vec![Err::<Bytes, IoError>(IoError::other("read failed"))]),
            &empty_digest,
            0,
        )
        .await
        .unwrap_err();
    assert_eq!(stream_error.code(), ErrorCode::ObjectStorageUnavailable);

    let too_many = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(Bytes::from_static(b"ab"))]),
            &hex::encode(Sha256::digest(b"ab")),
            1,
        )
        .await
        .unwrap_err();
    assert_eq!(too_many.code(), ErrorCode::LimitInvalid);
    let too_few = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(Bytes::from_static(b"a"))]),
            &hex::encode(Sha256::digest(b"a")),
            2,
        )
        .await
        .unwrap_err();
    assert_eq!(too_few.code(), ErrorCode::ArtifactIntegrityError);
    assert_eq!(
        store
            .put_verified(stream::empty::<Result<Bytes, IoError>>(), "bad-digest", 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );

    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    assert_eq!(
        store
            .put_verified_file(&missing, &empty_digest, 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::DiskHardLimit
    );
    assert_eq!(
        store
            .put_verified_file(temp.path(), &empty_digest, 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    let staged = temp.path().join("staged");
    fs::write(&staged, b"abc").unwrap();
    assert_eq!(
        store
            .put_verified_file(&staged, &hex::encode(Sha256::digest(b"abc")), 2)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .put_verified_file(&staged, &hex::encode(Sha256::digest(b"abc")), 65 * 1024)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );

    let payload = Bytes::from_static(b"writer-failure");
    let digest = hex::encode(Sha256::digest(&payload));
    let artifact = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(payload)]),
            &digest,
            14,
        )
        .await
        .unwrap();
    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(IoError::other("disk full"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        store
            .download_verified(&artifact, &mut FailingWriter)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::DiskHardLimit
    );

    let oversized = ArtifactRef::new(1, &empty_digest, 65 * 1024).unwrap();
    assert_eq!(
        store
            .download_verified(&oversized, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    let absent = ArtifactRef::new(1, &"11".repeat(32), 1).unwrap();
    let absent_error = store.open(&absent).await.unwrap_err();
    assert_eq!(absent_error.code(), ErrorCode::ObjectStorageUnavailable);
    assert!(is_not_found(&absent_error));

    mock.set_fault(Fault::CorruptMetadata);
    assert_eq!(
        store.head(&artifact).await.unwrap_err().code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::DeleteFail);
    assert_eq!(
        store
            .delete_unreferenced(&artifact)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageUnavailable
    );
    mock.set_fault(Fault::ServerError);
    assert_eq!(
        store.list_candidates().await.unwrap_err().code(),
        ErrorCode::ObjectStorageUnavailable
    );
}
