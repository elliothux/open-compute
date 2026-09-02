use super::*;
use open_compute_core::{RequestId, StorageConfig, SystemClock, VersionId};
use open_compute_storage::scheduler::{
    WorkflowClaimCursor, WorkflowFailure, WorkflowInstanceAction,
};
use open_compute_storage::{NewVersion, WorkerRepository};

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
        .create_worker(
            account,
            "workflow-owner",
            RequestId::generate(),
            0,
            1_000_000,
        )
        .unwrap();
    let version = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: open_compute_storage::VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 0,
            },
            &open_compute_storage::NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 1).unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows.create_definition(account, "flow", 2).unwrap();
    let version = workflows
        .stage_version(account, definition.id, version, "Flow", 3)
        .unwrap();
    workflows
        .finish_version(account, version.target.workflow_version_id, true, 4)
        .unwrap();
    (temp, storage, scheduler, definition.id)
}

#[test]
fn create_batch_restart_recovery_publishes_or_abandons_the_complete_group() {
    for scheduler_committed in [false, true] {
        let (_temp, storage, scheduler, definition) = fixture();
        let account = storage.identity().default_account_id;
        let config = WorkflowsConfig::default();
        let batch = WorkflowOperationId::generate();
        let operations = [
            WorkflowOperationId::generate(),
            WorkflowOperationId::generate(),
        ];
        let reservation_requests = [
            (operations[0], Some("restart-batch-a")),
            (operations[1], Some("restart-batch-b")),
        ];
        let repository = WorkflowRepository::new(storage.db());
        let reservations = repository
            .reserve_instances(
                account,
                definition,
                batch,
                &reservation_requests,
                &config,
                10,
            )
            .unwrap();
        let identities = reservations
            .iter()
            .map(|row| row.identity.clone())
            .collect::<Vec<_>>();
        if scheduler_committed {
            let scheduler_requests = identities
                .iter()
                .map(|identity| (identity, "T0NEVgECAA==", Some(&config.default_retention)))
                .collect::<Vec<_>>();
            scheduler
                .insert_workflows(&scheduler_requests, &config)
                .unwrap();
        }
        let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
        drop(scheduler);
        let scheduler = SchedulerStore::open(&scheduler_path, 5000, 11).unwrap();
        let now_ms = if scheduler_committed {
            11
        } else {
            10 + i64::try_from(config.creation_grace_ms).unwrap()
        };
        WorkflowController::new(&storage, &scheduler, &config)
            .reconcile(&mut WorkflowReconcileCursor::default(), 32, now_ms)
            .unwrap();
        for identity in identities {
            let reservation = repository.reservation(identity.instance_id).unwrap();
            if scheduler_committed {
                assert_eq!(reservation.unwrap().state, WorkflowRefState::Live);
                assert_eq!(
                    scheduler
                        .workflow_instance(identity.instance_id)
                        .unwrap()
                        .unwrap()
                        .identity,
                    identity
                );
            } else {
                assert!(reservation.is_none());
                assert!(
                    scheduler
                        .workflow_instance(identity.instance_id)
                        .unwrap()
                        .is_none()
                );
            }
        }
    }
}

#[test]
fn current_controller_validates_reports_and_deletes_through_one_lifecycle() {
    const NULL_VALUE: &str = "T0NEVgECAA==";

    let (_temp, storage, scheduler, definition) = fixture();
    let account = storage.identity().default_account_id;
    let limits = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &limits);
    let input = WorkflowCreateInput {
        payload_base64: NULL_VALUE,
        retention: None,
        schedule: None,
    };
    assert_eq!(format!("{input:?}"), "WorkflowCreateInput([REDACTED])");
    let event = WorkflowEventInput {
        operation_id: WorkflowOperationId::generate(),
        event_type: "ready",
        payload_base64: NULL_VALUE,
    };
    let event_debug = format!("{event:?}");
    assert!(event_debug.contains("event_type: \"ready\""));
    assert!(event_debug.contains("payload: \"[REDACTED]\""));

    assert_eq!(
        controller
            .create_batch(
                account,
                definition,
                WorkflowOperationId::generate(),
                &[],
                10,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
    );
    let oversized = vec![(WorkflowOperationId::generate(), None, input); 101];
    assert_eq!(
        controller
            .create_batch(
                account,
                definition,
                WorkflowOperationId::generate(),
                &oversized,
                10,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
    );
    assert_eq!(
        controller
            .create(
                account,
                definition,
                WorkflowOperationId::generate(),
                None,
                WorkflowCreateInput {
                    payload_base64: "not-base64",
                    retention: None,
                    schedule: None,
                },
                10,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowPayloadTooLarge
    );

    let identity = controller
        .create(
            account,
            definition,
            WorkflowOperationId::generate(),
            Some("lifecycle"),
            input,
            11,
        )
        .unwrap();
    assert_eq!(
        controller
            .inspect(account, definition, identity.instance_id, 11)
            .unwrap()
            .status,
        WorkflowState::Queued
    );
    assert!(matches!(
        controller
            .status(account, definition, identity.instance_id, 11)
            .unwrap(),
        WorkflowStatus::Queued
    ));
    controller
        .modify(
            account,
            definition,
            identity.instance_id,
            WorkflowInstanceAction::Pause,
            12,
        )
        .unwrap();
    assert!(matches!(
        controller
            .status(account, definition, identity.instance_id, 12)
            .unwrap(),
        WorkflowStatus::Paused
    ));
    controller
        .modify(
            account,
            definition,
            identity.instance_id,
            WorkflowInstanceAction::Resume,
            13,
        )
        .unwrap();
    controller
        .send_event(account, definition, identity.instance_id, event, 13)
        .unwrap();
    let run = controller
        .claim(14, &mut WorkflowClaimCursor::default())
        .unwrap()
        .unwrap();
    assert_eq!(run.fence.instance_id, identity.instance_id);
    assert!(matches!(
        controller
            .status(account, definition, identity.instance_id, 14)
            .unwrap(),
        WorkflowStatus::Running
    ));
    controller
        .modify(
            account,
            definition,
            identity.instance_id,
            WorkflowInstanceAction::Pause,
            15,
        )
        .unwrap();
    assert!(matches!(
        controller
            .status(account, definition, identity.instance_id, 15)
            .unwrap(),
        WorkflowStatus::WaitingForPause
    ));
    controller
        .delete(
            account,
            definition,
            identity.instance_id,
            WorkflowOperationId::generate(),
            16,
        )
        .unwrap();
    assert_eq!(
        controller
            .status(account, definition, identity.instance_id, 16)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );

    for (status, expected) in [
        (WorkflowStatus::Queued, "Queued"),
        (WorkflowStatus::Running, "Running"),
        (WorkflowStatus::Waiting, "Waiting"),
        (WorkflowStatus::WaitingForPause, "WaitingForPause"),
        (WorkflowStatus::Paused, "Paused"),
        (WorkflowStatus::Terminated, "Terminated"),
        (
            WorkflowStatus::Complete {
                output_base64: NULL_VALUE.into(),
            },
            "Complete",
        ),
        (
            WorkflowStatus::Errored {
                error: WorkflowFailure::default(),
            },
            "Errored",
        ),
    ] {
        assert_eq!(format!("{status:?}"), expected);
        serde_json::to_value(status).unwrap();
    }
}
