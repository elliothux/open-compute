//! Abort after writes within production transactions; no half token, frontier, or result survives.

use super::*;

#[test]
fn workflow_scheduler_claim_step_terminal_and_recovery_are_transactional() {
    let (_temp, store, identity, limits) = setup();
    let inject = |sql: &str| store.lock().unwrap().execute_batch(sql).unwrap();
    inject(
        "CREATE TEMP TRIGGER reject_workflow_run AFTER UPDATE ON workflow_instances
        WHEN NEW.state='running' BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;",
    );
    assert!(store.claim_workflow(&identity, 0, &limits).is_err());
    let unchanged = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.state, WorkflowState::Queued);
    assert!(unchanged.run_token.is_none());
    inject("DROP TRIGGER reject_workflow_run;");
    let run = store
        .claim_workflow(&identity, 0, &limits)
        .unwrap()
        .unwrap();
    let descriptor = step(0, "atomic", 1);
    let step_token = grant(&store, &run.fence, &descriptor, 1, &limits);
    let initial_bytes = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap()
        .state_bytes;
    for state in ["complete", "failed"] {
        inject(&format!(
            "CREATE TEMP TRIGGER reject_workflow_step AFTER UPDATE ON workflow_steps
            WHEN NEW.state='{state}' BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;"
        ));
        let result = if state == "complete" {
            store.complete_workflow_step(&run.fence, 0, &step_token, "42", 2, &limits)
        } else {
            store.fail_workflow_step(
                &run.fence,
                0,
                &step_token,
                ErrorCode::WorkflowExecutionFailed,
                2,
                &limits,
            )
        };
        assert!(result.is_err());
        let record = store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.state_bytes, initial_bytes);
        assert_eq!(record.completed_step_count, 0);
        assert_eq!(
            store
                .workflow_steps(identity.instance_id, None, 10)
                .unwrap()[0]
                .state,
            "running"
        );
        assert_eq!(
            grant(&store, &run.fence, &descriptor, 3, &limits),
            step_token
        );
        store.verify_workflow_history(identity.instance_id).unwrap();
        inject("DROP TRIGGER reject_workflow_step;");
    }
    inject(
        "CREATE TEMP TRIGGER reject_workflow_recovery AFTER UPDATE ON workflow_instances
        WHEN NEW.state='queued' BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;",
    );
    assert!(store.recover_workflows(200, &limits, 10).is_err());
    let record = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, WorkflowState::Running);
    assert_eq!(record.run_token, Some(run.fence.run_token.clone()));
    assert_eq!(
        store
            .workflow_steps(identity.instance_id, None, 10)
            .unwrap()[0]
            .state,
        "running"
    );
    inject("DROP TRIGGER reject_workflow_recovery;");
    assert_eq!(store.recover_workflows(200, &limits, 10).unwrap(), 1);
    let replay = store
        .claim_workflow(&identity, 210, &limits)
        .unwrap()
        .unwrap();
    let next_token = grant(&store, &replay.fence, &descriptor, 211, &limits);
    assert_ne!(next_token, step_token);
    store
        .complete_workflow_step(&replay.fence, 0, &next_token, "42", 212, &limits)
        .unwrap();
    let completed_bytes = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap()
        .state_bytes;
    inject("CREATE TEMP TRIGGER reject_workflow_terminal AFTER UPDATE ON workflow_instances
        WHEN NEW.state IN ('complete','errored') BEGIN SELECT RAISE(ABORT,'test transaction fault'); END;");
    let completion = WorkflowCompletion::Complete {
        output_json: "42".into(),
        final_ordinal: 1,
    };
    assert!(
        store
            .finish_workflow(&replay.fence, &completion, 213, &limits)
            .is_err()
    );
    let record = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, WorkflowState::Running);
    assert_eq!(record.run_token, Some(replay.fence.run_token.clone()));
    assert_eq!(record.state_bytes, completed_bytes);
    assert!(record.output_json.is_none());
    inject("DROP TRIGGER reject_workflow_terminal;");
    assert_eq!(
        store
            .finish_workflow(&replay.fence, &completion, 214, &limits)
            .unwrap(),
        WorkflowState::Complete
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
}
