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
        strip_empty_workflow_catalog(db);
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
        assert_eq!(
            db.user_version().unwrap(),
            crate::migrations::current_schema_version()
        );
        let definition = ready(&storage, deployment);
        assert!(definition.current_version_id.is_some());
        db.quick_check().unwrap();
    }
}

fn strip_empty_workflow_catalog(db: &ControlDb) {
    // Exact historical fixture construction only. Production has no downgrade path.
    db.with_immediate(|tx| {
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM workflow_definitions", [], |row| row.get::<_, u64>(0)).unwrap(), 0);
        let triggers = tx.prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workflow_%'").unwrap()
            .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        for trigger in triggers { tx.execute_batch(&format!("DROP TRIGGER \"{trigger}\";")).unwrap(); }
        tx.execute_batch("DROP TABLE workflow_instance_operations;
            DROP TABLE workflow_instance_referrers; DROP TABLE workflow_referrers;
            DROP TABLE workflow_bindings; DROP TABLE workflow_versions; DROP TABLE workflow_definitions;
            DELETE FROM schema_migrations WHERE version>=12; PRAGMA user_version=11;").unwrap();
        Ok(())
    }).unwrap();
}

fn legacy_fixture() -> (tempfile::TempDir, PlatformStorage, Vec<WorkflowReservation>) {
    let (temp, storage, deployment) = setup();
    strip_empty_workflow_catalog(storage.db());
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute_batch(include_str!("../../migrations/012_workflows.sql"))
                .unwrap();
            tx.execute(
                "INSERT INTO schema_migrations VALUES(12,'012_workflows',?1,0,'legacy')",
                [crate::migrations::expected_checksum(12).unwrap()],
            )
            .unwrap();
            tx.pragma_update(None, "user_version", 12).unwrap();
            Ok(())
        })
        .unwrap();
    let definition = ready(&storage, deployment);
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let caller = staging(
        &storage,
        repo.version(account, definition.current_version_id.unwrap())
            .unwrap()
            .target
            .worker_id,
    );
    let binding = repo
        .prepare_binding(account, caller, "FLOW", definition.id, 1, 2)
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| bindings::insert_workflow_bindings(tx, caller, &[binding]))
        .unwrap();
    let rows = ["creating", "live", "released"]
        .into_iter()
        .map(|id| {
            let row = repo
                .reserve_instance(
                    account,
                    definition.id,
                    Some(id),
                    1,
                    &WorkflowsConfig::default(),
                    3,
                )
                .unwrap();
            if id != "creating" {
                repo.finalize_instance(&row.identity, 4).unwrap();
            }
            if id == "released" {
                repo.release_instance(&row.identity, 5).unwrap();
            }
            repo.reservation(row.identity.instance_id).unwrap().unwrap()
        })
        .collect();
    storage.db().with_read(integrity::verify_catalog).unwrap();
    (temp, storage, rows)
}

fn historical_bytes(db: &ControlDb) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    db.with_read(|conn| {
        [
            "workflow_definitions",
            "workflow_versions",
            "workflow_bindings",
            "workflow_instance_referrers",
            "workflow_referrers",
            "deployment_referrers",
            "workers",
            "worker_deployments",
        ]
        .iter()
        .map(|table| {
            let columns = if *table == "workflow_instance_referrers" {
                "instance_id,definition_id,definition_name,external_instance_id,version_id,deployment_id,instance_generation,creation_nonce,state,created_at_ms,updated_at_ms,released_at_ms"
            } else { "*" };
            let mut statement = conn
                .prepare(&format!("SELECT {columns} FROM {table} ORDER BY 1,2,3"))
                .unwrap();
            let columns = statement.column_count();
            Ok(statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap())
        })
        .collect()
    })
    .unwrap()
}

