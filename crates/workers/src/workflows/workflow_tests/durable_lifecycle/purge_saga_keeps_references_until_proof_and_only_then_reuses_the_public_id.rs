use super::*;

#[test]
fn purge_saga_keeps_references_until_proof_and_only_then_reuses_the_public_id() {
    for phase in 0..=3 {
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
        let expiry = 3600020;
        assert!(matches!(
            controller
                .status(account, definition, identity.instance_id, expiry - 1)
                .unwrap(),
            WorkflowStatus::Terminated
        ));
        assert_eq!(
            controller
                .status(account, definition, identity.instance_id, expiry)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceNotFound
        );
        assert_eq!(
            controller
                .create(
                    account,
                    definition,
                    WorkflowOperationId::generate(),
                    Some("reusable"),
                    WorkflowCreateInput {
                        payload_base64: EMPTY_OBJECT_VALUE,
                        retention: None,
                        schedule: None,
                    },
                    expiry
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceCleanupPending
        );
        let repo = WorkflowRepository::new(storage.db());
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Purge,
                &config,
                expiry,
            )
            .unwrap();
        if phase >= 1 {
            let WorkflowOperationResult::Applied(proof) = scheduler
                .apply_workflow_operation(&operation, expiry, &config)
                .unwrap()
            else {
                panic!("purge")
            };
            assert!(
                scheduler
                    .workflow_instance(identity.instance_id)
                    .unwrap()
                    .is_none()
            );
            assert!(repo.instance_referrers_intact(&identity).unwrap());
            let receipt = scheduler.workflow_gc_receipts(None, 10).unwrap().remove(0);
            assert_eq!(
                repo.acknowledge_workflow_gc(&receipt).unwrap_err().code(),
                ErrorCode::WorkflowInstanceBusy
            );
            if phase >= 2 {
                repo.complete_instance_operation(&proof, expiry).unwrap();
                if phase == 3 {
                    scheduler
                        .sweep_workflow_gc(&repo.acknowledge_workflow_gc(&receipt).unwrap())
                        .unwrap();
                }
            }
        }
        let diagnostics = inspect(&storage);
        assert_eq!(diagnostics.pending_purges, u64::from(phase < 2));
        assert_eq!(diagnostics.pending_receipt_sweeps, u64::from(phase == 2));
        let path = storage.data_dir().ensure_scheduler_db().unwrap();
        drop(scheduler);
        let scheduler = SchedulerStore::open(&path, 5000, expiry + 1).unwrap();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 32, expiry + 1)
            .unwrap();
        assert!(repo.reservation(identity.instance_id).unwrap().is_none());
        assert!(!repo.instance_referrers_intact(&identity).unwrap());
        assert!(scheduler.workflow_gc_receipts(None, 10).unwrap().is_empty());
        let next = create(&controller, account, definition, expiry + 2);
        assert_ne!(next.instance_id, identity.instance_id);
        assert_ne!(next.creation_nonce, identity.creation_nonce);
        assert_eq!(
            controller
                .send_event(
                    account,
                    definition,
                    identity.instance_id,
                    WorkflowEventInput {
                        operation_id: WorkflowOperationId::generate(),
                        event_type: "approval",
                        payload_base64: TRUE_VALUE,
                    },
                    expiry + 3
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceNotFound
        );
        controller
            .send_event(
                account,
                definition,
                next.instance_id,
                WorkflowEventInput {
                    operation_id: WorkflowOperationId::generate(),
                    event_type: "approval",
                    payload_base64: TRUE_VALUE,
                },
                expiry + 3,
            )
            .unwrap();
        repo.verify_catalog().unwrap();
        scheduler.verify_workflow_history(next.instance_id).unwrap();
    }
}
