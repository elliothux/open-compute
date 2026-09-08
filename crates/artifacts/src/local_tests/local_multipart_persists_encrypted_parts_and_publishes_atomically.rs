use super::*;

#[tokio::test]
async fn local_multipart_persists_encrypted_parts_and_publishes_atomically() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/multipart").unwrap();
    let customer = CustomerKey::new([9; 32]);
    let upload = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), Some(customer.clone()))
        .await
        .unwrap();
    let first = fixture
        .backend
        .upload_part(
            &key,
            &upload,
            1,
            ObjectSource::Bytes(Bytes::from_static(b"first-part-secret")),
            Some(customer.clone()),
        )
        .await
        .unwrap();
    let second = fixture
        .backend
        .upload_part(
            &key,
            &upload,
            2,
            ObjectSource::Bytes(Bytes::from_static(b"second-part-secret")),
            Some(customer.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.backend.list_multipart(&key).await.unwrap(),
        vec![upload.clone()]
    );
    for file in regular_files(&fixture.config.path.join("multipart")) {
        let persisted = fs::read(file).unwrap();
        assert!(!persisted.windows(11).any(|window| window == b"part-secret"));
    }
    fixture
        .backend
        .complete_multipart(&key, &upload, &[first, second], Some(customer.clone()))
        .await
        .unwrap();
    assert!(
        fixture
            .backend
            .list_multipart(&key)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        bytes(
            &fixture.backend,
            &key,
            GetOptions {
                customer_key: Some(customer),
                ..GetOptions::default()
            },
        )
        .await,
        Bytes::from_static(b"first-part-secretsecond-part-secret")
    );

    let aborted = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    fixture
        .backend
        .abort_multipart(&key, &aborted)
        .await
        .unwrap();
    fixture
        .backend
        .abort_multipart(&key, &aborted)
        .await
        .unwrap();
}
