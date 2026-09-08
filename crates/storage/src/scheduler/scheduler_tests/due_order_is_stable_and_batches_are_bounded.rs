use super::*;

#[test]
fn due_order_is_stable_and_batches_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    for (byte, due) in [(1, 30), (2, 20), (3, 20), (4, 10)] {
        store
            .upsert_alarm(
                &projection(
                    namespace,
                    object(namespace, byte),
                    &format!("token-{byte:016}"),
                    due,
                ),
                1,
            )
            .unwrap();
    }
    let first = store.claim_due(20, 100, 2).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].due_at_ms, 10);
    assert_eq!(first[1].due_at_ms, 20);
    assert!(first[0].id < first[1].id || first[0].due_at_ms < first[1].due_at_ms);
}
