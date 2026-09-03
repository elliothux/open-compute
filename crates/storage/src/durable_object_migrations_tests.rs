use super::*;
use crate::{NewVersion, WorkerRepository};
use open_compute_core::config::StorageConfig;
use open_compute_core::{ErrorCode, RequestId, SystemClock};
use std::collections::BTreeMap;
use std::path::Path;

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn insert_validating_version(
    storage: &PlatformStorage,
    account_id: AccountId,
    worker_id: WorkerId,
    now_ms: i64,
) -> VersionId {
    let version_id = VersionId::generate();
    let workers = WorkerRepository::new(storage.db());
    workers
        .insert_staging_version(
            &NewVersion {
                id: version_id,
                account_id,
                worker_id,
                content_kind: crate::VersionContentKind::Worker,
                artifact_sha256: Some([7; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [8; 32],
                compatibility_date: "2026-08-30".to_owned(),
                compatibility_flags: Vec::new(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms,
            },
            &crate::NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version_id).unwrap();
    version_id
}

fn create_plan() -> DurableObjectMigrationPlan {
    DurableObjectMigrationPlan {
        declarative: false,
        old_tag: None,
        new_tag: "v1".to_owned(),
        new_sqlite_classes: vec!["Counter".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    }
}

#[test]
fn worker_migrations_publish_rename_retire_and_rollback_namespaces() {
    let temp = tempfile::tempdir().unwrap();
    let storage =
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap();
    let account = storage.identity().default_account_id;
    let (worker, _) = WorkerRepository::new(storage.db())
        .create_worker(account, "migration-worker", RequestId::generate(), 100, 100)
        .unwrap();
    let repository = DurableObjectRepository::new(&storage);
    let create = create_plan();
    assert_eq!(
        repository
            .prepare_worker_migration(account, worker.id, &create, 101)
            .unwrap(),
        DurableObjectMigrationPreparation::Pending
    );
    assert!(repository.list_namespaces(account).unwrap().is_empty());
    let pending = repository
        .namespace_for_worker_upload(account, worker.id, "Counter", Some("v1"))
        .unwrap();
    assert!(
        repository
            .namespace_for_worker_upload(account, worker.id, "Counter", None)
            .is_err()
    );
    let version_v1 = insert_validating_version(&storage, account, worker.id, 102);
    WorkerRepository::new(storage.db())
        .mark_ready_with_durable_object_migration(version_v1, worker.id, &create, 102)
        .unwrap();
    let head = repository
        .current_worker_migration(worker.id)
        .unwrap()
        .unwrap();
    assert_eq!(head.tag, "v1");
    assert_eq!(head.old_tag, None);
    assert_eq!(head.version_id, version_v1);
    assert_eq!(head.plan_sha256, create.fingerprint().unwrap());
    assert_eq!(repository.list_namespaces(account).unwrap().len(), 1);
    let illegal_rename = storage
        .db()
        .with_immediate(|tx| {
            Ok(tx
                .execute(
                    "UPDATE do_namespaces SET class_name = 'Corrupt' WHERE resource_id = ?1",
                    [pending.resource.id.to_string()],
                )
                .is_err())
        })
        .unwrap();
    assert!(illegal_rename);

    let declarative = DurableObjectMigrationPlan {
        declarative: true,
        old_tag: Some("v1".to_owned()),
        new_tag: "exports-v2".to_owned(),
        new_sqlite_classes: vec!["Counter".to_owned(), "Another".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    };
    repository
        .prepare_worker_migration(account, worker.id, &declarative, 103)
        .unwrap();
    let version_v2 = insert_validating_version(&storage, account, worker.id, 104);
    WorkerRepository::new(storage.db())
        .mark_ready_with_durable_object_migration(version_v2, worker.id, &declarative, 104)
        .unwrap();
    assert_eq!(
        repository
            .current_worker_migration(worker.id)
            .unwrap()
            .unwrap()
            .old_tag
            .as_deref(),
        Some("v1")
    );
    assert_eq!(repository.list_namespaces(account).unwrap().len(), 2);

    let rename = DurableObjectMigrationPlan {
        declarative: false,
        old_tag: Some("exports-v2".to_owned()),
        new_tag: "v3".to_owned(),
        new_sqlite_classes: Vec::new(),
        renamed_classes: vec![DurableObjectClassRename {
            from: "Counter".to_owned(),
            to: "RenamedCounter".to_owned(),
        }],
        deleted_classes: Vec::new(),
    };
    repository
        .prepare_worker_migration(account, worker.id, &rename, 105)
        .unwrap();
    let renamed = repository
        .namespace_for_worker_upload(account, worker.id, "RenamedCounter", Some("v3"))
        .unwrap();
    assert_eq!(renamed.resource.id, pending.resource.id);
    let version_v3 = insert_validating_version(&storage, account, worker.id, 106);
    WorkerRepository::new(storage.db())
        .mark_ready_with_durable_object_migration(version_v3, worker.id, &rename, 106)
        .unwrap();

    let failed = DurableObjectMigrationPlan {
        declarative: false,
        old_tag: Some("v3".to_owned()),
        new_tag: "v4-failed".to_owned(),
        new_sqlite_classes: vec!["Temporary".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    };
    repository
        .prepare_worker_migration(account, worker.id, &failed, 107)
        .unwrap();
    repository
        .rollback_worker_migration(worker.id, "v4-failed", 108)
        .unwrap();
    assert!(
        repository
            .namespace_for_worker_upload(account, worker.id, "Temporary", Some("v4-failed"))
            .is_err()
    );

    let delete = DurableObjectMigrationPlan {
        declarative: false,
        old_tag: Some("v3".to_owned()),
        new_tag: "v4".to_owned(),
        new_sqlite_classes: Vec::new(),
        renamed_classes: Vec::new(),
        deleted_classes: vec!["RenamedCounter".to_owned()],
    };
    repository
        .prepare_worker_migration(account, worker.id, &delete, 109)
        .unwrap();
    let version_v4 = insert_validating_version(&storage, account, worker.id, 110);
    WorkerRepository::new(storage.db())
        .mark_ready_with_durable_object_migration(version_v4, worker.id, &delete, 110)
        .unwrap();
    assert_eq!(repository.list_namespaces(account).unwrap().len(), 1);
    assert_eq!(
        repository
            .get_namespace(account, pending.resource.id)
            .unwrap()
            .namespace_storage_key,
        pending.namespace_storage_key
    );
}

#[test]
fn version_ready_and_migration_publish_are_atomic_across_failure_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let (worker, _) = WorkerRepository::new(storage.db())
        .create_worker(account, "atomic-migration", RequestId::generate(), 100, 100)
        .unwrap();
    let plan = create_plan();
    DurableObjectRepository::new(&storage)
        .prepare_worker_migration(account, worker.id, &plan, 101)
        .unwrap();
    let version = insert_validating_version(&storage, account, worker.id, 102);
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER fail_do_migration_publish
                 BEFORE INSERT ON worker_do_migrations
                 BEGIN
                   SELECT RAISE(ABORT, 'forced Durable Object migration publish failure');
                 END;",
            )
            .map_err(|_| PlatformError::new(ErrorCode::Internal, "test trigger failed"))?;
            Ok(())
        })
        .unwrap();

    assert!(
        WorkerRepository::new(storage.db())
            .mark_ready_with_durable_object_migration(version, worker.id, &plan, 103)
            .is_err()
    );
    assert_eq!(
        WorkerRepository::new(storage.db())
            .get_worker_version(account, worker.id, version)
            .unwrap()
            .state,
        crate::VersionState::Validating
    );
    assert!(
        DurableObjectRepository::new(&storage)
            .current_worker_migration(worker.id)
            .unwrap()
            .is_none()
    );
    assert!(
        DurableObjectRepository::new(&storage)
            .list_namespaces(account)
            .unwrap()
            .is_empty()
    );
    drop(storage);

    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    assert_eq!(
        WorkerRepository::new(storage.db())
            .get_worker_version(account, worker.id, version)
            .unwrap()
            .state,
        crate::VersionState::Validating
    );
    assert!(
        DurableObjectRepository::new(&storage)
            .current_worker_migration(worker.id)
            .unwrap()
            .is_none()
    );
    assert!(
        DurableObjectRepository::new(&storage)
            .namespace_for_worker_upload(account, worker.id, "Counter", Some("v1"))
            .is_ok()
    );
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute_batch("DROP TRIGGER fail_do_migration_publish")
                .map_err(|_| PlatformError::new(ErrorCode::Internal, "test trigger failed"))?;
            Ok(())
        })
        .unwrap();
    WorkerRepository::new(storage.db())
        .mark_ready_with_durable_object_migration(version, worker.id, &plan, 104)
        .unwrap();
    drop(storage);

    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let ready = WorkerRepository::new(storage.db())
        .get_worker_version(account, worker.id, version)
        .unwrap();
    assert_eq!(ready.state, crate::VersionState::Ready);
    let repository = DurableObjectRepository::new(&storage);
    let head = repository
        .current_worker_migration(worker.id)
        .unwrap()
        .unwrap();
    assert_eq!(head.version_id, version);
    assert_eq!(head.plan_sha256, plan.fingerprint().unwrap());
    assert_eq!(repository.list_namespaces(account).unwrap().len(), 1);
    let history_is_immutable = storage
        .db()
        .with_immediate(|tx| {
            let update_rejected = tx
                .execute(
                    "UPDATE worker_do_migrations SET plan_sha256 = zeroblob(32)
                     WHERE worker_id = ?1 AND tag = ?2",
                    rusqlite::params![worker.id.to_string(), plan.new_tag],
                )
                .is_err();
            let delete_rejected = tx
                .execute(
                    "DELETE FROM worker_do_migrations WHERE worker_id = ?1 AND tag = ?2",
                    rusqlite::params![worker.id.to_string(), plan.new_tag],
                )
                .is_err();
            Ok(update_rejected && delete_rejected)
        })
        .unwrap();
    assert!(history_is_immutable);

    let second_version = insert_validating_version(&storage, account, worker.id, 105);
    let validating_version_cannot_publish = storage
        .db()
        .with_immediate(|tx| {
            Ok(tx
                .execute(
                    "INSERT INTO worker_do_migrations
                     (worker_id, tag, plan_sha256, version_id, created_at_ms)
                     VALUES (?1, 'invalid-v2', zeroblob(32), ?2, 105)",
                    rusqlite::params![worker.id.to_string(), second_version.to_string()],
                )
                .is_err())
        })
        .unwrap();
    assert!(validating_version_cannot_publish);
    assert_eq!(
        repository
            .validate_worker_migration_version(worker.id, second_version, &plan)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        WorkerRepository::new(storage.db())
            .mark_ready_with_durable_object_migration(second_version, worker.id, &plan, 106)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        WorkerRepository::new(storage.db())
            .get_worker_version(account, worker.id, second_version)
            .unwrap()
            .state,
        crate::VersionState::Validating
    );
    assert_eq!(
        repository
            .current_worker_migration(worker.id)
            .unwrap()
            .unwrap()
            .version_id,
        version
    );

    let conflicting_plan = DurableObjectMigrationPlan {
        new_sqlite_classes: vec!["Different".to_owned()],
        ..plan
    };
    assert_eq!(
        repository
            .prepare_worker_migration(account, worker.id, &conflicting_plan, 107)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
}
