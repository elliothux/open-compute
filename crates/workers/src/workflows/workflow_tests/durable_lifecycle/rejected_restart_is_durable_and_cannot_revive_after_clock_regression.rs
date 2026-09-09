use super::*;

#[test]
fn rejected_restart_is_durable_and_cannot_revive_after_clock_regression() {
    let (_temp, storage, scheduler, definition) = durable_fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    let identity = create(&controller, account, definition, 10);
    controller
        .modify(
            account,
            definition,
            identity.instance_id,
            WorkflowInstanceAction::Terminate,
            20,
        )
        .unwrap();
    let repo = WorkflowRepository::new(storage.db());
    let expiry = 3600020;
    let operation = repo
        .prepare_instance_operation(
            &identity,
            WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &config,
            expiry - 1,
        )
        .unwrap();
    let WorkflowOperationResult::Rejected(proof) = scheduler
        .apply_workflow_operation(&operation, expiry, &config)
        .unwrap()
    else {
        panic!("expiry rejection")
    };
    assert_eq!(proof.code(), ErrorCode::WorkflowInstanceNotFound);
    assert_eq!(
        repo.reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Restarting
    );
    let path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(scheduler);
    let scheduler = SchedulerStore::open(&path, 5000, expiry).unwrap();
    let WorkflowOperationResult::Rejected(proof) = scheduler
        .apply_workflow_operation(&operation, expiry - 100, &config)
        .unwrap()
    else {
        panic!("rejection must survive clock regression")
    };
    repo.cancel_instance_operation(&proof, expiry).unwrap();
    repo.cancel_instance_operation(&proof, expiry).unwrap();
    assert_eq!(
        repo.reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Retained
    );
    let next = repo
        .prepare_instance_operation(
            &identity,
            WorkflowOperationId::generate(),
            WorkflowOperationKind::Restart,
            &config,
            expiry - 90,
        )
        .unwrap();
    assert_eq!(next.sequence(), operation.sequence() + 1);
    let WorkflowOperationResult::Applied(proof) = scheduler
        .apply_workflow_operation(&next, expiry - 90, &config)
        .unwrap()
    else {
        panic!("new request")
    };
    repo.complete_instance_operation(&proof, expiry - 90)
        .unwrap();
    assert_eq!(
        scheduler
            .apply_workflow_operation(&operation, expiry - 200, &config)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    repo.verify_catalog().unwrap();
    scheduler
        .verify_workflow_history(identity.instance_id)
        .unwrap();
}
