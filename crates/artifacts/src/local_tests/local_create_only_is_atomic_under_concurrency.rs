use super::*;

#[tokio::test]
async fn local_create_only_is_atomic_under_concurrency() {
    let fixture = Fixture::new();
    let key = ObjectKey::new("system/race/value").unwrap();
    let mut tasks = Vec::new();
    for value in 0_u8..16 {
        let backend = fixture.backend.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            backend
                .put(
                    &key,
                    ObjectSource::Bytes(Bytes::from(vec![value; 64])),
                    options(PutMode::CreateOnly),
                )
                .await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => successes += 1,
            Err(BackendError::PreconditionFailed) => {}
            Err(error) => panic!("unexpected create-only result: {error}"),
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(
        bytes(&fixture.backend, &key, GetOptions::default())
            .await
            .len(),
        64
    );
}
