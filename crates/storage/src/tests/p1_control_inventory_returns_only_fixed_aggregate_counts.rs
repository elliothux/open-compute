use super::*;

#[test]
fn p1_control_inventory_returns_only_fixed_aggregate_counts() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let empty = crate::inspect_control_inventory(storage.db()).unwrap();
    assert_eq!(empty.accounts, 1);
    assert_eq!(empty.workers, 0);
    assert_eq!(empty.versions, 0);
    assert_eq!(empty.routes, 0);
    assert_eq!(empty.kv_namespaces, 0);

    WorkerRepository::new(storage.db())
        .create_worker(
            storage.identity().default_account_id,
            "inventory-worker",
            open_compute_core::RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let populated = crate::inspect_control_inventory(storage.db()).unwrap();
    assert_eq!(populated.accounts, 1);
    assert_eq!(populated.workers, 1);
    assert_eq!(populated.routes, 1);
}
