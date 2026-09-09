use super::*;

#[test]
fn restart_from_reconciles_response_loss_and_replays_only_the_exact_completed_prefix() {
    let (_temp, storage, scheduler, definition) = durable_fixture();
    let account = storage.identity().default_account_id;
    let config = WorkflowsConfig::default();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    let identity = create(&controller, account, definition, 10);
    let old_run = controller
        .claim(11, &mut Default::default())
        .unwrap()
        .unwrap();
    let descriptor =
        |ordinal, name: &str, name_count, batch_first_ordinal, batch_size, dependencies| {
            WorkflowStepDeclaration {
                ordinal,
                name: name.into(),
                name_count,
                kind: WorkflowStepKind::Do,
                config: serde_json::json!({"timeout":1000,"retries":{"limit":0,"delay":0}}),
                rollback_config: None,
                rollback_step: false,
                batch_first_ordinal,
                batch_size,
                dependencies,
            }
            .resolve()
            .unwrap()
        };
    let prefix = descriptor(0, "prefix", 1, 0, 1, vec![]);
    let prefix_attempt = attempt(
        0,
        scheduler
            .claim_workflow_batch(
                &old_run.fence,
                std::slice::from_ref(&prefix),
                300_000,
                12,
                &config,
            )
            .unwrap()
            .remove(0),
    );
    scheduler
        .settle_workflow_step(
            &old_run.fence,
            &prefix_attempt,
            WorkflowStepOutcome::Success(TRUE_VALUE),
            13,
            &config,
        )
        .unwrap();
    let target = descriptor(1, "target", 1, 1, 2, vec![0]);
    let sibling = descriptor(2, "sibling", 1, 1, 2, vec![0]);
    let mut grants = scheduler
        .claim_workflow_batch(
            &old_run.fence,
            &[target.clone(), sibling.clone()],
            300_000,
            14,
            &config,
        )
        .unwrap()
        .into_iter();
    let target_attempt = attempt(1, grants.next().unwrap());
    let sibling_attempt = attempt(2, grants.next().unwrap());
    scheduler
        .settle_workflow_step(
            &old_run.fence,
            &target_attempt,
            WorkflowStepOutcome::Success(EIGHT_VALUE),
            15,
            &config,
        )
        .unwrap();
    scheduler
        .settle_workflow_step(
            &old_run.fence,
            &sibling_attempt,
            WorkflowStepOutcome::Success(EMPTY_OBJECT_VALUE),
            16,
            &config,
        )
        .unwrap();
    let later = descriptor(3, "target", 2, 3, 1, vec![1, 2]);
    let later_attempt = attempt(
        3,
        scheduler
            .claim_workflow_batch(
                &old_run.fence,
                std::slice::from_ref(&later),
                300_000,
                17,
                &config,
            )
            .unwrap()
            .remove(0),
    );
    scheduler
        .settle_workflow_step(
            &old_run.fence,
            &later_attempt,
            WorkflowStepOutcome::Success(NULL_VALUE),
            18,
            &config,
        )
        .unwrap();

    let selector = WorkflowRestartSelector {
        name: "target".into(),
        count: 1,
        step_type: Some(WorkflowRestartStepType::Do),
    };
    let operation_id = WorkflowOperationId::generate();
    let repository = WorkflowRepository::new(storage.db());
    let operation = repository
        .prepare_restart_operation(&identity, operation_id, Some(selector.clone()), &config, 19)
        .unwrap();
    assert!(matches!(
        scheduler
            .apply_workflow_operation(&operation, 20, &config)
            .unwrap(),
        WorkflowOperationResult::Applied(_)
    ));
    let path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(scheduler);
    let scheduler = SchedulerStore::open(&path, 5000, 21).unwrap();
    let controller = WorkflowController::new(&storage, &scheduler, &config);
    controller
        .reconcile(&mut WorkflowReconcileCursor::default(), 32, 21)
        .unwrap();
    let restarted = scheduler
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(restarted.identity.instance_generation, 2);
    assert_eq!(restarted.durable.registered_step_count, 3);
    assert_eq!(restarted.completed_step_count, 1);
    assert!(
        repository
            .instance_operation(identity.instance_id)
            .unwrap()
            .is_none()
    );

    controller
        .restart(
            account,
            definition,
            identity.instance_id,
            operation_id,
            Some(selector.clone()),
            22,
        )
        .unwrap();
    assert_eq!(
        scheduler
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .identity
            .instance_generation,
        2
    );
    assert_eq!(
        controller
            .restart(
                account,
                definition,
                identity.instance_id,
                operation_id,
                Some(WorkflowRestartSelector {
                    count: 2,
                    ..selector.clone()
                }),
                22,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    assert_eq!(
        controller
            .restart(
                account,
                definition,
                identity.instance_id,
                WorkflowOperationId::generate(),
                Some(WorkflowRestartSelector {
                    name: "missing".into(),
                    ..selector
                }),
                22,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );

    let new_run = controller
        .claim(23, &mut Default::default())
        .unwrap()
        .unwrap();
    assert!(matches!(
        scheduler
            .claim_workflow_batch(
                &new_run.fence,
                std::slice::from_ref(&prefix),
                300_000,
                23,
                &config,
            )
            .unwrap()[0],
        WorkflowStepGrant::Complete { .. }
    ));
    assert!(matches!(
        scheduler
            .workflow_step_result(&new_run.fence, 0, 23)
            .unwrap(),
        WorkflowStepResult::Complete { output_base64: Some(ref output) } if output == TRUE_VALUE
    ));
    let grants = scheduler
        .claim_workflow_batch(&new_run.fence, &[target, sibling], 300_000, 23, &config)
        .unwrap();
    assert!(
        grants
            .into_iter()
            .all(|grant| matches!(grant, WorkflowStepGrant::Run { attempt: 1, .. }))
    );
    scheduler
        .verify_workflow_history(identity.instance_id)
        .unwrap();
    repository.verify_catalog().unwrap();
}
