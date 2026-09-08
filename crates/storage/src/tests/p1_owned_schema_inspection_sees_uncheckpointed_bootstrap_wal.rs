use super::*;

#[test]
fn p1_owned_schema_inspection_sees_uncheckpointed_bootstrap_wal() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    assert!(crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).is_err());
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());

    let state = crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).unwrap();
    assert_eq!(
        i64::from(state.control),
        crate::migrations::current_schema_version()
    );
    assert_eq!(
        state.scheduler,
        u32::try_from(crate::current_scheduler_schema_version()).unwrap()
    );
    assert_eq!(state.kv_files, 0);
    assert_eq!(state.d1_files, 0);
}
