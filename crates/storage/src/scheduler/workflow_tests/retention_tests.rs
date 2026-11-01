use super::*;
use crate::scheduler::SchedulerWakeSignal;
use crate::schema_migrations::{self, DatabaseKind};
use open_compute_core::workflow::WorkflowRetention;
use std::sync::{Arc, Mutex};

#[test]
fn retention_migration_preserves_active_history_and_removes_the_old_minimum() {
    let (_temp, template, identity) = setup();
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 23);
    let store = SchedulerStore {
        connection: Mutex::new(connection),
        wake: Arc::new(SchedulerWakeSignal::default()),
        instance_id: template.instance_id(),
    };
    store.bind_instance(template.instance_id()).unwrap();
    let limits = WorkflowsConfig::default();
    let short = WorkflowRetention {
        success_retention_ms: 86_400_000,
        error_retention_ms: 3_600,
    };
    assert_eq!(
        store
            .insert_workflow(&identity, TEST_NULL_VALUE, Some(&short), &limits)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInvariantViolation
    );
    store
        .insert_workflow(
            &identity,
            TEST_NULL_VALUE,
            Some(&Default::default()),
            &limits,
        )
        .unwrap();
    store
        .send_workflow_event(
            &identity,
            WorkflowOperationId::generate(),
            "event",
            TEST_TRUE_VALUE,
            1,
            &limits,
        )
        .unwrap();
    let run = store
        .claim_workflow(&identity, 2, &limits)
        .unwrap()
        .unwrap();
    let before = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    let guards_query = "SELECT group_concat(sql, char(10)) FROM
        (SELECT sql FROM sqlite_schema WHERE type IN ('index','trigger','view') ORDER BY name)";
    let guards_before: String = store
        .lock()
        .unwrap()
        .query_row(guards_query, [], |row| row.get(0))
        .unwrap();
    schema_migrations::migrate(&mut store.lock().unwrap(), DatabaseKind::Control).unwrap();
    let after = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.identity, before.identity);
    assert_eq!(after.state_bytes, before.state_bytes);
    assert_eq!(after.durable.event_count, 1);
    assert_eq!(after.durable.retention, before.durable.retention);
    assert_eq!(after.run_token.unwrap(), run.fence.run_token);
    store.verify_workflow_history(identity.instance_id).unwrap();
    let connection = store.lock().unwrap();
    let guards_after: String = connection
        .query_row(guards_query, [], |row| row.get(0))
        .unwrap();
    assert_eq!(guards_after, guards_before);
    assert!(
        !connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    drop(connection);
    let mut short_identity = identity;
    short_identity.instance_id = WorkflowInstanceId::generate();
    short_identity.external_instance_id = "short".to_owned();
    short_identity.creation_operation_id = WorkflowOperationId::generate();
    short_identity.creation_batch_id = short_identity.creation_operation_id;
    short_identity.creation_nonce = token().unwrap();
    store
        .insert_workflow(&short_identity, TEST_NULL_VALUE, Some(&short), &limits)
        .unwrap();
}

#[test]
fn sub_hour_retention_is_frozen_and_sets_exact_terminal_expiry() {
    let (_temp, store, template) = setup();
    let limits = WorkflowsConfig::default();
    for duration in [0, 1, 3_600, 3_599_999] {
        for success in [false, true] {
            let mut identity = template.clone();
            identity.instance_id = WorkflowInstanceId::generate();
            identity.external_instance_id = format!("short-{duration}-{success}");
            identity.creation_operation_id = WorkflowOperationId::generate();
            identity.creation_batch_id = identity.creation_operation_id;
            identity.creation_nonce = token().unwrap();
            let retention = WorkflowRetention {
                success_retention_ms: duration,
                error_retention_ms: duration,
            };
            store
                .insert_workflow(&identity, TEST_NULL_VALUE, Some(&retention), &limits)
                .unwrap();
            let run = store
                .claim_workflow(&identity, 1, &limits)
                .unwrap()
                .unwrap();
            let completion = if success {
                WorkflowCompletion::Complete {
                    output_json: TEST_TRUE_VALUE.to_owned(),
                    final_ordinal: 0,
                }
            } else {
                WorkflowCompletion::Errored {
                    code: ErrorCode::WorkflowExecutionFailed,
                }
            };
            store
                .finish_workflow(&run.fence, &completion, 2, &limits)
                .unwrap();
            let record = store
                .workflow_instance(identity.instance_id)
                .unwrap()
                .unwrap();
            assert_eq!(record.durable.retention, retention);
            assert_eq!(record.durable.expires_at_ms, Some(2 + duration as i64));
            store.verify_workflow_history(identity.instance_id).unwrap();
        }
    }
}
