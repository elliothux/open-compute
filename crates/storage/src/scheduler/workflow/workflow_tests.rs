use super::*;
use open_compute_core::{AccountId, DeploymentId, WorkerId, WorkflowId, WorkflowVersionId};

#[path = "atomicity_tests.rs"]
mod atomicity_tests;
#[path = "v2_schema_tests.rs"]
mod v2_schema_tests;

fn setup() -> (
    tempfile::TempDir,
    SchedulerStore,
    WorkflowInstanceIdentity,
    WorkflowsConfig,
) {
    let tmp = tempfile::tempdir().unwrap();
    let store = SchedulerStore::open(&tmp.path().join("scheduler.sqlite"), 5000, 0).unwrap();
    let mut target = WorkflowTarget {
        account_id: AccountId::generate(),
        definition_id: WorkflowId::generate(),
        definition_name: "flow".into(),
        version_id: WorkflowVersionId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        worker_code_sha256: [1; 32],
        class_name: "Workflow".into(),
        loader_schema_version: 1,
        capability_version: 1,
        descriptor_sha256: [0; 32],
    };
    target.descriptor_sha256 = crate::workflows::helpers::version_digest(&target).unwrap();
    let identity = WorkflowInstanceIdentity {
        instance_id: WorkflowInstanceId::generate(),
        external_instance_id: "order-one".into(),
        target,
        instance_generation: 1,
        creation_nonce: token().unwrap(),
        created_at_ms: 0,
    };
    let limits = WorkflowsConfig {
        lease_ms: 100,
        heartbeat_ms: 20,
        dispatch_timeout_ms: 1000,
        recovery_backoff_ms: 10,
        ..WorkflowsConfig::default()
    };
    store
        .insert_workflow(&identity, r#"{"z":1,"a":2}"#, None, &limits)
        .unwrap();
    (tmp, store, identity, limits)
}

fn step(ordinal: u32, name: &str, count: u32) -> WorkflowStepIdentity {
    WorkflowStepIdentity {
        ordinal,
        name: name.into(),
        name_count: count,
        config_json: "null".into(),
    }
}

fn grant(
    store: &SchedulerStore,
    run: &WorkflowFence,
    descriptor: &WorkflowStepIdentity,
    now: i64,
    limits: &WorkflowsConfig,
) -> WorkflowToken {
    match store
        .claim_workflow_step(run, descriptor, now, limits)
        .unwrap()
    {
        WorkflowStepGrant::Run { step_token } => step_token,
        other => panic!("unexpected grant: {other:?}"),
    }
}

#[test]
fn workflow_commit_replay_restart_and_terminal_are_durable() {
    let (tmp, store, id, limits) = setup();
    store
        .insert_workflow(&id, r#"{"a":2,"z":1}"#, None, &limits)
        .unwrap();
    assert_eq!(
        store.due_workflows(0, 10, &mut Default::default()).unwrap(),
        vec![id.instance_id]
    );
    let run = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
    assert!(store.claim_workflow(&id, 0, &limits).unwrap().is_none());
    let descriptor = step(0, "fetch", 1);
    let step_token = grant(&store, &run.fence, &descriptor, 1, &limits);
    assert_eq!(
        grant(&store, &run.fence, &descriptor, 1, &limits),
        step_token
    );
    store
        .complete_workflow_step(&run.fence, 0, &step_token, r#"{"answer":42}"#, 2, &limits)
        .unwrap();
    store.verify_workflow_history(id.instance_id).unwrap();
    assert_eq!(
        store
            .complete_workflow_step(&run.fence, 0, &step_token, "null", 3, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepStale
    );
    drop(store);
    let store = SchedulerStore::open(&tmp.path().join("scheduler.sqlite"), 5000, 500).unwrap();
    assert_eq!(store.recover_workflows(500, &limits, 10).unwrap(), 1);
    assert!(store.claim_workflow(&id, 509, &limits).unwrap().is_none());
    let replay = store.claim_workflow(&id, 510, &limits).unwrap().unwrap();
    assert_ne!(run.fence.run_token, replay.fence.run_token);
    assert!(
        matches!(store.claim_workflow_step(&replay.fence,&descriptor,511,&limits).unwrap(),WorkflowStepGrant::Complete { output_json } if output_json==r#"{"answer":42}"#)
    );
    let second = grant(&store, &replay.fence, &step(1, "fetch", 2), 512, &limits);
    store
        .complete_workflow_step(&replay.fence, 1, &second, "2", 513, &limits)
        .unwrap();
    assert_eq!(
        store
            .finish_workflow(
                &replay.fence,
                &WorkflowCompletion::Complete {
                    output_json: "42".into(),
                    final_ordinal: 2
                },
                514,
                &limits
            )
            .unwrap(),
        WorkflowState::Complete
    );
    store.verify_workflow_history(id.instance_id).unwrap();
    assert_eq!(
        store
            .workflow_instance(id.instance_id)
            .unwrap()
            .unwrap()
            .output_json
            .as_deref(),
        Some("42")
    );
    assert_eq!(
        store
            .heartbeat_workflow(&replay.fence, 515, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert_eq!(store.recover_workflows(1000, &limits, 10).unwrap(), 0);
    assert!(store.claim_workflow(&id, 1000, &limits).unwrap().is_none());
    assert_eq!(
        store
            .workflow_steps(id.instance_id, None, 100)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(store.inspect_workflows(1000).unwrap().complete, 1);
    assert_eq!(store.workflow_workload_summary(1000).unwrap().ready, 0);
    let conn = store.lock().unwrap();
    assert!(
        conn.execute(
            "UPDATE workflow_instances SET output_json=X'30' WHERE id=?1",
            [id.instance_id.to_string()]
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "DELETE FROM workflow_steps WHERE instance_id=?1",
            [id.instance_id.to_string()]
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "DELETE FROM workflow_instances WHERE id=?1",
            [id.instance_id.to_string()]
        )
        .is_err()
    );
}

#[test]
fn workflow_expired_run_and_step_tokens_cannot_commit_or_revive() {
    let (_tmp, store, id, limits) = setup();
    let first = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
    let descriptor = step(0, "action", 1);
    let first_step = grant(&store, &first.fence, &descriptor, 0, &limits);
    assert_eq!(
        store
            .heartbeat_workflow(&first.fence, 100, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert_eq!(
        store
            .complete_workflow_step(&first.fence, 0, &first_step, "1", 100, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    assert_eq!(store.recover_workflows(100, &limits, 10).unwrap(), 1);
    let next = store.claim_workflow(&id, 110, &limits).unwrap().unwrap();
    let next_step = grant(&store, &next.fence, &descriptor, 110, &limits);
    assert_ne!(first_step, next_step);
    assert_eq!(
        store
            .complete_workflow_step(&next.fence, 0, &first_step, "1", 111, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStepStale
    );
    assert_eq!(
        store
            .fail_workflow_step(
                &first.fence,
                0,
                &first_step,
                ErrorCode::WorkflowExecutionFailed,
                111,
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    store.heartbeat_workflow(&next.fence, 200, &limits).unwrap();
    assert_eq!(store.recover_workflows(211, &limits, 10).unwrap(), 0);
    store
        .complete_workflow_step(&next.fence, 0, &next_step, "null", 299, &limits)
        .unwrap();
    store.verify_workflow_history(id.instance_id).unwrap();
}

#[test]
fn workflow_failed_steps_replay_and_cannot_be_caught_into_success() {
    let (_tmp, store, id, limits) = setup();
    let run = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
    let descriptor = step(0, "action", 1);
    let step_token = grant(&store, &run.fence, &descriptor, 1, &limits);
    store
        .fail_workflow_step(
            &run.fence,
            0,
            &step_token,
            ErrorCode::WorkflowExecutionFailed,
            2,
            &limits,
        )
        .unwrap();
    assert!(matches!(
        store
            .claim_workflow_step(&run.fence, &descriptor, 3, &limits)
            .unwrap(),
        WorkflowStepGrant::Failed { .. }
    ));
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: "true".into(),
                    final_ordinal: 1
                },
                4,
                &limits
            )
            .unwrap(),
        WorkflowState::Errored
    );
    let instance = store.workflow_instance(id.instance_id).unwrap().unwrap();
    assert_eq!(instance.error, Some(WorkflowFailure::default()));
    assert_eq!(
        instance.error_code.as_deref(),
        Some("WORKFLOW_EXECUTION_FAILED")
    );
    store.verify_workflow_history(id.instance_id).unwrap();
}

#[test]
fn workflow_descriptor_and_short_frontier_are_permanent_failures() {
    for mismatch in [true, false] {
        let (_tmp, store, id, limits) = setup();
        let run = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
        let token = grant(&store, &run.fence, &step(0, "a", 1), 0, &limits);
        store
            .complete_workflow_step(&run.fence, 0, &token, "1", 1, &limits)
            .unwrap();
        if mismatch {
            assert_eq!(
                store
                    .claim_workflow_step(&run.fence, &step(0, "b", 1), 2, &limits)
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowNonDeterministic
            );
        } else {
            assert_eq!(
                store
                    .finish_workflow(
                        &run.fence,
                        &WorkflowCompletion::Complete {
                            output_json: "null".into(),
                            final_ordinal: 0
                        },
                        2,
                        &limits
                    )
                    .unwrap(),
                WorkflowState::Errored
            );
        }
        assert_eq!(
            store
                .workflow_instance(id.instance_id)
                .unwrap()
                .unwrap()
                .error_code
                .as_deref(),
            Some("WORKFLOW_NON_DETERMINISTIC")
        );
        store.verify_workflow_history(id.instance_id).unwrap();
    }
}

#[test]
fn workflow_capacity_serialization_and_instance_identity_fail_closed() {
    let (_tmp, store, id, limits) = setup();
    assert_eq!(
        store
            .insert_workflow(&id, "null", None, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    let mut wrong = id.clone();
    wrong.creation_nonce = token().unwrap();
    assert_eq!(
        store.claim_workflow(&wrong, 0, &limits).unwrap_err().code(),
        ErrorCode::WorkflowInvariantViolation
    );
    let run = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
    let token = grant(&store, &run.fence, &step(0, "a", 1), 0, &limits);
    assert_eq!(
        store
            .complete_workflow_step(&run.fence, 0, &token, "[", 1, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        store
            .workflow_instance(id.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Errored
    );
    let mut id2 = id.clone();
    id2.instance_id = WorkflowInstanceId::generate();
    id2.external_instance_id = "two".into();
    let constrained = WorkflowsConfig {
        max_instances_per_account: 1,
        max_instances_per_definition: 1,
        max_active_per_account: 1,
        ..limits.clone()
    };
    assert_eq!(
        store
            .insert_workflow(&id2, "null", None, &constrained)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStateQuotaExceeded
    );
    assert_eq!(store.workflow_instance_ids(None, 100).unwrap().len(), 1);
    assert!(store.workflow_instance_ids(None, 0).is_err());
    assert!(
        store
            .due_workflows(0, 1001, &mut Default::default())
            .is_err()
    );
}

#[test]
fn workflow_quota_failure_can_always_persist_terminal_error() {
    let (_tmp, store, id, limits) = setup();
    let limits = WorkflowsConfig {
        max_state_bytes: 1024 * 1024,
        ..limits
    };
    let run = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
    let token = grant(&store, &run.fence, &step(0, "a", 1), 0, &limits);
    let huge = serde_json::to_string(&"x".repeat(1024 * 1024 - 2)).unwrap();
    assert_eq!(
        store
            .complete_workflow_step(&run.fence, 0, &token, &huge, 1, &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStateQuotaExceeded
    );
    let record = store.workflow_instance(id.instance_id).unwrap().unwrap();
    assert_eq!(record.state, WorkflowState::Errored);
    assert!(record.state_bytes <= limits.max_state_bytes);
    store.verify_workflow_history(id.instance_id).unwrap();
}

#[test]
fn workflow_step_count_config_and_parallel_requests_do_not_grant_callbacks() {
    for mode in 0..3 {
        let (_tmp, store, id, mut limits) = setup();
        limits.max_steps = 1;
        let run = store.claim_workflow(&id, 0, &limits).unwrap().unwrap();
        let mut descriptor = step(0, "a", 1);
        let expected = match mode {
            0 => {
                descriptor.config_json = "{}".into();
                ErrorCode::WorkflowStepConfigUnsupported
            }
            1 => {
                descriptor.ordinal = 1;
                ErrorCode::WorkflowStepLimitExceeded
            }
            _ => {
                descriptor.name_count = 2;
                ErrorCode::WorkflowNonDeterministic
            }
        };
        assert_eq!(
            store
                .claim_workflow_step(&run.fence, &descriptor, 0, &limits)
                .unwrap_err()
                .code(),
            expected
        );
        assert!(
            store
                .workflow_steps(id.instance_id, None, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .workflow_instance(id.instance_id)
                .unwrap()
                .unwrap()
                .state,
            WorkflowState::Errored
        );
    }
}
