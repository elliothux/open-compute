use super::*;

#[test]
fn recovery_is_bounded_and_workload_summary_includes_lease_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    for byte in 1..=3 {
        store
            .upsert_alarm(
                &projection(
                    namespace,
                    object(namespace, byte),
                    &format!("recover-{byte:016}"),
                    10,
                ),
                1,
            )
            .unwrap();
    }
    assert_eq!(store.claim_due(10, 50, 3).unwrap().len(), 3);
    let before = store.workload_summary(20).unwrap();
    assert_eq!(before.claimed, 3);
    assert_eq!(before.next_due_at_ms, Some(60));
    assert_eq!(store.recover_expired(60, 2).unwrap(), 2);
    let after = store.workload_summary(60).unwrap();
    assert_eq!(after.ready, 2);
    assert_eq!(after.claimed, 1);
    assert_eq!(after.expired, 1);
}