#[test]
fn workflow_operation_sequence_upgrade_preserves_prepared_intent_and_rolls_back_atomically() {
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::DuringDdl,
        MigrationFault::BeforeMigrationRow,
        MigrationFault::AfterCommit,
    ] {
        let (_temp, storage, rows) = legacy_fixture();
        let db = storage.db();
        db.with_immediate(|tx| {
            tx.execute_batch(include_str!("../../migrations/013_workflow_durable_waiting.sql")).unwrap();
            tx.execute("INSERT INTO schema_migrations VALUES(13,'013_workflow_durable_waiting',?1,0,'legacy')",
                [crate::migrations::expected_checksum(13).unwrap()]).unwrap();
            tx.pragma_update(None,"user_version",13).unwrap();
            Ok(())
        }).unwrap();
        let repo = WorkflowRepository::new(db);
        let target = &rows[0].identity.target;
        let version = repo
            .stage_version(
                target.account_id,
                target.definition_id,
                target.deployment_id,
                "Flow",
                2,
                6,
            )
            .unwrap();
        repo.finish_version(target.account_id, version.target.version_id, true, 7)
            .unwrap();
        let identity = repo
            .reserve_instance(
                target.account_id,
                target.definition_id,
                Some("operation-upgrade"),
                2,
                &WorkflowsConfig::default(),
                8,
            )
            .unwrap()
            .identity;
        repo.finalize_instance(&identity, 9).unwrap();
        let operation_id = open_compute_core::WorkflowOperationId::generate();
        // Schema 013 did not have an operation sequence. Preserve its exact old intent,
        // instead of teaching current production APIs to write an obsolete schema.
        db.with_immediate(|tx| {
            tx.execute("INSERT INTO workflow_instance_operations(operation_id,instance_id,creation_nonce,expected_generation,target_generation,kind,prior_ref_state,created_at_ms)
                VALUES(?1,?2,?3,1,2,'restart','live',10)",params![operation_id.to_string(),identity.instance_id.to_string(),identity.creation_nonce.as_bytes().as_slice()]).unwrap();
            tx.execute("UPDATE workflow_instance_referrers SET state='restarting',updated_at_ms=10 WHERE instance_id=?1",[identity.instance_id.to_string()]).unwrap();
            Ok(())
        }).unwrap();
        let before = historical_bytes(db);
        assert_eq!(
            db.migrate_with_fault(&SystemClock, Some(fault))
                .unwrap_err()
                .code(),
            ErrorCode::MigrationFailed
        );
        assert_eq!(
            db.user_version().unwrap(),
            if fault == MigrationFault::AfterCommit {
                14
            } else {
                13
            }
        );
        assert_eq!(historical_bytes(db), before);
        db.migrate(&SystemClock).unwrap();
        let operation = repo
            .instance_operation(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(operation.id(), operation_id);
        assert_eq!(operation.identity(), &identity);
        assert_eq!(operation.sequence(), 1);
        assert_eq!(operation.target_generation(), 2);
        assert_eq!(operation.created_at_ms(), 10);
        repo.verify_catalog().unwrap();
    }
}

#[test]
fn workflow_v12_rebuild_preserves_every_byte_and_rolls_back_with_foreign_keys_on() {
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::AfterWorkflowRebuild,
        MigrationFault::DuringDdl,
        MigrationFault::BeforeMigrationRow,
        MigrationFault::AfterCommit,
    ] {
        let (_temp, storage, rows) = legacy_fixture();
        let db = storage.db();
        let before = historical_bytes(db);
        assert_eq!(
            db.migrate_with_fault(&SystemClock, Some(fault))
                .unwrap_err()
                .code(),
            ErrorCode::MigrationFailed
        );
        assert_eq!(
            db.user_version().unwrap(),
            if fault == MigrationFault::AfterCommit {
                13
            } else {
                12
            }
        );
        assert_eq!(historical_bytes(db), before);
        db.with_read(|conn| {
            assert_eq!(
                conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_temp_master WHERE name LIKE 'saved_workflow_%'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
            Ok(())
        })
        .unwrap();
        db.migrate(&SystemClock).unwrap();
        assert_eq!(
            db.user_version().unwrap(),
            crate::migrations::current_schema_version()
        );
        assert_eq!(historical_bytes(db), before);
        let repo = WorkflowRepository::new(db);
        repo.verify_catalog().unwrap();
        for expected in &rows {
            let actual = repo
                .reservation(expected.identity.instance_id)
                .unwrap()
                .unwrap();
            assert_eq!(actual.identity, expected.identity);
            assert_eq!(actual.state, expected.state);
            assert_eq!(actual.updated_at_ms, expected.updated_at_ms);
        }
    }
}

#[test]
fn workflow_upgrade_rejects_corrupt_hashes_and_missing_references_without_ddl() {
    for corruption in [
        "DROP TRIGGER workflow_version_identity_guard; UPDATE workflow_versions SET descriptor_sha256=zeroblob(32);",
        "DROP TRIGGER workflow_binding_immutable; UPDATE workflow_bindings SET descriptor_sha256=zeroblob(32);",
        "DROP TRIGGER workflow_referrer_guard; DELETE FROM workflow_referrers WHERE referrer_kind='instance';",
    ] {
        let (_temp, storage, _) = legacy_fixture();
        storage
            .db()
            .with_read(|conn| {
                conn.execute_batch(corruption).unwrap();
                Ok(())
            })
            .unwrap();
        let corrupt = historical_bytes(storage.db());
        assert_eq!(
            storage.db().migrate(&SystemClock).unwrap_err().code(),
            ErrorCode::MigrationFailed
        );
        assert_eq!(storage.db().user_version().unwrap(), 12);
        assert_eq!(historical_bytes(storage.db()), corrupt);
        assert!(
            !storage
                .db()
                .table_exists("workflow_instance_operations")
                .unwrap()
        );
    }
}
