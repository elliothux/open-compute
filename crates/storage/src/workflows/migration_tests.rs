use super::*;
use crate::migrations::MigrationFault;

fn legacy_current_control(path: &std::path::Path) -> ControlDb {
    let db = ControlDb::open(path, 5_000).unwrap();
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch(concat!(
                include_str!("../../refinery-migrations/control/V1__init.sql"),
                "CREATE TABLE schema_migrations (
                   version INTEGER NOT NULL PRIMARY KEY,
                   name TEXT NOT NULL,
                   checksum_sha256 BLOB NOT NULL CHECK(length(checksum_sha256)=32),
                   applied_at_ms INTEGER NOT NULL,
                   app_version TEXT NOT NULL
                 ) STRICT;"
            ))
            .unwrap();
        for (version, name, checksum) in crate::migrations::legacy_migration_registry() {
            transaction
                .execute(
                    "INSERT INTO schema_migrations
                     (version,name,checksum_sha256,applied_at_ms,app_version)
                     VALUES(?1,?2,?3,0,?4)",
                    params![
                        version,
                        name,
                        checksum.as_slice(),
                        env!("CARGO_PKG_VERSION")
                    ],
                )
                .unwrap();
        }
        transaction.pragma_update(None, "user_version", 19).unwrap();
        Ok(())
    })
    .unwrap();
    db
}

#[test]
fn verified_legacy_head_adoption_is_atomic_at_every_fault_boundary() {
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::DuringLegacyAdoption,
        MigrationFault::AfterCommit,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let db = legacy_current_control(&temp.path().join("control.sqlite"));
        assert_eq!(db.user_version().unwrap(), 19);

        assert_eq!(
            db.migrate_with_fault(&SystemClock, Some(fault))
                .unwrap_err()
                .code(),
            ErrorCode::MigrationFailed
        );
        let committed = fault == MigrationFault::AfterCommit;
        assert_eq!(
            db.table_exists("refinery_schema_history").unwrap(),
            committed
        );
        assert_eq!(db.table_exists("schema_migrations").unwrap(), !committed);

        db.migrate(&SystemClock).unwrap();
        assert_eq!(db.user_version().unwrap(), 0);
        assert_eq!(
            crate::migrations::inspect_schema(&db).unwrap(),
            crate::migrations::current_schema_version()
        );
        WorkflowRepository::new(&db).verify_catalog().unwrap();
        db.quick_check().unwrap();
    }
}

#[test]
fn legacy_head_with_schema_drift_is_not_adopted() {
    let temp = tempfile::tempdir().unwrap();
    let db = legacy_current_control(&temp.path().join("control.sqlite"));
    db.with_exclusive(|transaction| {
        transaction
            .execute_batch("CREATE TABLE unexpected_platform_table(id INTEGER PRIMARY KEY) STRICT;")
            .unwrap();
        Ok(())
    })
    .unwrap();

    assert_eq!(
        db.migrate(&SystemClock).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert!(db.table_exists("schema_migrations").unwrap());
    assert!(!db.table_exists("refinery_schema_history").unwrap());
}
