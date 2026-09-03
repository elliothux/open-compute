use super::*;
use crate::ControlDb;

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
fn terminal_reservation_rejections_allow_a_different_class_after_restart() {
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
    repo.reserve_definition(account, "probe-rejected", "Corrected", "workflow-retry", 4)
        .unwrap();

    let rejected_upload = repo
        .reserve_definition(account, "upload-rejected", "Missing", "worker-upload", 5)
        .unwrap();
    let workers = WorkerRepository::new(storage.db());
    let caller_worker = workers
        .create_worker(
            account,
            "rejected-upload-caller",
            RequestId::generate(),
            6,
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
            7,
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
            8,
        )
        .unwrap();
    verify_reopened(&temp);
    assert!(
        repo.definition(account, rejected_upload.definition.id)
            .unwrap()
            .reserved_class_name
            .is_none()
    );

    let corrected = repo
        .reserve_definition(account, "upload-rejected", "Corrected", "worker-retry", 9)
        .unwrap();
    assert!(corrected.fence > rejected_upload.fence);
    verify_reopened(&temp);
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
