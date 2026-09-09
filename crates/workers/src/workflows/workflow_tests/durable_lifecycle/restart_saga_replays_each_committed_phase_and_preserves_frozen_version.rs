use super::*;

#[test]
fn restart_saga_replays_each_committed_phase_and_preserves_frozen_version() {
    for phase in 0..=2 {
        let (_temp, storage, mut scheduler, definition) = durable_fixture();
        let account = storage.identity().default_account_id;
        let config = WorkflowsConfig::default();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        let identity = create(&controller, account, definition, 10);
        let old = controller
            .claim(11, &mut Default::default())
            .unwrap()
            .unwrap();
        let old_grant = grant(&scheduler, &old, 11, &config);
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
                12,
            )
            .unwrap();
        let repo = WorkflowRepository::new(storage.db());
        let newer = repo
            .stage_version(
                account,
                definition,
                identity.target.worker_version_id,
                "Flow",
                13,
            )
            .unwrap();
        repo.finish_version(account, newer.target.workflow_version_id, true, 13)
            .unwrap();
        let operation = repo
            .prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Restart,
                &config,
                14,
            )
            .unwrap();
        assert_eq!(
            repo.prepare_instance_operation(
                &identity,
                operation.id(),
                WorkflowOperationKind::Restart,
                &config,
                14
            )
            .unwrap(),
            operation
        );
        assert_eq!(
            repo.prepare_instance_operation(
                &identity,
                WorkflowOperationId::generate(),
                WorkflowOperationKind::Restart,
                &config,
                14
            )
            .unwrap_err()
            .code(),
            ErrorCode::WorkflowInstanceBusy
        );
        assert_eq!(
            controller
                .status(account, definition, identity.instance_id, 14)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceBusy
        );
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
                    14
                )
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceBusy
        );
        if phase >= 1 {
            let WorkflowOperationResult::Applied(proof) = scheduler
                .apply_workflow_operation(&operation, 15, &config)
                .unwrap()
            else {
                panic!("restart")
            };
            assert!(
                controller
                    .claim(15, &mut Default::default())
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                scheduler
                    .settle_workflow_step(
                        &old.fence,
                        &old_grant,
                        WorkflowStepOutcome::Success(TRUE_VALUE),
                        15,
                        &config
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowRunStale
            );
            if phase == 2 {
                repo.complete_instance_operation(&proof, 16).unwrap();
            }
        }
        assert_eq!(inspect(&storage).pending_restarts, u64::from(phase < 2));
        let path = storage.data_dir().ensure_scheduler_db().unwrap();
        drop(scheduler);
        scheduler = SchedulerStore::open(&path, 5000, 17).unwrap();
        let controller = WorkflowController::new(&storage, &scheduler, &config);
        controller
            .reconcile(&mut WorkflowReconcileCursor::default(), 32, 17)
            .unwrap();
        let next = scheduler
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(next.identity.instance_generation, 2);
        assert_eq!(next.identity.target, identity.target);
        assert_ne!(
            next.identity.target.workflow_version_id,
            newer.target.workflow_version_id
        );
        assert_eq!(next.identity.created_at_ms, 10);
        assert_eq!(next.input_json, OBJECT_VALUE);
        assert_eq!(next.durable.registered_step_count, 0);
        assert_eq!(next.durable.event_count, 0);
        assert_eq!(next.durable.next_event_seq, 1);
        assert_eq!(
            repo.reservation(identity.instance_id)
                .unwrap()
                .unwrap()
                .identity,
            next.identity
        );
        assert!(
            repo.instance_operation(identity.instance_id)
                .unwrap()
                .is_none()
        );
        let run = controller
            .claim(18, &mut Default::default())
            .unwrap()
            .unwrap();
        let step = grant(&scheduler, &run, 18, &config);
        scheduler
            .settle_workflow_step(
                &run.fence,
                &step,
                WorkflowStepOutcome::Success(EIGHT_VALUE),
                19,
                &config,
            )
            .unwrap();
        assert!(matches!(
            scheduler
                .apply_workflow_operation(&operation, 20, &config)
                .unwrap(),
            WorkflowOperationResult::Applied(_)
        ));
        assert_eq!(
            scheduler
                .workflow_instance(identity.instance_id)
                .unwrap()
                .unwrap()
                .completed_step_count,
            1
        );
        controller
            .restart(
                account,
                definition,
                identity.instance_id,
                WorkflowOperationId::generate(),
                None,
                21,
            )
            .unwrap();
        assert_eq!(
            scheduler
                .workflow_instance(identity.instance_id)
                .unwrap()
                .unwrap()
                .identity
                .instance_generation,
            3
        );
        assert_eq!(
            scheduler
                .apply_workflow_operation(&operation, 22, &config)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowRunStale
        );
        scheduler
            .verify_workflow_history(identity.instance_id)
            .unwrap();
        repo.verify_catalog().unwrap();
    }
}
