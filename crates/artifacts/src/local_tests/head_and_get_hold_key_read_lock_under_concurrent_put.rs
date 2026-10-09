use super::*;

#[tokio::test]
async fn head_and_get_hold_key_read_lock_under_concurrent_put() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/concurrent/read-write").unwrap();
    fixture
        .backend
        .put(
            &key,
            ObjectSource::Bytes(Bytes::from_static(b"seed-body")),
            options(PutMode::Replace),
        )
        .await
        .unwrap();

    let backend = fixture.backend.clone();
    let key_for_put = key.clone();
    let put_task = tokio::spawn(async move {
        backend
            .put(
                &key_for_put,
                ObjectSource::Bytes(Bytes::from_static(b"updated-body")),
                options(PutMode::Replace),
            )
            .await
    });

    let mut head_tasks = Vec::new();
    for _ in 0..32 {
        let backend = fixture.backend.clone();
        let key = key.clone();
        head_tasks.push(tokio::spawn(async move {
            backend.head(&key, HeadOptions::default()).await
        }));
    }
    let mut get_tasks = Vec::new();
    for _ in 0..32 {
        let backend = fixture.backend.clone();
        let key = key.clone();
        get_tasks.push(tokio::spawn(async move {
            bytes(&backend, &key, GetOptions::default()).await
        }));
    }

    put_task.await.unwrap().unwrap();
    for task in head_tasks {
        match task.await.unwrap() {
            Ok(_) | Err(BackendError::NotFound) => {}
            Err(other) => panic!("unexpected concurrent head result: {other}"),
        }
    }
    for task in get_tasks {
        let _ = task.await.unwrap();
    }
    assert_eq!(
        bytes(&fixture.backend, &key, GetOptions::default()).await,
        Bytes::from_static(b"updated-body")
    );
}
