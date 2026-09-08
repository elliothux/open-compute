use super::*;

#[test]
fn current_workflow_scheduler_domain_initialization_is_atomic() {
    for fault in [
        SchedulerMigrationFault::BeforeExecution,
        SchedulerMigrationFault::BeforeMigrationRow,
        SchedulerMigrationFault::AfterCommit,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("scheduler.sqlite");
        let store = SchedulerStore {
            connection: Mutex::new(create_current_scheduler_fixture(&path, 4)),
            wake: Arc::new(SchedulerWakeSignal::default()),
        };

        assert!(store.migrate(10, Some(fault)).is_err());
        let committed = fault == SchedulerMigrationFault::AfterCommit;
        let connection = store.lock().unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            if committed { 5 } else { 4 }
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='workflow_instances')",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap(),
            committed
        );
        drop(connection);
        drop(store);

        let reopened = SchedulerStore::open(&path, 5_000, 11).unwrap();
        assert_eq!(reopened.inspect_workflows(11).unwrap(), Default::default());
        workflow::verify_operation_progress(&reopened.lock().unwrap()).unwrap();
        reopened.quick_check().unwrap();
    }
}
