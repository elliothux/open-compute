use super::*;

#[test]
fn p1_readonly_schema_fence_sees_uncheckpointed_bootstrap_wal() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();

    let readonly =
        crate::ControlDb::open_readonly_wal_aware(&root.join("control.sqlite"), 5_000).unwrap();
    assert_eq!(
        crate::migrations::inspect_schema(&readonly).unwrap(),
        crate::migrations::current_schema_version()
    );
    drop(storage);
}
