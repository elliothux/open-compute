use super::*;

#[test]
fn current_production_do_sleep_event_resume_and_terminal_are_durable() {
    let (temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let action = do_step(0, json!({"timeout":100}));
    let grant = claim(&store, &run.fence, &action, 1, &limits);
    assert!(matches!(
        store
            .settle_workflow_step(
                &run.fence,
                &grant,
                WorkflowStepOutcome::Success(SEVEN_VALUE),
                2,
                &limits
            )
            .unwrap(),
        WorkflowStepResult::Complete { .. }
    ));
    let mut sleep = descriptor(1, WorkflowStepKind::Sleep, json!({"duration":10}));
    sleep.name_count = 1;
    assert!(matches!(
        wait_result(&store, &run.fence, &sleep, 3, &limits),
        WorkflowStepResult::Suspended
    ));
    assert_eq!(
        store.yield_workflow(&run.fence, 3).unwrap(),
        WorkflowState::Waiting
    );
    assert_eq!(store.maintain_workflow_due(12, &limits, 10).unwrap(), 0);
    assert_eq!(store.maintain_workflow_due(13, &limits, 10).unwrap(), 1);
    drop(store);
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 13).unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
    let run = store
        .claim_workflow(&identity, 13, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_batch(
                &run.fence,
                std::slice::from_ref(&action),
                300000,
                13,
                &limits
            )
            .unwrap()[0],
        WorkflowStepGrant::Complete { .. }
    ));
    assert!(matches!(
        wait_result(&store, &run.fence, &sleep, 13, &limits),
        WorkflowStepResult::Complete {
            output_base64: None
        }
    ));
    let mut wait = descriptor(
        2,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approval","timeout":100}),
    );
    wait.name_count = 1;
    assert!(matches!(
        wait_result(&store, &run.fence, &wait, 13, &limits),
        WorkflowStepResult::Suspended
    ));
    store.yield_workflow(&run.fence, 13).unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "approval",
            DECISION_VALUE,
            14,
            &limits,
        )
        .unwrap();
    let run = store
        .claim_workflow(&identity, 14, &limits)
        .unwrap()
        .unwrap();
    let result = wait_result(&store, &run.fence, &wait, 14, &limits);
    let WorkflowStepResult::Event {
        event_type,
        payload_base64,
        timestamp_ms,
    } = result
    else {
        panic!("event result")
    };
    assert_eq!(
        (event_type.as_str(), payload_base64.as_str(), timestamp_ms),
        ("approval", DECISION_VALUE, 14)
    );
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: TRUE_VALUE.into(),
                    final_ordinal: 3
                },
                15,
                &limits
            )
            .unwrap(),
        WorkflowState::Complete
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
    let metadata = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap()
        .durable;
    assert_eq!(
        (
            metadata.registered_step_count,
            metadata.settled_step_count,
            metadata.event_count
        ),
        (3, 3, 0)
    );
    assert_eq!(metadata.expires_at_ms, Some(3_600_015));
    assert_eq!(
        store
            .send_workflow_event(
                &identity,
                WorkflowOperationId::generate(),
                "approval",
                NULL_VALUE,
                16,
                &limits,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceStateConflict
    );
}
