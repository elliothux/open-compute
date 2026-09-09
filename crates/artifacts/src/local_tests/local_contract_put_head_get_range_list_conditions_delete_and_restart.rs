use super::*;

#[tokio::test]
async fn local_contract_put_head_get_range_list_conditions_delete_and_restart() {
    let fixture = Fixture::new();
    let first = ObjectKey::new("system/contracts/a").unwrap();
    let second = ObjectKey::new("system/contracts/a/child").unwrap();
    let first_meta = fixture
        .backend
        .put(
            &first,
            ObjectSource::Bytes(Bytes::from_static(b"0123456789")),
            options(PutMode::CreateOnly),
        )
        .await
        .unwrap();
    assert_eq!(first_meta.size, 10);
    assert_eq!(
        first_meta.user.get("name").map(String::as_str),
        Some("value")
    );
    assert_eq!(
        fixture
            .backend
            .put(
                &first,
                ObjectSource::Bytes(Bytes::from_static(b"different")),
                options(PutMode::CreateOnly),
            )
            .await
            .unwrap_err(),
        BackendError::PreconditionFailed
    );
    fixture
        .backend
        .put(
            &second,
            ObjectSource::Bytes(Bytes::from_static(b"child")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();
    assert_eq!(
        bytes(
            &fixture.backend,
            &first,
            GetOptions {
                range: Some(ObjectRange { start: 2, end: 5 }),
                ..GetOptions::default()
            }
        )
        .await,
        Bytes::from_static(b"2345")
    );
    let replacement = fixture
        .backend
        .put(
            &first,
            ObjectSource::Bytes(Bytes::from_static(b"replacement")),
            options(PutMode::IfMatch(first_meta.etag.clone())),
        )
        .await
        .unwrap();
    assert_ne!(replacement.etag, first_meta.etag);
    assert_eq!(
        fixture
            .backend
            .put(
                &first,
                ObjectSource::Bytes(Bytes::from_static(b"stale")),
                options(PutMode::IfMatch(first_meta.etag)),
            )
            .await
            .unwrap_err(),
        BackendError::PreconditionFailed
    );
    let page = fixture
        .backend
        .list("system/contracts/", 1, None)
        .await
        .unwrap();
    assert_eq!(page.objects.len(), 1);
    let cursor = page.next_cursor.unwrap();
    assert_eq!(
        fixture
            .backend
            .list("system/contracts/", 10, Some("not-a-local-cursor"))
            .await
            .unwrap_err(),
        BackendError::InvalidKey
    );
    let page = fixture
        .backend
        .list("system/contracts/", 10, Some(&cursor))
        .await
        .unwrap();
    assert_eq!(page.objects.len(), 1);
    fixture.backend.delete(&first).await.unwrap();
    fixture.backend.delete(&first).await.unwrap();
    assert_eq!(
        fixture
            .backend
            .head(&first, HeadOptions::default())
            .await
            .unwrap_err(),
        BackendError::NotFound
    );

    let fingerprint = fixture.backend.authority_sha256();
    let Fixture {
        _temp,
        config,
        platform_id,
        backend,
    } = fixture;
    drop(backend);
    let reopened = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
    assert_eq!(reopened.authority_sha256(), fingerprint);
    assert_eq!(
        bytes(&reopened, &second, GetOptions::default()).await,
        Bytes::from_static(b"child")
    );
    drop(_temp);
}
