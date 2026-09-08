use super::*;

#[test]
fn durable_purge_receipts_cannot_be_erased_by_corrupt_scheduler_recovery() {
    for finalized in [false, true] {
        let (_temp, storage, scheduler, definition) = durable_fixture();
        let config = WorkflowsConfig::default();
        let account = storage.identity().default_account_id;
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        let identity = create(&controller, account, definition, 10);
        controller
            .modify(
                account,
                definition,
                identity.instance_id,
                WorkflowInstanceAction::Terminate,
                11,
            )
            .unwrap();
        let repo = WorkflowRepository::new(storage.db());
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Purge,
                &config,
                3600011,
            )
            .unwrap();
        let WorkflowOperationResult::Applied(proof) = scheduler
            .apply_workflow_operation(&operation, 3600011, &config)
            .unwrap()
        else {
            panic!("purge");
        };
        if finalized {
            repo.complete_instance_operation(&proof, 3600012).unwrap();
            assert!(repo.reservation(identity.instance_id).unwrap().is_none());
            assert!(repo.instance_operations(None, 10).unwrap().is_empty());
            assert!(!repo.instance_referrers_intact(&identity).unwrap());
        }
        assert_eq!(scheduler.workflow_gc_receipts(None, 10).unwrap().len(), 1);
        drop(scheduler);
        let path = storage.data_dir().scheduler_db_path();
        std::fs::write(&path, b"corrupt durable workflow receipt").unwrap();
        assert_eq!(
            storage
                .data_dir()
                .recover_corrupt_scheduler_db("scheduler-corrupt-durable", 5000, 3600013)
                .unwrap_err()
                .code(),
            ErrorCode::SchedulerUnavailable
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"corrupt durable workflow receipt"
        );
        assert!(
            !storage
                .data_dir()
                .root()
                .join("diagnostics/scheduler-recovery/scheduler-corrupt-durable")
                .exists()
        );
    }
}
