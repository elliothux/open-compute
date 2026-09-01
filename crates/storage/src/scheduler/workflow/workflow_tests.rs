//! Direct SQL guard/accounting regressions. These do not stand in for product driver tests.

use super::*;
use open_compute_core::workflow::{
    WorkflowStepDeclaration, WorkflowStepDescriptor, WorkflowStepKind,
};
use open_compute_core::{
    AccountId, DeploymentId, WorkerId, WorkflowId, WorkflowOperationId, WorkflowVersionId,
};
use serde_json::{Value, json};

const TEST_NULL_VALUE: &str = "T0NEVgECAA==";
const TEST_TRUE_VALUE: &str = "T0NEVgECAw==";
const TEST_SEVEN_VALUE: &str = "T0NEVgECBEAcAAAAAAAA";
const TEST_EIGHT_VALUE: &str = "T0NEVgECBEAgAAAAAAAA";

#[path = "durable_history_tests.rs"]
mod durable_history_tests;
#[path = "durable_protocol_tests.rs"]
mod durable_protocol_tests;

#[test]
fn workflow_model_debug_output_excludes_payloads_and_private_tokens() {
    let (_temp, store, identity) = setup();
    let instance = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    let instance_debug = format!("{instance:?}");
    assert!(instance_debug.contains(&identity.instance_id.to_string()));
    assert!(!instance_debug.contains(TEST_NULL_VALUE));

    let run = store
        .claim_workflow(&identity, 1, &WorkflowsConfig::default())
        .unwrap()
        .unwrap();
    let run_debug = format!("{run:?}");
    assert!(run_debug.contains("ClaimedWorkflowRun"));
    assert!(!run_debug.contains(TEST_NULL_VALUE));
    assert!(!run_debug.contains(&hex::encode(run.fence.run_token.as_bytes())));

    for completion in [
        WorkflowCompletion::Complete {
            output_json: TEST_TRUE_VALUE.to_owned(),
            final_ordinal: 1,
        },
        WorkflowCompletion::Errored {
            code: ErrorCode::WorkflowExecutionFailed,
        },
        WorkflowCompletion::Terminated { final_ordinal: 2 },
    ] {
        let debug = format!("{completion:?}");
        assert!(!debug.contains(TEST_TRUE_VALUE));
    }
    assert_eq!(WorkflowFailure::default().name, "Error");
}

#[test]
fn durable_operation_rejection_requires_a_non_null_code_on_insert_update_and_inspection() {
    let (_temp, store, identity) = setup();
    let operation = WorkflowOperationId::generate();
    let conn = store.lock().unwrap();
    let insert="INSERT INTO workflow_operation_progress(instance_id,operation_id,operation_sequence,creation_nonce,
        expected_generation,target_generation,kind,outcome,error_code,decided_at_ms) VALUES(?1,?2,1,?3,1,2,'restart','rejected',?4,1)";
    for code in [None, Some("unexpected")] {
        assert!(
            conn.execute(
                insert,
                params![
                    identity.instance_id.to_string(),
                    operation.to_string(),
                    identity.creation_nonce.as_bytes().as_slice(),
                    code
                ]
            )
            .is_err()
        );
    }
    conn.execute(
        insert,
        params![
            identity.instance_id.to_string(),
            operation.to_string(),
            identity.creation_nonce.as_bytes().as_slice(),
            "WORKFLOW_STATE_QUOTA_EXCEEDED"
        ],
    )
    .unwrap();
    assert!(
        conn.execute(
            "UPDATE workflow_operation_progress SET operation_sequence=2,error_code=NULL",
            []
        )
        .is_err()
    );
    verify_operation_progress(&conn).unwrap();
    // A corrupted pre-008 NULL decision must fail snapshot/recovery inspection too.
    conn.execute_batch(
        "DROP TRIGGER workflow_progress_rejection_update_guard;
        UPDATE workflow_operation_progress SET operation_sequence=2,error_code=NULL;",
    )
    .unwrap();
    assert_eq!(
        verify_operation_progress(&conn).unwrap_err().code(),
        ErrorCode::WorkflowInvariantViolation
    );
    drop(conn);
    assert_eq!(
        crate::inspect_scheduler_db(&_temp.path().join("scheduler.sqlite"), 5000, 2)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
}

