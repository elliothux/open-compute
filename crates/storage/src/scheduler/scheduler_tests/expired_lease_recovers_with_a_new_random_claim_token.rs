use super::*;

#[test]
fn expired_lease_recovers_with_a_new_random_claim_token() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 1), "recover-token-001", 10),
            1,
        )
        .unwrap();
    let [first] = store.claim_due(10, 50, 1).unwrap().try_into().unwrap();
    assert!(store.claim_due(59, 50, 1).unwrap().is_empty());
    let (second, recovered) = store.claim_due_with_recovery(60, 50, 1).unwrap();
    assert_eq!(recovered, 1);
    let [second] = second.try_into().unwrap();
    assert_ne!(first.claim_token, second.claim_token);
    assert!(!store.finish_claim(&first, ClaimResult::Delete, 61).unwrap());
    assert!(
        store
            .finish_claim(&second, ClaimResult::Delete, 61)
            .unwrap()
    );
}
