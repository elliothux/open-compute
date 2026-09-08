use super::*;

#[test]
fn scheduler_registry_is_contiguous_and_future_schema_fails_closed() {
    let registry = scheduler_migration_registry();
    assert_eq!(registry.len(), 5);
    assert_eq!(registry[0].0, 1);
    assert_eq!(registry[0].1, "001_scheduler");
    assert_eq!(registry[1].0, 2);
    assert_eq!(registry[1].1, "002_queue_producer");
    assert_eq!(registry[2].0, 3);
    assert_eq!(registry[2].1, "003_queue_consumer");
    assert_eq!(registry[3].0, 4);
    assert_eq!(registry[3].1, "004_cron");
    assert_eq!(registry[4].0, 5);
    assert_eq!(registry[4].1, "005_workflow");

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 10);
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "user_version", current_scheduler_schema_version() + 1)
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 20).unwrap_err().code(),
        ErrorCode::SchemaTooNew
    );
}
