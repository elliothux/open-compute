use super::*;
use crate::migrations::MigrationFault;

#[test]
fn workflow_control_v11_upgrade_is_atomic_at_every_migration_boundary() {
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::DuringDdl,
        MigrationFault::BeforeMigrationRow,
        MigrationFault::AfterCommit,
    ] {
        let (_temp, storage, deployment) = setup();
        let db = storage.db();
        // Construct the exact previous schema from the populated fixture. No production
        // downgrade path exists; only this test strips the new, still-empty tables.
        db.with_immediate(|tx| {
            let triggers = tx.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workflow_%'").unwrap()
                .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
            for trigger in triggers { tx.execute_batch(&format!("DROP TRIGGER \"{trigger}\";")).unwrap(); }
            tx.execute_batch("DROP TABLE workflow_instance_referrers; DROP TABLE workflow_referrers;
                DROP TABLE workflow_bindings; DROP TABLE workflow_versions; DROP TABLE workflow_definitions;
                DELETE FROM schema_migrations WHERE version=12; PRAGMA user_version=11;").unwrap();
            Ok(())
        }).unwrap();
        assert_eq!(db.user_version().unwrap(), 11);
        assert_eq!(
            db.migrate_with_fault(&SystemClock, Some(fault))
                .unwrap_err()
                .code(),
            ErrorCode::MigrationFailed
        );
        assert_eq!(
            db.user_version().unwrap(),
            if fault == MigrationFault::AfterCommit {
                12
            } else {
                11
            }
        );
        db.migrate(&SystemClock).unwrap();
        assert_eq!(db.user_version().unwrap(), 12);
        let definition = ready(&storage, deployment);
        assert!(definition.current_version_id.is_some());
        db.quick_check().unwrap();
    }
}
