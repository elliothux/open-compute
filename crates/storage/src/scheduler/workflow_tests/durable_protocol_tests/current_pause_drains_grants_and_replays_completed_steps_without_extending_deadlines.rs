use super::*;

#[test]
fn current_pause_drains_grants_and_replays_completed_steps_without_extending_deadlines() {
    let (temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 0, &limits)
        .unwrap();
    assert!(
        store
            .claim_workflow(&identity, 0, &limits)
            .unwrap()
            .is_none()
    );
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 0, &limits)
        .unwrap();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, 1, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 1, &limits)
        .unwrap()
        .unwrap();
    let first = do_step(0, json!({"timeout":100}));
    let grant = claim(&store, &run.fence, &first, 1, &limits);
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 2, &limits)
        .unwrap();
    assert!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .durable
            .pause_requested
    );
    assert_eq!(
        store
            .modify_workflow(&identity, WorkflowInstanceAction::Resume, 3, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
    assert_eq!(
        store.yield_workflow(&run.fence, 3).unwrap_err().code(),
        ErrorCode::WorkflowInstanceBusy
    );
    let next = do_step(1, json!({"timeout":100}));
    assert!(matches!(
        store
            .claim_workflow_batch(&run.fence, &[next], 300000, 3, &limits)
            .unwrap()[0],
        WorkflowStepGrant::Suspended
    ));
    store
        .settle_workflow_step(
            &run.fence,
            &grant,
            WorkflowStepOutcome::Success(ONE_VALUE),
            4,
            &limits,
        )
        .unwrap();
    // Pause committed before terminal wins even when it arrived after the last grant.
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: ONE_VALUE.into(),
                    final_ordinal: 1
                },
                5,
                &limits
            )
            .unwrap(),
        WorkflowState::Paused
    );
    assert!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .output_json
            .is_none()
    );
    drop(store);
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 6).unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, 6, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 6, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_batch(&run.fence, &[first], 300000, 6, &limits)
            .unwrap()[0],
        WorkflowStepGrant::Complete { .. }
    ));
    let mut wait = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approval","timeout":10}),
    );
    wait.name_count = 1;
    wait_result(&store, &run.fence, &wait, 6, &limits);
    store.yield_workflow(&run.fence, 6).unwrap();
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Pause, 7, &limits)
        .unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "approval",
            TRUE_VALUE,
            15,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Paused
    );
    store
        .modify_workflow(&identity, WorkflowInstanceAction::Resume, 20, &limits)
        .unwrap();
    let run = store
        .claim_workflow(&identity, 20, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        wait_result(&store, &run.fence, &wait, 20, &limits),
        WorkflowStepResult::Event { .. }
    ));
    store
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: TRUE_VALUE.into(),
                final_ordinal: 2,
            },
            21,
            &limits,
        )
        .unwrap();
    for action in [
        WorkflowInstanceAction::Pause,
        WorkflowInstanceAction::Resume,
        WorkflowInstanceAction::Terminate,
    ] {
        assert_eq!(
            store
                .modify_workflow(&identity, action, 22, &limits)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInstanceStateConflict
        );
    }
    store.verify_workflow_history(identity.instance_id).unwrap();
}
