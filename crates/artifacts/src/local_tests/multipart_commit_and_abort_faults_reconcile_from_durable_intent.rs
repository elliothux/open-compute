use super::*;

#[tokio::test]
async fn multipart_commit_and_abort_faults_reconcile_from_durable_intent() {
    for fault in [
        LocalFaultPoint::MultipartIntentCommitted,
        LocalFaultPoint::MultipartBeforePublish,
        LocalFaultPoint::MultipartAfterPublish,
        LocalFaultPoint::MultipartBeforeRetire,
    ] {
        let fixture = Fixture::new();
        let key = ObjectKey::new("tenant/r2/fault-multipart").unwrap();
        let upload_id = fixture
            .backend
            .create_multipart(&key, ObjectMetadata::default(), None)
            .await
            .unwrap();
        let part = fixture
            .backend
            .upload_part(
                &key,
                &upload_id,
                1,
                ObjectSource::Bytes(Bytes::from_static(b"multipart-body")),
                None,
            )
            .await
            .unwrap();
        fixture.backend.inject_local_fault(fault);
        assert_eq!(
            fixture
                .backend
                .complete_multipart(&key, &upload_id, std::slice::from_ref(&part), None)
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
        if matches!(
            fault,
            LocalFaultPoint::MultipartIntentCommitted | LocalFaultPoint::MultipartBeforePublish
        ) {
            reopened
                .complete_multipart(&key, &upload_id, &[part], None)
                .await
                .unwrap();
        } else {
            assert!(!config.path.join("multipart").join(&upload_id).exists());
        }
        assert_eq!(
            bytes(&reopened, &key, GetOptions::default()).await,
            Bytes::from_static(b"multipart-body"),
            "{fault:?}"
        );
        drop(reopened);
        drop(_temp);
    }

    let fixture = Fixture::new();
    let key = ObjectKey::new("tenant/r2/fault-abort").unwrap();
    let upload_id = fixture
        .backend
        .create_multipart(&key, ObjectMetadata::default(), None)
        .await
        .unwrap();
    fixture
        .backend
        .inject_local_fault(LocalFaultPoint::MultipartAbortIntent);
    assert_eq!(
        fixture
            .backend
            .abort_multipart(&key, &upload_id)
            .await
            .unwrap_err(),
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
    assert!(!config.path.join("multipart").join(upload_id).exists());
    drop(reopened);
    drop(_temp);
}
