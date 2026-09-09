use super::*;

#[tokio::test]
async fn local_ssec_is_authenticated_chunked_and_never_persists_plaintext() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/encrypted").unwrap();
    let plaintext = Bytes::from(vec![b'Q'; 150_000]);
    let customer = CustomerKey::new([7; 32]);
    let mut put = options(PutMode::Replace);
    put.customer_key = Some(customer.clone());
    fixture
        .backend
        .put(&key, ObjectSource::Bytes(plaintext.clone()), put)
        .await
        .unwrap();
    let persisted = fs::read(fixture.object_file(&key)).unwrap();
    assert!(
        !persisted
            .windows(1024)
            .any(|window| window == &plaintext[..1024])
    );
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::CustomerKeyInvalid
    );
    assert_eq!(
        fixture
            .backend
            .head(
                &key,
                HeadOptions {
                    customer_key: Some(CustomerKey::new([8; 32])),
                },
            )
            .await
            .unwrap_err(),
        BackendError::CustomerKeyInvalid
    );
    let ranged = bytes(
        &fixture.backend,
        &key,
        GetOptions {
            range: Some(ObjectRange {
                start: 65_530,
                end: 65_550,
            }),
            customer_key: Some(customer.clone()),
            ..GetOptions::default()
        },
    )
    .await;
    assert_eq!(ranged, Bytes::from(vec![b'Q'; 21]));

    let path = fixture.object_file(&key);
    let mut persisted = fs::read(&path).unwrap();
    let last = persisted.len() - 1;
    persisted[last] ^= 1;
    fs::write(&path, persisted).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let output = fixture
        .backend
        .get(
            &key,
            GetOptions {
                customer_key: Some(customer),
                ..GetOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(output.body.collect().await.is_err());
}
