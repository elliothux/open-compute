use super::*;
use open_compute_core::{DeploymentId, RequestId, StorageConfig, SystemClock};
use open_compute_storage::scheduler::{
    WorkflowCompletion, WorkflowStepGrant, WorkflowStepIdentity,
};
use open_compute_storage::{NewDeployment, WorkerRepository};

#[path = "workflow_crash_tests.rs"]
mod crash_matrix;
#[path = "workflow_lifecycle_tests.rs"]
mod durable_lifecycle;

fn fixture() -> (
    tempfile::TempDir,
    PlatformStorage,
    SchedulerStore,
    WorkflowId,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1024 * 1024 * 1024,
        free_space_hard_bytes: 256 * 1024 * 1024,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler =
        SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 5000, 0).unwrap();
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "workflow-owner", RequestId::generate(), 0)
        .unwrap();
    let deployment = DeploymentId::generate();
    workers
        .insert_staging_deployment(&NewDeployment {
            id: deployment,
            account_id: account,
            worker_id: worker.id,
            artifact_sha256: [1; 32],
            artifact_size: 100,
            artifact_schema_version: 1,
            main_module: "index.js".into(),
            compatibility_date: "2026-08-26".into(),
            compatibility_flags: vec![],
            limits: serde_json::json!({"profile":"default"}),
            worker_code_sha256: [2; 32],
            vars: Default::default(),
            secrets: Default::default(),
            request_id: RequestId::generate(),
            now_ms: 0,
        })
        .unwrap();
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 1).unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows.create_definition(account, "flow", 2).unwrap();
    let version = workflows
        .stage_version(account, definition.id, deployment, "Flow", 1, 3)
        .unwrap();
    workflows
        .finish_version(account, version.target.version_id, true, 4)
        .unwrap();
    (temp, storage, scheduler, definition.id)
}

