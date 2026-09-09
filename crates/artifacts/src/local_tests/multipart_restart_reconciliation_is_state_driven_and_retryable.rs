use super::*;

#[tokio::test]
async fn multipart_restart_reconciliation_is_state_driven_and_retryable() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/restart-multipart").unwrap();
    let publishing = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    let part = fixture
        .backend
        .upload_part(
            &key,
            &publishing,
            1,
            ObjectSource::Bytes(Bytes::from_static(b"restart-safe")),
            None,
        )
        .await
        .unwrap();
    set_multipart_status(
        &fixture.config.path,
        &publishing,
        serde_json::json!({
            "kind": "publishing",
            "etag": "pending-publication"
        }),
    );
    let aborting = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    set_multipart_status(
        &fixture.config.path,
        &aborting,
        serde_json::json!({"kind": "aborting"}),
    );
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    reopened.recover().await.unwrap();
    assert!(!config.path.join("multipart").join(&aborting).exists());
    assert!(config.path.join("multipart").join(&publishing).exists());
    reopened
        .complete_multipart(&key, &publishing, &[part], None)
        .await
        .unwrap();
    assert_eq!(
        bytes(&reopened, &key, GetOptions::default()).await,
        Bytes::from_static(b"restart-safe")
    );
    drop(reopened);
    drop(_temp);
}