#[test]
fn durable_ready_admission_rotates_accounts_and_reserves_every_fourth_selection_for_new_work() {
    let (_temp, store, template) = setup();
    let limits = WorkflowsConfig::default();
    // Remove pre-existing ready fixtures from admission without manufacturing history.
    for id in store.workflow_instance_ids(None, 10).unwrap() {
        let instance = store.workflow_instance(id).unwrap().unwrap();
        let run = store
            .claim_workflow(&instance.identity, 1, &limits)
            .unwrap()
            .unwrap();
        let completion = WorkflowCompletion::Errored {
            code: ErrorCode::WorkflowExecutionFailed,
        };
        store
            .finish_workflow(&run.fence, &completion, 2, &limits)
            .unwrap();
    }
    let mut accounts = [AccountId::generate(), AccountId::generate()];
    accounts.sort_by_key(ToString::to_string);
    let mut ready = Vec::new();
    for recovered in [true, false] {
        for account in accounts {
            let mut identity = template.clone();
            identity.instance_id = WorkflowInstanceId::generate();
            identity.external_instance_id = identity.instance_id.to_string();
            identity.creation_nonce = token().unwrap();
            identity.creation_operation_id = WorkflowOperationId::generate();
            identity.creation_batch_id = identity.creation_operation_id;
            identity.target.account_id = account;
            identity.target.descriptor_sha256 =
                crate::workflows::helpers::version_digest(&identity.target).unwrap();
            store
                .insert_workflow(
                    &identity,
                    TEST_NULL_VALUE,
                    Some(&Default::default()),
                    &limits,
                )
                .unwrap();
            if recovered {
                let run = store
                    .claim_workflow(&identity, 3, &limits)
                    .unwrap()
                    .unwrap();
                let step = descriptor(0, WorkflowStepKind::Sleep, json!({"duration":1}));
                store
                    .claim_workflow_batch(
                        &run.fence,
                        std::slice::from_ref(&step),
                        limits.dispatch_timeout_ms,
                        4,
                        &limits,
                    )
                    .unwrap();
                store.yield_workflow(&run.fence, 4).unwrap();
            }
            ready.push((identity.instance_id, account, recovered));
        }
    }
    store.maintain_workflow_due(5, &limits, 32).unwrap();
    let mut cursor = WorkflowClaimCursor::default();
    let mut selected = Vec::new();
    for _ in 0..8 {
        let id = store.due_workflows(5, 1, &mut cursor).unwrap()[0];
        selected.push(*ready.iter().find(|row| row.0 == id).unwrap());
    }
    assert_eq!(
        selected.iter().map(|row| row.2).collect::<Vec<_>>(),
        [true, true, true, false, true, true, true, false]
    );
    assert_eq!(
        selected.iter().take(4).map(|row| row.1).collect::<Vec<_>>(),
        [accounts[0], accounts[1], accounts[0], accounts[1]]
    );
    // Cursor loss starts from durable work; it neither invents nor removes a ready row.
    assert_eq!(
        store.due_workflows(5, 1, &mut Default::default()).unwrap()[0],
        selected[0].0
    );
    let plan = store
        .lock()
        .unwrap()
        .prepare(&format!("EXPLAIN QUERY PLAN {}", durable_due::DUE_STEPS))
        .unwrap()
        .query_map(params![5, 32], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    for index in [
        "workflow_steps_wait_due",
        "workflow_steps_delay_pending",
        "workflow_steps_retry_due",
        "workflow_steps_pending_timeout",
    ] {
        assert!(plan.contains(index), "{plan}");
    }
    assert!(!plan.contains("SCAN s\n"), "{plan}");
}

#[test]
fn scheduler_create_batch_is_atomic_and_idempotent_by_durable_operations() {
    let (_temp, store, template) = setup();
    let limits = WorkflowsConfig::default();
    let batch = WorkflowOperationId::generate();
    let make_identity = |external: &str| {
        let mut identity = template.clone();
        identity.instance_id = WorkflowInstanceId::generate();
        identity.external_instance_id = external.into();
        identity.creation_nonce = token().unwrap();
        identity.creation_operation_id = WorkflowOperationId::generate();
        identity.creation_batch_id = batch;
        identity.created_at_ms = 10;
        identity
    };
    let first = make_identity("batch-a");
    let second = make_identity("batch-b");
    let retention = open_compute_core::workflow::WorkflowRetention::default();
    let requests = [
        (&first, "T0NEVgECAA==", Some(&retention)),
        (&second, "T0NEVgECAA==", Some(&retention)),
    ];
    store
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_scheduler_batch BEFORE INSERT ON workflow_instances
             WHEN NEW.external_instance_id='batch-b'
             BEGIN SELECT RAISE(ABORT,'test batch fault'); END;",
        )
        .unwrap();
    assert!(store.insert_workflows(&requests, &limits).is_err());
    assert!(
        store
            .workflow_instance(first.instance_id)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .workflow_instance(second.instance_id)
            .unwrap()
            .is_none()
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_scheduler_batch;")
        .unwrap();

    store.insert_workflows(&requests, &limits).unwrap();
    store.insert_workflows(&requests, &limits).unwrap();
    assert_eq!(
        store
            .workflow_instance(first.instance_id)
            .unwrap()
            .unwrap()
            .identity,
        first
    );
    assert_eq!(
        store
            .workflow_instance(second.instance_id)
            .unwrap()
            .unwrap()
            .identity,
        second
    );
}

fn setup() -> (tempfile::TempDir, SchedulerStore, WorkflowInstanceIdentity) {
    let temp = tempfile::tempdir().unwrap();
    let store = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 0).unwrap();
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
    let creation_operation = WorkflowOperationId::generate();
    let identity = WorkflowInstanceIdentity {
        instance_id: WorkflowInstanceId::generate(),
        external_instance_id: "durable".into(),
        target,
        instance_generation: 1,
        creation_nonce: token().unwrap(),
        creation_operation_id: creation_operation,
        creation_batch_id: creation_operation,
        created_at_ms: 0,
        schedule: None,
    };
    store
        .insert_workflow(
            &identity,
            "T0NEVgECAA==",
            Some(&open_compute_core::workflow::WorkflowRetention {
                success_retention_ms: 3_600_000,
                error_retention_ms: 3_600_000,
            }),
            &WorkflowsConfig::default(),
        )
        .unwrap();
    (temp, store, identity)
}