#[test]
fn workflow_create_status_duplicate_and_terminal_release_saga() {
    let (_temp, storage, scheduler, definition) = fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    assert_eq!(
        controller
            .status(account, definition, WorkflowInstanceId::generate(), 1, 0)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );
    let identity = controller
        .create(
            account,
            definition,
            1,
            Some("order"),
            WorkflowCreateInput {
                payload_json: "{}",
                retention: None,
            },
            10,
        )
        .unwrap();
    let id = &identity.external_instance_id;
    assert_eq!(id, "order");
    assert!(matches!(
        controller
            .status(account, definition, identity.instance_id, 1, 0)
            .unwrap(),
        WorkflowStatus::Queued
    ));
    assert_eq!(
        controller
            .create(
                account,
                definition,
                1,
                Some("order"),
                WorkflowCreateInput {
                    payload_json: "{}",
                    retention: None
                },
                11
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceAlreadyExists
    );
    let run = controller
        .claim(12, &mut Default::default())
        .unwrap()
        .unwrap();
    assert!(matches!(
        controller
            .status(account, definition, identity.instance_id, 1, 0)
            .unwrap(),
        WorkflowStatus::Running
    ));
    let step = WorkflowStepIdentity {
        ordinal: 0,
        name: "compute".into(),
        name_count: 1,
        config_json: "null".into(),
    };
    let WorkflowStepGrant::Run { step_token } = scheduler
        .claim_workflow_step(&run.fence, &step, 13, &config)
        .unwrap()
    else {
        panic!("expected grant")
    };
    scheduler
        .complete_workflow_step(&run.fence, 0, &step_token, "42", 14, &config)
        .unwrap();
    scheduler
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: "42".into(),
                final_ordinal: 1,
            },
            15,
            &config,
        )
        .unwrap();
    let repository = WorkflowRepository::new(storage.db());
    let reservation = repository.find_instance(definition, id).unwrap();
    assert_eq!(reservation.state, WorkflowRefState::Live);
    assert!(
        matches!(controller.status(account,definition,identity.instance_id,1, 0).unwrap(),WorkflowStatus::Complete {output} if output.as_f64()==Some(42.0))
    );
    controller
        .reconcile(&mut WorkflowReconcileCursor::default(), 10, 16)
        .unwrap();
    assert_eq!(
        repository.find_instance(definition, id).unwrap().state,
        WorkflowRefState::Released
    );
    assert!(
        !repository
            .instance_referrers_intact(&reservation.identity)
            .unwrap()
    );
    assert!(
        controller
            .claim(17, &mut Default::default())
            .unwrap()
            .is_none()
    );
    let wire = serde_json::to_string(
        &controller
            .status(account, definition, identity.instance_id, 1, 0)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(wire, r#"{"status":"complete","output":42.0}"#);
}

#[test]
fn workflow_reconcile_creation_crashes_and_bounded_grace() {
    let (_temp, storage, scheduler, definition) = fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let repository = WorkflowRepository::new(storage.db());
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    let absent = repository
        .reserve_instance(account, definition, Some("aborted"), 1, &config, 10)
        .unwrap();
    let durable = repository
        .reserve_instance(account, definition, Some("durable"), 1, &config, 10)
        .unwrap();
    scheduler
        .insert_workflow(&durable.identity, "null", None, &config)
        .unwrap();
    assert!(
        controller
            .claim(11, &mut Default::default())
            .unwrap()
            .is_none()
    );
    controller
        .reconcile(&mut WorkflowReconcileCursor::default(), 10, 11)
        .unwrap();
    assert_eq!(
        repository
            .reservation(durable.identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Live
    );
    assert_eq!(
        repository
            .reservation(absent.identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Creating
    );
    assert!(
        controller
            .claim(12, &mut Default::default())
            .unwrap()
            .is_none()
    );
    let run = controller
        .claim(1011, &mut Default::default())
        .unwrap()
        .unwrap();
    assert_eq!(run.fence.instance_id, durable.identity.instance_id);
    controller
        .reconcile(&mut WorkflowReconcileCursor::default(), 10, 60_010)
        .unwrap();
    assert!(
        repository
            .reservation(absent.identity.instance_id)
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .instance_referrers_intact(&durable.identity)
            .unwrap()
    );
}

#[test]
fn workflow_orphan_scheduler_authority_is_fenced_without_guessing_history() {
    let (_temp, storage, scheduler, definition) = fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let repository = WorkflowRepository::new(storage.db());
    let reservation = repository
        .reserve_instance(account, definition, None, 1, &config, 10)
        .unwrap();
    scheduler
        .insert_workflow(&reservation.identity, "null", None, &config)
        .unwrap();
    // Explicit test-only cross-database fault: remove an unfinalized control reservation.
    repository.abandon_creation(&reservation.identity).unwrap();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    assert_eq!(
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 10, 11)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    assert_eq!(
        repository
            .definition(account, definition)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );
    assert_eq!(
        controller
            .claim(12, &mut Default::default())
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    assert_eq!(
        scheduler
            .workflow_instance(reservation.identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Queued
    );
}

#[test]
fn workflow_draining_and_invalid_payload_fail_before_reservation() {
    let (_temp, storage, scheduler, definition) = fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    assert_eq!(
        controller
            .create(
                account,
                definition,
                1,
                Some("bad"),
                WorkflowCreateInput {
                    payload_json: "[",
                    retention: None
                },
                10
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    storage.begin_draining();
    assert_eq!(
        controller
            .create(
                account,
                definition,
                1,
                Some("draining"),
                WorkflowCreateInput {
                    payload_json: "null",
                    retention: None
                },
                10
            )
            .unwrap_err()
            .code(),
        ErrorCode::PlatformUnavailable
    );
    assert!(
        WorkflowRepository::new(storage.db())
            .live_reservations(None, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        scheduler
            .workflow_instance_ids(None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn workflow_readonly_diagnostics_and_claim_reject_corrupt_retained_accounting() {
    let (_temp, storage, scheduler, definition) = fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let repository = WorkflowRepository::new(storage.db());
    let reservation = repository
        .reserve_instance(account, definition, Some("retained"), 1, &config, 10)
        .unwrap();
    let inspect = || {
        open_compute_storage::scheduler::inspect_workflow_databases(
            &storage.data_dir().control_db_path(),
            &storage.data_dir().scheduler_db_path(),
            5000,
            32,
        )
        .unwrap()
    };
    assert_eq!(inspect().pending_creations, 1);
    assert!(inspect().is_valid());
    scheduler
        .insert_workflow(&reservation.identity, "null", None, &config)
        .unwrap();
    assert_eq!(inspect().pending_creations, 1);
    repository
        .finalize_instance(&reservation.identity, 11)
        .unwrap();
    assert_eq!(inspect().pending_creations, 0);
    // Deliberately corrupt only this fixture's persisted counter after proving the
    // ordinary SQL guard rejects the write. Neither diagnostics nor claim may repair it.
    let connection = rusqlite::Connection::open(storage.data_dir().scheduler_db_path()).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE workflow_instances SET state_bytes=state_bytes+1",
                []
            )
            .is_err()
    );
    connection
        .execute_batch(
            "DROP TRIGGER workflow_v1_instance_count_guard;
        UPDATE workflow_instances SET state_bytes=state_bytes+1;",
        )
        .unwrap();
    let view = inspect();
    assert_eq!(view.history_mismatches, 1);
    assert!(!view.is_valid());
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    assert_eq!(
        controller
            .claim(12, &mut Default::default())
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    assert_eq!(
        repository
            .definition(account, definition)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );
    let row = scheduler
        .workflow_instance(reservation.identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.state, WorkflowState::Queued);
    assert_eq!(row.state_bytes, 5);
    assert_eq!(inspect().history_mismatches, 1);
}

#[test]
fn workflow_history_prevents_empty_scheduler_replacement() {
    let (_temp, storage, scheduler, definition) = fixture();
    let config = WorkflowsConfig::default();
    WorkflowController::new(&storage, &scheduler, &config)
        .create(
            storage.identity().default_account_id,
            definition,
            1,
            Some("retained"),
            WorkflowCreateInput {
                payload_json: "null",
                retention: None,
            },
            10,
        )
        .unwrap();
    drop(scheduler);
    let path = storage.data_dir().scheduler_db_path();
    std::fs::write(&path, b"corrupt workflow authority").unwrap();
    assert_eq!(
        storage
            .data_dir()
            .recover_corrupt_scheduler_db("scheduler-corrupt-workflow", 5000, 13)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerUnavailable
    );
    assert!(
        !storage
            .data_dir()
            .root()
            .join("diagnostics/scheduler-recovery/scheduler-corrupt-workflow")
            .exists()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"corrupt workflow authority");
}
