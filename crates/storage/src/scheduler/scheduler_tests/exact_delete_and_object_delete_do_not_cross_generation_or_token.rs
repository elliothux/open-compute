use super::*;

#[test]
fn exact_delete_and_object_delete_do_not_cross_generation_or_token() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    let object_id = object(namespace, 1);
    store
        .upsert_alarm(&projection(namespace, object_id, "delete-token-001", 10), 1)
        .unwrap();
    assert!(
        !store
            .delete_alarm_exact(namespace, object_id, 1, "different-token-1")
            .unwrap()
    );
    assert_eq!(store.delete_object(namespace, object_id, 2).unwrap(), 0);
    assert_eq!(store.delete_object(namespace, object_id, 1).unwrap(), 1);
}
