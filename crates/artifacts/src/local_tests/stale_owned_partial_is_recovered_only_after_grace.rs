use super::*;

#[tokio::test]
async fn stale_owned_partial_is_recovered_only_after_grace() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/recovery/value").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"value")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    let parent = fixture.object_file(&key).parent().unwrap().to_owned();
    let partial = parent.join(format!(".partial-{}", uuid::Uuid::now_v7()));
    fs::write(&partial, b"owned-crash-remnant").unwrap();
    fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&partial)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(2))
        .unwrap();
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    assert!(partial.exists());
    reopened.recover().await.unwrap();
    assert!(!partial.exists());
    drop(reopened);
    drop(_temp);
}
