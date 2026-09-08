use super::*;

#[tokio::test]
async fn put_delete_and_fsync_faults_recover_without_torn_objects() {
    for (fault, published) in [
        (LocalFaultPoint::BeforeEnvelopeFsync, false),
        (LocalFaultPoint::AfterEnvelopeFsync, false),
        (LocalFaultPoint::BeforePublishRename, false),
        (LocalFaultPoint::AfterPublishRename, true),
    ] {
        let fixture = Fixture::new();
        let key = ObjectKey::new("system/fault/put").unwrap();
        fixture.backend.inject_local_fault(fault);
        assert_eq!(
            fixture
                .backend
                .put(
                    &key,
                    ObjectSource::Bytes(Bytes::from_static(b"atomic-body")),
                    options(PutMode::Replace),
                )
                .await
                .unwrap_err(),
            BackendError::Unavailable,
            "{fault:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
        let Fixture {
            _temp,
            config,
            platform_id,
            backend,
        } = fixture;
        drop(backend);
        let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
        reopened.recover().await.unwrap();
        if published {
            assert_eq!(
                bytes(&reopened, &key, GetOptions::default()).await,
                Bytes::from_static(b"atomic-body"),
                "{fault:?}"
            );
        } else {
            assert_eq!(
                reopened
                    .head(&key, HeadOptions::default())
                    .await
                    .unwrap_err(),
                BackendError::NotFound,
                "{fault:?}"
            );
        }
        drop(reopened);
        drop(_temp);
    }

    let fixture = Fixture::new();
    let key = ObjectKey::new("system/fault/delete").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"delete-me")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    fixture
        .backend
        .inject_local_fault(LocalFaultPoint::AfterDeleteUnlink);
    assert_eq!(
        fixture.backend.delete(&key).await.unwrap_err(),
        BackendError::Unavailable
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
    assert_eq!(
        reopened
            .head(&key, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::NotFound
    );
    drop(reopened);
    drop(_temp);
}
