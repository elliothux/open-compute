use super::*;
use crate::ControlDb;

fn reopen_storage(temp: &tempfile::TempDir) -> PlatformStorage {
    let root = temp.path().join("data");
    PlatformStorage::bootstrap(
        &DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1024 * 1024 * 1024,
            free_space_hard_bytes: 256 * 1024 * 1024,
        },
        &SystemClock,
    )
    .unwrap()
}

fn verify_reopened(temp: &tempfile::TempDir) {
    let reopened =
        ControlDb::open_readonly_wal_aware(&temp.path().join("data/control.sqlite"), 5_000)
            .unwrap();
    WorkflowRepository::new(&reopened).verify_catalog().unwrap();
}

#[test]
fn workflow_upload_reservation_freezes_class_and_fails_closed_until_ready() {
    let (_tmp, storage, target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let first = repo
        .reserve_definition(account, "upload-first", "Flow", "upload-a", 1)
        .unwrap();
    assert!(first.created_definition);
    assert_eq!(first.definition.state, ResourceState::Creating);
    assert_eq!(
        first.definition.reserved_class_name.as_deref(),
        Some("Flow")
    );
    assert_eq!(
        repo.reserve_definition(account, "upload-first", "Flow", "upload-a", 2)
            .unwrap()
            .fence,
        first.fence
    );
    assert_eq!(
        repo.reserve_definition(account, "upload-first", "Other", "upload-b", 2)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowNameConflict
    );
    let reclaimed = repo
        .reserve_definition(account, "upload-first", "Flow", "upload-b", 3)
        .unwrap();
    assert!(!reclaimed.created_definition);
    assert_eq!(reclaimed.fence, first.fence + 1);

    let workers = WorkerRepository::new(storage.db());
    let caller_worker = workers
        .create_worker(
            account,
            "upload-first-caller",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap()
        .0;
    let caller = staging(&storage, caller_worker.id);
    let binding = repo
        .prepare_binding(
            account,
            caller,
            "FLOW",
            reclaimed.definition.id,
            "Flow",
            Some(&reclaimed.owner),
            Some(reclaimed.fence),
            Vec::new(),
            4,
        )
        .unwrap();
    assert_eq!(
        repo.prepare_binding(
            account,
            caller,
            "STALE",
            reclaimed.definition.id,
            "Flow",
            Some(&first.owner),
            Some(first.fence),
            Vec::new(),
            4,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowNotReady
    );
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, caller, std::slice::from_ref(&binding))
        })
        .unwrap();
    workers.begin_validation(caller).unwrap();
    workers.mark_ready(caller, 5).unwrap();
    assert_eq!(
        repo.authorize_binding(
            binding.descriptor.binding_id,
            caller,
            &binding.descriptor_sha256,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowBindingStale
    );

    let workflow_version = repo
        .stage_reserved_version(
            account,
            reclaimed.definition.id,
            target_version,
            "Flow",
            &reclaimed,
            6,
        )
        .unwrap();
    repo.finish_version(
        account,
        workflow_version.target.workflow_version_id,
        true,
        7,
    )
    .unwrap();
    assert!(
        repo.definition(account, reclaimed.definition.id)
            .unwrap()
            .reserved_class_name
            .is_none()
    );
    repo.authorize_binding(
        binding.descriptor.binding_id,
        caller,
        &binding.descriptor_sha256,
    )
    .unwrap();

    let cancelled = repo
        .reserve_definition(account, "upload-first", "Flow", "cancelled-update", 8)
        .unwrap();
    assert!(
        repo.release_definition_reservation(account, &cancelled, 9)
            .unwrap()
    );
    let after_cancel = repo.definition(account, cancelled.definition.id).unwrap();
    assert_eq!(after_cancel.state, ResourceState::Ready);
    assert!(after_cancel.reserved_class_name.is_none());

    let class_change = repo
        .reserve_definition(account, "upload-first", "Other", "upload-c", 10)
        .unwrap();
    let next_caller_worker = workers
        .create_worker(
            account,
            "upload-first-next-caller",
            RequestId::generate(),
            10,
            1_000_000,
        )
        .unwrap()
        .0;
    let next_caller = staging(&storage, next_caller_worker.id);
    let next_binding = repo
        .prepare_binding(
            account,
            next_caller,
            "FLOW",
            class_change.definition.id,
            "Other",
            Some(&class_change.owner),
            Some(class_change.fence),
            Vec::new(),
            11,
        )
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, next_caller, std::slice::from_ref(&next_binding))
        })
        .unwrap();
    workers.begin_validation(next_caller).unwrap();
    workers.mark_ready(next_caller, 12).unwrap();
    assert_eq!(
        repo.authorize_binding(
            next_binding.descriptor.binding_id,
            next_caller,
            &next_binding.descriptor_sha256,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowBindingStale
    );
    let changed = repo
        .stage_reserved_version(
            account,
            class_change.definition.id,
            target_version,
            "Other",
            &class_change,
            13,
        )
        .unwrap();
    repo.finish_version(account, changed.target.workflow_version_id, true, 14)
        .unwrap();
    repo.authorize_binding(
        next_binding.descriptor.binding_id,
        next_caller,
        &next_binding.descriptor_sha256,
    )
    .unwrap();
    assert_eq!(
        repo.authorize_binding(
            binding.descriptor.binding_id,
            caller,
            &binding.descriptor_sha256,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowBindingStale
    );

    let without_current = repo
        .create_definition(account, "without-current", 15)
        .unwrap();
    assert_eq!(
        repo.prepare_binding(
            account,
            caller,
            "MISSING_CURRENT",
            without_current.id,
            "Flow",
            None,
            None,
            Vec::new(),
            16,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkflowNotReady
    );
    let claimed = repo
        .reserve_definition(account, "without-current", "Flow", "upload-d", 17)
        .unwrap();
    assert_eq!(claimed.definition.id, without_current.id);
    assert_eq!(
        claimed.definition.reserved_class_name.as_deref(),
        Some("Flow")
    );

    let abandoned = repo
        .reserve_definition(account, "abandoned", "Flow", "upload-e", 18)
        .unwrap();
    assert!(
        repo.release_definition_reservation(account, &abandoned, 19)
            .unwrap()
    );
    assert_eq!(
        repo.definitions(
            account,
            Some("abandoned"),
            None,
            CatalogSort::Name,
            CatalogDirection::Asc,
            None,
            10,
        )
        .unwrap()
        .items
        .len(),
        0
    );
    repo.verify_catalog().unwrap();
}

#[test]
fn workflow_upload_refence_retains_stale_evidence_across_reopen_and_reconcile() {
    let (temp, storage, target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let upload = repo
        .reserve_definition(account, "refenced", "Flow", "worker-upload", 1)
        .unwrap();
    let workers = WorkerRepository::new(storage.db());
    let caller_worker = workers
        .create_worker(
            account,
            "refenced-caller",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap()
        .0;
    let caller = staging(&storage, caller_worker.id);
    let binding = repo
        .prepare_binding(
            account,
            caller,
            "FLOW",
            upload.definition.id,
            "Flow",
            Some(&upload.owner),
            Some(upload.fence),
            Vec::new(),
            3,
        )
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, caller, std::slice::from_ref(&binding))
        })
        .unwrap();
    workers.begin_validation(caller).unwrap();
    workers.mark_ready(caller, 4).unwrap();
    verify_reopened(&temp);

    let put = repo
        .reserve_definition(account, "refenced", "Flow", "workflow-put", 5)
        .unwrap();
    assert!(put.fence > upload.fence);
    verify_reopened(&temp);
    let pending = repo
        .stage_reserved_version(account, put.definition.id, target_version, "Flow", &put, 6)
        .unwrap();
    verify_reopened(&temp);

    let retry = repo
        .reserve_definition(account, "refenced", "Flow", "workflow-retry", 7)
        .unwrap();
    assert!(retry.fence > put.fence);
    verify_reopened(&temp);
    let stale = repo
        .finish_version(account, pending.target.workflow_version_id, true, 8)
        .unwrap();
    assert_eq!(stale.state, VersionState::Rejected);
    verify_reopened(&temp);

    let current = repo
        .stage_reserved_version(
            account,
            retry.definition.id,
            target_version,
            "Flow",
            &retry,
            9,
        )
        .unwrap();
    repo.finish_version(account, current.target.workflow_version_id, true, 10)
        .unwrap();
    verify_reopened(&temp);
    repo.authorize_binding(
        binding.descriptor.binding_id,
        caller,
        &binding.descriptor_sha256,
    )
    .unwrap();
}

#[test]
fn terminal_workflow_rejection_allows_a_different_class_after_restart() {
    let (temp, storage, target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;

    let rejected_probe = repo
        .reserve_definition(account, "probe-rejected", "Missing", "workflow-put", 1)
        .unwrap();
    let rejected_version = repo
        .stage_reserved_version(
            account,
            rejected_probe.definition.id,
            target_version,
            "Missing",
            &rejected_probe,
            2,
        )
        .unwrap();
    let rejected = repo
        .finish_version(
            account,
            rejected_version.target.workflow_version_id,
            false,
            3,
        )
        .unwrap();
    assert_eq!(rejected.state, VersionState::Rejected);
    assert!(
        repo.definition(account, rejected_probe.definition.id)
            .unwrap()
            .reserved_class_name
            .is_none()
    );
    drop(storage);
    let reopened = reopen_storage(&temp);
    let repo = WorkflowRepository::new(reopened.db());
    repo.verify_catalog().unwrap();
    repo.reserve_definition(account, "probe-rejected", "Corrected", "workflow-retry", 4)
        .unwrap();
}

#[test]
fn terminal_worker_rejection_allows_a_different_class_after_restart() {
    let (temp, storage, _target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let rejected_upload = repo
        .reserve_definition(account, "upload-rejected", "Missing", "worker-upload", 1)
        .unwrap();
    let workers = WorkerRepository::new(storage.db());
    let caller_worker = workers
        .create_worker(
            account,
            "rejected-upload-caller",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap()
        .0;
    let caller = staging(&storage, caller_worker.id);
    let binding = repo
        .prepare_binding(
            account,
            caller,
            "FLOW",
            rejected_upload.definition.id,
            "Missing",
            Some(&rejected_upload.owner),
            Some(rejected_upload.fence),
            Vec::new(),
            3,
        )
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, caller, std::slice::from_ref(&binding))
        })
        .unwrap();
    workers.begin_validation(caller).unwrap();
    workers
        .mark_rejected(
            caller,
            VersionState::Validating,
            ErrorCode::BundleRuntimeInvalid,
            4,
        )
        .unwrap();
    verify_reopened(&temp);
    assert!(
        repo.definition(account, rejected_upload.definition.id)
            .unwrap()
            .reserved_class_name
            .is_none()
    );
    drop(storage);
    let reopened = reopen_storage(&temp);
    let repo = WorkflowRepository::new(reopened.db());
    repo.verify_catalog().unwrap();
    let corrected = repo
        .reserve_definition(account, "upload-rejected", "Corrected", "worker-retry", 5)
        .unwrap();
    assert!(corrected.fence > rejected_upload.fence);
    verify_reopened(&temp);
}

#[test]
fn deleting_the_last_worker_consumer_releases_its_fence_across_recovery() {
    let (temp, storage, _target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let upload = repo
        .reserve_definition(account, "retired-upload", "Missing", "worker-upload", 1)
        .unwrap();
    let workers = WorkerRepository::new(storage.db());
    let caller_worker = workers
        .create_worker(
            account,
            "retired-upload-caller",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap()
        .0;
    let caller = staging(&storage, caller_worker.id);
    let binding = repo
        .prepare_binding(
            account,
            caller,
            "FLOW",
            upload.definition.id,
            "Missing",
            Some(&upload.owner),
            Some(upload.fence),
            Vec::new(),
            3,
        )
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, caller, std::slice::from_ref(&binding))
        })
        .unwrap();
    workers.begin_validation(caller).unwrap();
    workers.mark_ready(caller, 4).unwrap();
    workers
        .begin_version_delete(account, caller_worker.id, caller)
        .unwrap();
    assert!(
        repo.definition(account, upload.definition.id)
            .unwrap()
            .reserved_class_name
            .is_none()
    );
    drop(storage);

    let reopened = reopen_storage(&temp);
    let repo = WorkflowRepository::new(reopened.db());
    repo.verify_catalog().unwrap();
    let corrected = repo
        .reserve_definition(account, "retired-upload", "Corrected", "worker-retry", 5)
        .unwrap();
    assert!(corrected.fence > upload.fence);
    assert_eq!(
        WorkerRepository::new(reopened.db())
            .recover_deleting_versions(RequestId::generate(), 6, 100)
            .unwrap(),
        1
    );
    repo.verify_catalog().unwrap();
}

