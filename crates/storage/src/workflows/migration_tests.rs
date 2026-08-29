use super::*;
use crate::migrations::MigrationFault;

fn control_at_version_10(path: &std::path::Path) -> ControlDb {
    let db = ControlDb::open(path, 5_000).unwrap();
    let definitions = [
        ("001_init", include_str!("../../migrations/001_init.sql")),
        (
            "002_workers_runtime",
            include_str!("../../migrations/002_workers_runtime.sql"),
        ),
        (
            "003_resource_bindings",
            include_str!("../../migrations/003_resource_bindings.sql"),
        ),
        ("004_kv", include_str!("../../migrations/004_kv.sql")),
        ("005_r2", include_str!("../../migrations/005_r2.sql")),
        ("006_d1", include_str!("../../migrations/006_d1.sql")),
        (
            "007_durable_objects",
            include_str!("../../migrations/007_durable_objects.sql"),
        ),
        (
            "008_queues",
            include_str!("../../migrations/008_queues.sql"),
        ),
        (
            "009_queue_consumers",
            include_str!("../../migrations/009_queue_consumers.sql"),
        ),
        (
            "010_cron_triggers",
            include_str!("../../migrations/010_cron_triggers.sql"),
        ),
    ];
    for (index, (name, sql)) in definitions.into_iter().enumerate() {
        let version = i64::try_from(index + 1).unwrap();
        db.with_exclusive(|tx| {
            tx.execute_batch(sql).unwrap();
            tx.execute(
                "INSERT INTO schema_migrations
                 (version,name,checksum_sha256,applied_at_ms,app_version)
                 VALUES(?1,?2,?3,0,?4)",
                params![
                    version,
                    name,
                    crate::migrations::expected_checksum(version).unwrap(),
                    env!("CARGO_PKG_VERSION")
                ],
            )
            .unwrap();
            tx.pragma_update(None, "user_version", version).unwrap();
            Ok(())
        })
        .unwrap();
    }
    db
}

#[test]
fn current_workflow_domain_initialization_is_atomic_at_every_fault_boundary() {
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::DuringDdl,
        MigrationFault::BeforeMigrationRow,
        MigrationFault::AfterCommit,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("control.sqlite");
        let db = control_at_version_10(&path);
        assert_eq!(db.user_version().unwrap(), 10);

        assert_eq!(
            db.migrate_with_fault(&SystemClock, Some(fault))
                .unwrap_err()
                .code(),
            ErrorCode::MigrationFailed
        );
        let committed = fault == MigrationFault::AfterCommit;
        assert_eq!(db.user_version().unwrap(), if committed { 11 } else { 10 });
        assert_eq!(
            db.table_exists("workflow_instance_operations").unwrap(),
            committed
        );

        db.migrate(&SystemClock).unwrap();
        assert_eq!(
            db.user_version().unwrap(),
            crate::migrations::current_schema_version()
        );
        WorkflowRepository::new(&db).verify_catalog().unwrap();
        db.quick_check().unwrap();
    }
}
