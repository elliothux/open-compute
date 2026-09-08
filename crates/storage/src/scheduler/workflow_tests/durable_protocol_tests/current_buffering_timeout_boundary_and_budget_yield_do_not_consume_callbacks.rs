use super::*;

#[test]
fn current_buffering_timeout_boundary_and_budget_yield_do_not_consume_callbacks() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "ok",
            SEVEN_VALUE,
            0,
            &limits,
        )
        .unwrap();
    let run = store
        .claim_workflow(&identity, 1, &limits)
        .unwrap()
        .unwrap();
    let wait = descriptor(
        0,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ok","timeout":0}),
    );
    assert!(matches!(
        wait_result(&store, &run.fence, &wait, 1, &limits),
        WorkflowStepResult::Event { .. }
    ));
    let mut late = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ok","timeout":1}),
    );
    late.name_count = 2;
    wait_result(&store, &run.fence, &late, 1, &limits);
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "ok",
            EIGHT_VALUE,
            2,
            &limits,
        )
        .unwrap();
    assert_eq!(
        store.yield_workflow(&run.fence, 2).unwrap(),
        WorkflowState::Queued
    );
    let run = store
        .claim_workflow(&identity, 2, &limits)
        .unwrap()
        .unwrap();
    assert!(matches!(
        wait_result(&store, &run.fence, &late, 2, &limits),
        WorkflowStepResult::Failed { code } if code == "WORKFLOW_EVENT_TIMEOUT"
    ));
    let mut action = do_step(2, json!({"timeout":100}));
    action.name_count = 1;
    assert!(matches!(
        store
            .claim_workflow_batch(&run.fence, std::slice::from_ref(&action), 100, 2, &limits)
            .unwrap()[0],
        WorkflowStepGrant::Suspended
    ));
    store.yield_workflow(&run.fence, 2).unwrap();
    let run = store
        .claim_workflow(&identity, 3, &limits)
        .unwrap()
        .unwrap();
    assert_eq!(claim(&store, &run.fence, &action, 3, &limits).attempt, 1);
    store.verify_workflow_history(identity.instance_id).unwrap();
}
