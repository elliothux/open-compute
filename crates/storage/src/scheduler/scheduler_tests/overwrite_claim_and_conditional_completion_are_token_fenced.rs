use super::*;

#[test]
fn overwrite_claim_and_conditional_completion_are_token_fenced() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 10);
    let namespace = ResourceId::generate();
    let object_id = object(namespace, 7);
    let old = projection(namespace, object_id, "old-token-0000001", 20);
    store.upsert_alarm(&old, 10).unwrap();
    let [claimed] = store.claim_due(20, 100, 1).unwrap().try_into().unwrap();
    assert_eq!(claimed.row_token, old.row_token);
    assert_eq!(claimed.claim_token.len(), 64);

    let mut current = projection(namespace, object_id, "new-token-0000002", 40);
    current.target_version_id = old.target_version_id;
    store.upsert_alarm(&current, 21).unwrap();
    assert!(
        !store
            .finish_claim(&claimed, ClaimResult::Delete, 22)
            .unwrap()
    );
    assert!(store.claim_due(39, 100, 1).unwrap().is_empty());
    let [replacement] = store.claim_due(40, 100, 1).unwrap().try_into().unwrap();
    assert_eq!(replacement.row_token, current.row_token);
}
