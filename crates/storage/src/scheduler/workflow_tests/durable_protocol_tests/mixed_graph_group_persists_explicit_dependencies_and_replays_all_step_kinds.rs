use super::*;

#[test]
fn mixed_graph_group_persists_explicit_dependencies_and_replays_all_step_kinds() {
    let (_temp, store, identity) = setup();
    let limits = WorkflowsConfig::default();
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let first = do_step(0, json!({"timeout":100}));
    let first_attempt = claim(&store, &run.fence, &first, 1, &limits);
    store
        .settle_workflow_step(
            &run.fence,
            &first_attempt,
            WorkflowStepOutcome::Success(ONE_VALUE),
            2,
            &limits,
        )
        .unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "ready",
            TRUE_VALUE,
            2,
            &limits,
        )
        .unwrap();
    let mut sleep = descriptor(1, WorkflowStepKind::Sleep, json!({"duration":0}));
    let mut event = descriptor(
        2,
        WorkflowStepKind::WaitEvent,
        json!({"type":"ready","timeout":100}),
    );
    let mut action = do_step(3, json!({"timeout":100}));
    sleep.name_count = 1;
    event.name_count = 1;
    action.name_count = 2;
    for item in [&mut sleep, &mut event, &mut action] {
        item.dependencies = vec![0];
        item.batch_first_ordinal = 1;
        item.batch_size = 3;
    }
    let grants = store
        .claim_workflow_batch(&run.fence, &[sleep, event, action], 300_000, 3, &limits)
        .unwrap();
    assert!(matches!(grants[0], WorkflowStepGrant::Complete { .. }));
    assert!(matches!(grants[1], WorkflowStepGrant::Complete { .. }));
    let last = attempt(3, grants.into_iter().nth(2).unwrap());
    assert!(matches!(
        store.workflow_step_result(&run.fence, 2, 3).unwrap(),
        WorkflowStepResult::Event {
            ref event_type,
            ref payload_base64,
            timestamp_ms: 2,
        } if event_type == "ready" && payload_base64 == TRUE_VALUE
    ));
    store
        .settle_workflow_step(
            &run.fence,
            &last,
            WorkflowStepOutcome::Success(TWO_VALUE),
            4,
            &limits,
        )
        .unwrap();
    store
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: ARRAY_VALUE.into(),
                final_ordinal: 4,
            },
            5,
            &limits,
        )
        .unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
}