fn base_bytes(identity: &WorkflowInstanceIdentity) -> usize {
    open_compute_core::workflow::WORKFLOW_INSTANCE_BYTES
        + "T0NEVgECAA==".len()
        + identity.target.definition_name.len()
        + identity.external_instance_id.len()
        + identity.target.class_name.len()
}

fn activate(conn: &Connection, id: WorkflowInstanceId) {
    conn.execute("UPDATE workflow_instances SET state='running',next_run_at_ms=NULL,run_token=?2,run_claimed_at_ms=0,
        run_lease_until_ms=1000,has_activated=1 WHERE id=?1",params![id.to_string(),&[0x11_u8;32][..]]).unwrap();
}

fn descriptor(ordinal: u32, kind: WorkflowStepKind, config: Value) -> WorkflowStepDescriptor {
    WorkflowStepDeclaration {
        ordinal,
        kind,
        name: "step".into(),
        name_count: ordinal + 1,
        config,
        rollback_config: None,
        rollback_step: false,
        dependencies: if ordinal == 0 {
            vec![]
        } else {
            vec![ordinal - 1]
        },
        batch_first_ordinal: ordinal,
        batch_size: 1,
    }
    .resolve()
    .unwrap()
}

fn register(
    conn: &Connection,
    id: WorkflowInstanceId,
    step: &WorkflowStepDescriptor,
    now_ms: i64,
    due: Option<i64>,
    ceiling: Option<i64>,
) {
    let config = step.canonical_config_json().unwrap();
    let config_hash: [u8; 32] = Sha256::digest(config.as_bytes()).into();
    conn.execute("INSERT INTO workflow_steps(instance_id,instance_generation,ordinal,name,name_count,kind,config_json,descriptor_sha256,
        state,attempt,started_at_ms,updated_at_ms,config_sha256,batch_first_ordinal,batch_size,dependency_count,due_at_ms,event_buffer_ceiling)
        VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,0,?9,?9,?10,?11,?12,?13,?14,?15)",params![id.to_string(),step.ordinal,step.name,step.name_count,
            step.config.kind().as_str(),config.as_bytes(),step.sha256().unwrap().as_slice(),if step.config.kind()==WorkflowStepKind::Do {"pending"} else {"waiting"},
            now_ms,config_hash.as_slice(),step.batch_first_ordinal,step.batch_size,step.dependencies.len(),due,ceiling]).unwrap();
    for parent in &step.dependencies {
        conn.execute(
            "INSERT INTO workflow_step_dependencies VALUES(?1,1,?2,?3)",
            params![id.to_string(), step.ordinal, parent],
        )
        .unwrap();
    }
}

