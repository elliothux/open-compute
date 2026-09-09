use super::*;

#[tokio::test]
async fn committed_projection_mutations_wake_generation_waiters() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let wake = store.wake_signal();
    let observed = wake.generation();
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 1), "wake-token-00001", 10),
            1,
        )
        .unwrap();
    assert_eq!(wake.notified_since(observed).await, observed + 1);
}
