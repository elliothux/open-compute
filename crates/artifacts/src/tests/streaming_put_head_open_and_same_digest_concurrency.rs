use super::*;

#[tokio::test]
async fn streaming_put_head_open_and_same_digest_concurrency() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"hello-artifact");
    let digest = hex::encode(Sha256::digest(&payload));
    let stream = stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]);
    let r1 = store
        .put_verified(stream, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(r1.sha256_hex(), digest);
    let h = store.head(&r1).await.unwrap();
    assert_eq!(h.size(), payload.len() as u64);
    let body = store.open(&r1).await.unwrap();
    assert_eq!(body.as_ref(), payload.as_ref());

    let store2 = store.clone();
    let store3 = store.clone();
    let p1 = payload.clone();
    let p2 = payload.clone();
    let d1 = digest.clone();
    let d2 = digest.clone();
    let a = tokio::spawn(async move {
        store2
            .put_verified(stream::iter(vec![Ok::<Bytes, std::io::Error>(p1)]), &d1, 14)
            .await
    });
    let b = tokio::spawn(async move {
        store3
            .put_verified(stream::iter(vec![Ok::<Bytes, std::io::Error>(p2)]), &d2, 14)
            .await
    });
    let ra = a.await.unwrap().unwrap();
    let rb = b.await.unwrap().unwrap();
    assert_eq!(ra, rb);

    let too_big = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(vec![1; 8]))]),
            &digest,
            8,
        )
        .await
        .unwrap_err();
    assert_eq!(too_big.code(), ErrorCode::ArtifactIntegrityError);
    let over = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(vec![1; 16]))]),
            &"ab".repeat(32),
            1024 * 1024,
        )
        .await
        .unwrap_err();
    assert_eq!(over.code(), ErrorCode::LimitInvalid);

    let mismatch = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"nope",
            ))]),
            &digest,
            4,
        )
        .await
        .unwrap_err();
    assert_eq!(mismatch.code(), ErrorCode::ArtifactIntegrityError);
}
