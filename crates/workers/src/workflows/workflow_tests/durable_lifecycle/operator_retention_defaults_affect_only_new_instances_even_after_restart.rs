use super::*;

#[test]
fn operator_retention_defaults_affect_only_new_instances_even_after_restart() {
    let (_temp, storage, scheduler, definition) = durable_fixture();
    let account = storage.identity().default_account_id;
    let mut limits = WorkflowsConfig {
        default_retention: WorkflowRetention {
            success_retention_ms: 3600000,
            error_retention_ms: 7200000,
        },
        ..Default::default()
    };
    let old = WorkflowController::new(&storage, &scheduler, &limits)
        .create(
            account,
            definition,
            WorkflowOperationId::generate(),
            None,
            WorkflowCreateInput {
                payload_base64: NULL_VALUE,
                retention: None,
                schedule: None,
            },
            10,
        )
        .unwrap();
    let frozen = limits.default_retention.clone();
    limits.default_retention = WorkflowRetention {
        success_retention_ms: 10800000,
        error_retention_ms: 14400000,
    };
    let path = storage.data_dir().scheduler_db_path();
    drop(scheduler);
    let scheduler = SchedulerStore::open(&path, 5000, 11).unwrap();
    let controller = WorkflowController::new(&storage, &scheduler, &limits);
    let new = controller
        .create(
            account,
            definition,
            WorkflowOperationId::generate(),
            None,
            WorkflowCreateInput {
                payload_base64: NULL_VALUE,
                retention: None,
                schedule: None,
            },
            11,
        )
        .unwrap();
    assert_eq!(
        scheduler
            .workflow_instance(new.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .retention,
        limits.default_retention
    );
    controller
        .modify(
            account,
            definition,
            old.instance_id,
            WorkflowInstanceAction::Terminate,
            12,
        )
        .unwrap();
    let retained = scheduler
        .workflow_instance(old.instance_id)
        .unwrap()
        .unwrap()
        .durable;
    assert_eq!(retained.retention, frozen);
    assert_eq!(retained.expires_at_ms, Some(7200012));
    controller
        .restart(
            account,
            definition,
            old.instance_id,
            WorkflowOperationId::generate(),
            None,
            13,
        )
        .unwrap();
    assert_eq!(
        scheduler
            .workflow_instance(old.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .retention,
        frozen
    );
}