#[test]
fn current_fence_class_corruption_fails_catalog_integrity() {
    let (_temp, storage, _target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let upload = repo
        .reserve_definition(account, "integrity-upload", "Flow", "worker-upload", 1)
        .unwrap();
    let workers = WorkerRepository::new(storage.db());
    let caller_worker = workers
        .create_worker(
            account,
            "integrity-upload-caller",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap()
        .0;
    let caller = staging(&storage, caller_worker.id);
    let binding = repo
        .prepare_binding(
            account,
            caller,
            "FLOW",
            upload.definition.id,
            "Flow",
            Some(&upload.owner),
            Some(upload.fence),
            Vec::new(),
            3,
        )
        .unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            bindings::insert_workflow_bindings(tx, caller, std::slice::from_ref(&binding))
        })
        .unwrap();
    workers.begin_validation(caller).unwrap();
    workers.mark_ready(caller, 4).unwrap();
    let mut corrupted = binding.descriptor.clone();
    corrupted.class_name = "Other".into();
    let digest = corrupted.sha256().unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute_batch("DROP TRIGGER workflow_binding_immutable;")
                .map_err(sql_error)?;
            tx.execute(
                "UPDATE workflow_bindings SET class_name=?2,descriptor_sha256=?3 WHERE id=?1",
                params![
                    binding.descriptor.binding_id.to_string(),
                    "Other",
                    digest.as_slice()
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.verify_catalog().unwrap_err().code(),
        ErrorCode::WorkflowInvariantViolation
    );
}

#[test]
fn current_fence_version_class_corruption_fails_catalog_integrity() {
    let (_temp, storage, target_version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let reservation = repo
        .reserve_definition(account, "integrity-version", "Flow", "workflow-put", 1)
        .unwrap();
    let version = repo
        .stage_reserved_version(
            account,
            reservation.definition.id,
            target_version,
            "Flow",
            &reservation,
            2,
        )
        .unwrap();
    let mut corrupted = version.target.clone();
    corrupted.class_name = "Other".into();
    let digest = version_digest(&corrupted).unwrap();
    storage
        .db()
        .with_immediate(|tx| {
            tx.execute_batch("DROP TRIGGER workflow_version_identity_guard;")
                .map_err(sql_error)?;
            tx.execute(
                "UPDATE workflow_versions SET class_name=?2,descriptor_sha256=?3 WHERE id=?1",
                params![
                    version.target.workflow_version_id.to_string(),
                    "Other",
                    digest.as_slice()
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.verify_catalog().unwrap_err().code(),
        ErrorCode::WorkflowInvariantViolation
    );
}

#[test]
fn workflow_delete_reports_a_stable_conflict_for_a_pending_reservation() {
    let (_temp, storage, version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = ready(&storage, version);
    let pending = repo
        .reserve_definition(account, "orders", "Replacement", "pending-update", 3)
        .unwrap();
    assert_eq!(
        repo.delete(account, definition.id, 4).unwrap_err().code(),
        ErrorCode::WorkflowReferenced
    );
    assert!(
        repo.release_definition_reservation(account, &pending, 5)
            .unwrap()
    );
    assert_eq!(
        repo.delete(account, definition.id, 6).unwrap().state,
        ResourceState::Tombstoned
    );
}

#[test]
fn delete_intent_and_upload_reservation_have_one_linearization_winner() {
    let (temp, storage, version) = setup();
    let repo = WorkflowRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let definition = ready(&storage, version);
    let path = temp.path().join("data/control.sqlite");
    let delete_db = ControlDb::open(&path, 5_000).unwrap();
    let reserve_db = ControlDb::open(&path, 5_000).unwrap();
    let definition_id = definition.id;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let (deleting, reserving) = std::thread::scope(|scope| {
        let delete_barrier = barrier.clone();
        let delete = scope.spawn(move || {
            delete_barrier.wait();
            WorkflowRepository::new(&delete_db).begin_definition_delete(account, "orders", 3)
        });
        let reserve_barrier = barrier.clone();
        let reserve = scope.spawn(move || {
            reserve_barrier.wait();
            WorkflowRepository::new(&reserve_db).reserve_definition(
                account,
                "orders",
                "Replacement",
                "racing-upload",
                3,
            )
        });
        (delete.join().unwrap(), reserve.join().unwrap())
    });

    match (deleting, reserving) {
        (Ok(intent), Err(error)) => {
            assert_eq!(error.code(), ErrorCode::WorkflowNameConflict);
            repo.finish_definition_delete(account, &intent, 4).unwrap();
        }
        (Err(error), Ok(reservation)) => {
            assert_eq!(error.code(), ErrorCode::WorkflowReferenced);
            assert!(
                repo.release_definition_reservation(account, &reservation, 4)
                    .unwrap()
            );
            repo.delete(account, definition_id, 5).unwrap();
        }
        _ => panic!("delete and reservation must have exactly one winner"),
    }
    repo.verify_catalog().unwrap();
}

#[test]
fn delete_intent_can_finish_after_a_real_restart() {
    let (temp, storage, version) = setup();
    let account = storage.identity().default_account_id;
    let _definition = ready(&storage, version);
    let intent = WorkflowRepository::new(storage.db())
        .begin_definition_delete(account, "orders", 3)
        .unwrap();
    drop(storage);

    let reopened = reopen_storage(&temp);
    let repo = WorkflowRepository::new(reopened.db());
    assert_eq!(
        repo.finish_definition_delete(account, &intent, 4)
            .unwrap()
            .state,
        ResourceState::Tombstoned
    );
    repo.verify_catalog().unwrap();
}
