use super::*;

#[tokio::test]
async fn local_envelope_truncation_and_plaintext_tamper_fail_closed() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/corruption/value").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"authenticated payload")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let path = fixture.object_file(&key);
    let original = fs::read(&path).unwrap();
    fs::write(&path, &original[..8]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        fixture
            .backend
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::Corrupt
    );
    fs::write(&path, &original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut tampered = original;
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    fs::write(&path, tampered).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let body = fixture
        .backend
        .get(&key, GetOptions::default())
        .await
        .unwrap()
        .body;
    assert!(body.collect().await.is_err());
}
