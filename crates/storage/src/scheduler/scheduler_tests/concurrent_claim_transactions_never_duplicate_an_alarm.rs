use super::*;

#[test]
fn concurrent_claim_transactions_never_duplicate_an_alarm() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(open_store(&temp, 1));
    let namespace = ResourceId::generate();
    for byte in 1..=8 {
        store
            .upsert_alarm(
                &projection(
                    namespace,
                    object(namespace, byte),
                    &format!("concurrent-{byte:016}"),
                    10,
                ),
                1,
            )
            .unwrap();
    }
    let threads = (0..2)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || store.claim_due(10, 100, 8).unwrap())
        })
        .collect::<Vec<_>>();
    let claimed = threads
        .into_iter()
        .flat_map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 8);
    let ids = claimed
        .iter()
        .map(|claim| claim.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 8);
}