fn operation(
    conn: &Connection,
    identity: &WorkflowInstanceIdentity,
    kind: &str,
    now_ms: i64,
) -> WorkflowOperationId {
    let id = WorkflowOperationId::generate();
    let restart_retain_step_count = (kind == "restart").then_some(0);
    let restart_next_event_seq = (kind == "restart").then_some(1);
    conn.execute(
        "INSERT INTO workflow_mutation_context(instance_id,operation_id,creation_nonce,expected_generation,
         target_generation,kind,restart_retain_step_count,restart_next_event_seq,authorized_at_ms)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            identity.instance_id.to_string(),
            id.to_string(),
            identity.creation_nonce.as_bytes().as_slice(),
            identity.instance_generation,
            identity.instance_generation + i64::from(kind == "restart"),
            kind,
            restart_retain_step_count,
            restart_next_event_seq,
            now_ms
        ],
    )
    .unwrap();
    id
}

#[test]
fn current_do_result_is_immutable_and_restart_purge_need_exact_scoped_context() {
    let (_temp, store, mut identity) = setup();
    let queued = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(queued.state, WorkflowState::Queued);
    assert_eq!(queued.durable.retention.success_retention_ms, 3_600_000);
    assert_eq!(queued.durable.next_event_seq, 1);
    let mut conn = store.lock().unwrap();
    let id = identity.instance_id;
    let step = descriptor(0, WorkflowStepKind::Do, json!({"timeout":10}));
    {
        let tx = conn.transaction().unwrap();
        activate(&tx, id);
        register(&tx, id, &step, 1, None, None);
        tx.execute("UPDATE workflow_steps SET state='running',attempt=1,attempt_started_at_ms=1,attempt_deadline_at_ms=11,
            run_token=?2,step_token=?3 WHERE instance_id=?1",params![id.to_string(),&[0x11_u8;32][..],&[0x22_u8;32][..]]).unwrap();
        assert!(tx.execute("UPDATE workflow_steps SET state='complete',output_json=X'37',completed_at_ms=11,updated_at_ms=11,
            run_token=NULL,step_token=NULL WHERE instance_id=?1",[id.to_string()]).is_err());
        tx.execute("UPDATE workflow_steps SET state='complete',output_json=X'37',completed_at_ms=10,updated_at_ms=10,
            run_token=NULL,step_token=NULL WHERE instance_id=?1",[id.to_string()]).unwrap();
        assert!(
            tx.execute(
                "UPDATE workflow_steps SET output_json=X'38' WHERE instance_id=?1",
                [id.to_string()]
            )
            .is_err()
        );
        tx.execute("UPDATE workflow_instances SET state='complete',output_json=X'37',state_bytes=state_bytes+1,
            run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL,terminal_at_ms=10,expires_at_ms=3600010,updated_at_ms=10 WHERE id=?1",
            [id.to_string()]).unwrap();
        let bytes: usize = tx
            .query_row(
                "SELECT state_bytes FROM workflow_instances WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            bytes,
            base_bytes(&identity) + step.state_bytes().unwrap() + 2
        );
        assert!(
            tx.execute(
                "DELETE FROM workflow_steps WHERE instance_id=?1",
                [id.to_string()]
            )
            .is_err()
        );
        assert!(
            tx.execute(
                "DELETE FROM workflow_instances WHERE id=?1",
                [id.to_string()]
            )
            .is_err()
        );
        tx.commit().unwrap();
    }
    drop(conn);
    let completed = store.workflow_instance(id).unwrap().unwrap();
    assert_eq!(completed.state, WorkflowState::Complete);
    assert_eq!(completed.durable.registered_step_count, 1);
    assert_eq!(completed.durable.settled_step_count, 1);
    assert_eq!(completed.durable.expires_at_ms, Some(3_600_010));
    let mut conn = store.lock().unwrap();
    {
        let tx = conn.transaction().unwrap();
        let op = operation(&tx, &identity, "restart", 12);
        assert!(
            tx.execute(
                "DELETE FROM workflow_mutation_context WHERE operation_id=?1",
                [op.to_string()]
            )
            .is_err()
        );
        tx.execute(
            "DELETE FROM workflow_steps WHERE instance_id=?1",
            [id.to_string()],
        )
        .unwrap();
        tx.execute("UPDATE workflow_instances SET instance_generation=2,last_restart_operation_id=?2,state='queued',next_run_at_ms=12,
            has_activated=0,output_json=NULL,state_bytes=?3,terminal_at_ms=NULL,expires_at_ms=NULL,updated_at_ms=12 WHERE id=?1",
            params![id.to_string(),op.to_string(),base_bytes(&identity)]).unwrap();
        tx.execute(
            "DELETE FROM workflow_mutation_context WHERE operation_id=?1",
            [op.to_string()],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    identity.instance_generation = 2;
    conn.execute("UPDATE workflow_instances SET state='terminated',next_run_at_ms=NULL,terminal_at_ms=20,expires_at_ms=3600020,updated_at_ms=20 WHERE id=?1",[id.to_string()]).unwrap();
    drop(conn);
    let terminated = store.workflow_instance(id).unwrap().unwrap();
    assert_eq!(terminated.state, WorkflowState::Terminated);
    assert!(terminated.state.is_terminal());
    assert_eq!(terminated.identity.instance_generation, 2);
    assert!(terminated.durable.last_restart_operation_id.is_some());
    let mut conn = store.lock().unwrap();
    let op;
    {
        let tx = conn.transaction().unwrap();
        tx.execute_batch("SAVEPOINT explicit_purge_before_retention")
            .unwrap();
        assert_eq!(
            tx.execute(
                "INSERT INTO workflow_mutation_context(instance_id,operation_id,creation_nonce,expected_generation,
                 target_generation,kind,authorized_at_ms) VALUES(?1,?2,?3,2,2,'purge',3600019)",
                params![
                    id.to_string(),
                    WorkflowOperationId::generate().to_string(),
                    identity.creation_nonce.as_bytes().as_slice()
                ]
            )
            .unwrap(),
            1
        );
        tx.execute_batch(
            "ROLLBACK TO explicit_purge_before_retention; RELEASE explicit_purge_before_retention",
        )
        .unwrap();
        op = operation(&tx, &identity, "purge", 3600020);
        tx.execute(
            "DELETE FROM workflow_instances WHERE id=?1",
            [id.to_string()],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO workflow_gc_receipts VALUES(?1,?2,?3,?4,2,3600020)",
            params![
                op.to_string(),
                id.to_string(),
                identity.creation_nonce.as_bytes().as_slice(),
                identity.creation_operation_id.to_string()
            ],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM workflow_mutation_context WHERE operation_id=?1",
            [op.to_string()],
        )
        .unwrap();
        assert!(
            tx.execute(
                "DELETE FROM workflow_gc_receipts WHERE operation_id=?1",
                [op.to_string()]
            )
            .is_err()
        );
        tx.commit().unwrap();
    }
    {
        let tx = conn.transaction().unwrap();
        tx.execute("INSERT INTO workflow_mutation_context(instance_id,operation_id,creation_nonce,expected_generation,
            target_generation,kind,authorized_at_ms) VALUES(?1,?2,?3,2,2,'acknowledge_purge',3600021)",
            params![id.to_string(),op.to_string(),identity.creation_nonce.as_bytes().as_slice()]).unwrap();
        tx.execute(
            "DELETE FROM workflow_gc_receipts WHERE operation_id=?1",
            [op.to_string()],
        )
        .unwrap();
        tx.execute(
            "DELETE FROM workflow_mutation_context WHERE operation_id=?1",
            [op.to_string()],
        )
        .unwrap();
        tx.commit().unwrap();
    }
}

#[test]
fn current_buffered_event_precedes_zero_timeout_but_equal_deadline_new_event_is_not_consumed() {
    let (_temp, store, identity) = setup();
    let mut conn = store.lock().unwrap();
    let id = identity.instance_id;
    let tx = conn.transaction().unwrap();
    activate(&tx, id);
    tx.execute(
        "INSERT INTO workflow_events VALUES(?1,1,1,'approved',?2,1,60)",
        params![id.to_string(), TEST_SEVEN_VALUE.as_bytes()],
    )
    .unwrap();
    let first = descriptor(
        0,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approved","timeout":0}),
    );
    register(&tx, id, &first, 1, Some(1), Some(1));
    let envelope = open_compute_core::workflow::WorkflowEventEnvelope {
        event_type: "approved",
        payload_base64: TEST_SEVEN_VALUE,
        timestamp_ms: 1,
    }
    .canonical_wire()
    .unwrap();
    tx.execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,output_json=?2,consumed_event_seq=1,completed_at_ms=1 WHERE instance_id=?1",
        params![id.to_string(),envelope.as_bytes()]).unwrap();
    tx.execute(
        "DELETE FROM workflow_events WHERE instance_id=?1 AND event_seq=1",
        [id.to_string()],
    )
    .unwrap();
    let second = descriptor(
        1,
        WorkflowStepKind::WaitEvent,
        json!({"type":"approved","timeout":0}),
    );
    register(&tx, id, &second, 2, Some(2), Some(1));
    tx.execute(
        "INSERT INTO workflow_events VALUES(?1,1,2,'approved',?2,2,60)",
        params![id.to_string(), TEST_EIGHT_VALUE.as_bytes()],
    )
    .unwrap();
    let late = open_compute_core::workflow::WorkflowEventEnvelope {
        event_type: "approved",
        payload_base64: TEST_EIGHT_VALUE,
        timestamp_ms: 2,
    }
    .canonical_wire()
    .unwrap();
    assert!(tx.execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,output_json=?2,consumed_event_seq=2,
        completed_at_ms=2 WHERE instance_id=?1 AND ordinal=1",params![id.to_string(),late.as_bytes()]).is_err());
    tx.execute("UPDATE workflow_steps SET state='failed',due_at_ms=NULL,error_code='WORKFLOW_EVENT_TIMEOUT',error_json=?2,
        completed_at_ms=2 WHERE instance_id=?1 AND ordinal=1",params![id.to_string(),failure_json().as_bytes()]).unwrap();
    assert!(
        tx.execute(
            "DELETE FROM workflow_events WHERE instance_id=?1 AND event_seq=2",
            [id.to_string()]
        )
        .is_err()
    );
    let (registered,settled,complete,events,event_bytes,bytes):(u32,u32,u32,u32,u32,usize)=tx.query_row(
        "SELECT registered_step_count,settled_step_count,completed_step_count,event_count,event_bytes,state_bytes FROM workflow_instances WHERE id=?1",
        [id.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).unwrap();
    assert_eq!(
        (registered, settled, complete, events, event_bytes),
        (2, 2, 1, 1, 60)
    );
    assert_eq!(
        bytes,
        base_bytes(&identity)
            + first.state_bytes().unwrap()
            + second.state_bytes().unwrap()
            + envelope.len()
            + failure_json().len()
            + 60
    );
    tx.commit().unwrap();
    drop(conn);
    let steps = store.workflow_steps(id, None, 10).unwrap();
    assert_eq!(
        steps[1].error_code.as_deref(),
        Some("WORKFLOW_EVENT_TIMEOUT")
    );
    let record = store.workflow_instance(id).unwrap().unwrap();
    let metadata = record.durable;
    assert_eq!(
        (
            metadata.event_count,
            metadata.event_bytes,
            metadata.next_event_seq
        ),
        (1, 60, 3)
    );
    assert_eq!(
        (metadata.registered_step_count, metadata.settled_step_count),
        (2, 2)
    );
}

#[test]
fn current_paused_instance_is_readable_without_an_activation_lease() {
    let (_temp, store, identity) = setup();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE workflow_instances SET state='paused',next_run_at_ms=NULL,
        updated_at_ms=1 WHERE id=?1",
            [identity.instance_id.to_string()],
        )
        .unwrap();
    let record = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, WorkflowState::Paused);
    assert!(!record.state.is_terminal());
    assert!(record.run_token.is_none());
    assert_eq!(record.durable.expires_at_ms, None);
    let inspection = store
        .inspect_workflow_instances(
            identity.target.account_id,
            identity.target.definition_id,
            None,
            10,
            2,
        )
        .unwrap();
    assert_eq!(
        inspection
            .iter()
            .find(|row| row.id == identity.instance_id)
            .unwrap()
            .status,
        WorkflowState::Paused
    );
}
