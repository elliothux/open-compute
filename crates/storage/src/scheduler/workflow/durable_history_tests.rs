//! Admission and read-only history verification use the current SQLite schema.

use super::*;
use open_compute_core::workflow::WorkflowRetention;

#[test]
fn admission_freezes_retention_and_preserves_the_current_run_fence() {
    let (temp, store, identity) = setup();
    let id = identity.instance_id;
    let retention = WorkflowRetention {
        success_retention_ms: 3_600_000,
        error_retention_ms: 3_600_000,
    };
    let limits = WorkflowsConfig::default();
    store
        .insert_workflow(&identity, "{}", Some(&retention), &limits)
        .unwrap();
    let before = store.workflow_instance(id).unwrap().unwrap();
    assert_eq!(before.state_bytes, base_bytes(&identity) as u64);
    assert!(!before.durable.has_activated);
    for (input, retention) in [
        ("null", Some(&retention)),
        ("{}", None),
        ("{}", Some(&WorkflowRetention::default())),
    ] {
        assert_eq!(
            store
                .insert_workflow(&identity, input, retention, &limits)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInvariantViolation
        );
    }
    let run = store
        .claim_workflow(&identity, 1, &limits)
        .unwrap()
        .unwrap();
    assert_eq!(run.target.capability_version, 1);
    assert!(
        store
            .claim_workflow(&identity, 1, &limits)
            .unwrap()
            .is_none()
    );
    store.verify_workflow_history(id).unwrap();
    assert_eq!(workflow_invalid_rows(&store.lock().unwrap()).unwrap(), 0);
    drop(store);
    let reopened = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 3).unwrap();
    let record = reopened.workflow_instance(id).unwrap().unwrap();
    assert!(record.durable.has_activated);
    assert_eq!(record.durable.retention, retention);
    assert_eq!(record.run_token.unwrap(), run.fence.run_token);
    reopened.verify_workflow_history(id).unwrap();
}

#[test]
fn paused_instances_still_consume_active_capacity() {
    let (_temp, store, mut identity) = setup();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE workflow_instances SET state='paused',next_run_at_ms=NULL WHERE id=?1",
            [identity.instance_id.to_string()],
        )
        .unwrap();
    store.verify_workflow_history(identity.instance_id).unwrap();
    identity.instance_id = WorkflowInstanceId::generate();
    identity.external_instance_id = "next".into();
    let limits = WorkflowsConfig {
        max_active_per_account: 1,
        ..WorkflowsConfig::default()
    };
    assert_eq!(
        store
            .insert_workflow(
                &identity,
                "{}",
                Some(&WorkflowRetention::default()),
                &limits
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowStateQuotaExceeded
    );
    assert!(
        store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .is_none()
    );
}

fn settled_batch_then_wait(store: &SchedulerStore, id: WorkflowInstanceId) {
    let mut conn = store.lock().unwrap();
    let tx = conn.transaction().unwrap();
    activate(&tx, id);
    for ordinal in 0..2 {
        let mut item = descriptor(ordinal, WorkflowStepKind::Do, json!({"timeout":100}));
        item.batch_first_ordinal = 0;
        item.batch_size = 2;
        item.dependencies.clear();
        register(&tx, id, &item, 1, None, None);
    }
    tx.execute("UPDATE workflow_steps SET state='running',attempt=1,attempt_started_at_ms=2,attempt_deadline_at_ms=102,
        run_token=?2,step_token=?3,updated_at_ms=2 WHERE instance_id=?1", params![id.to_string(), &[0x11_u8;32][..], &[0x22_u8;32][..]]).unwrap();
    // The second callback settles first; a failed sibling is part of the settled frontier.
    tx.execute("UPDATE workflow_steps SET state='failed',error_json=?2,error_code='WORKFLOW_NON_RETRYABLE',completed_at_ms=3,updated_at_ms=3,
        run_token=NULL,step_token=NULL WHERE instance_id=?1 AND ordinal=1", params![id.to_string(),failure_json().as_bytes()]).unwrap();
    tx.execute("UPDATE workflow_steps SET state='complete',output_json=X'3131',completed_at_ms=4,updated_at_ms=4,
        run_token=NULL,step_token=NULL WHERE instance_id=?1 AND ordinal=0", [id.to_string()]).unwrap();
    let mut wait = descriptor(2, WorkflowStepKind::Sleep, json!({"duration":10}));
    wait.name_count = 1;
    wait.dependencies = vec![0, 1];
    register(&tx, id, &wait, 5, Some(15), None);
    tx.commit().unwrap();
}

#[test]
fn history_accepts_failed_siblings_dependency_barriers_and_wait_kinds_after_restart() {
    let (temp, store, identity) = setup();
    let id = identity.instance_id;
    settled_batch_then_wait(&store, id);
    store.verify_workflow_history(id).unwrap();
    {
        let mut conn = store.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,updated_at_ms=15,completed_at_ms=15
            WHERE instance_id=?1 AND ordinal=2", [id.to_string()]).unwrap();
        let mut event = descriptor(
            3,
            WorkflowStepKind::WaitEvent,
            json!({"type":"ok","timeout":0}),
        );
        event.name_count = 1;
        tx.execute(
            "INSERT INTO workflow_events VALUES(?1,1,1,'ok',X'37',16,35)",
            [id.to_string()],
        )
        .unwrap();
        register(&tx, id, &event, 16, Some(16), Some(1));
        let envelope = open_compute_core::workflow::WorkflowEventEnvelope {
            event_type: "ok",
            payload_json: "7",
            timestamp_ms: 16,
        }
        .canonical_json()
        .unwrap();
        tx.execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,output_json=?2,consumed_event_seq=1,completed_at_ms=16,updated_at_ms=16
            WHERE instance_id=?1 AND ordinal=3", params![id.to_string(),envelope.as_bytes()]).unwrap();
        tx.execute(
            "DELETE FROM workflow_events WHERE instance_id=?1 AND event_seq=1",
            [id.to_string()],
        )
        .unwrap();
        tx.execute("UPDATE workflow_instances SET state='complete',output_json=X'3131',state_bytes=state_bytes+2,
            run_token=NULL,run_claimed_at_ms=NULL,run_lease_until_ms=NULL,terminal_at_ms=17,updated_at_ms=17,expires_at_ms=3600017
            WHERE id=?1", [id.to_string()]).unwrap();
        tx.commit().unwrap();
    }
    store.verify_workflow_history(id).unwrap();
    assert_eq!(workflow_invalid_rows(&store.lock().unwrap()).unwrap(), 0);
    drop(store);
    let reopened = SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 20).unwrap();
    reopened.verify_workflow_history(id).unwrap();
    let metadata = reopened.workflow_instance(id).unwrap().unwrap().durable;
    assert_eq!(
        (metadata.registered_step_count, metadata.settled_step_count),
        (4, 4)
    );
    assert_eq!(metadata.event_count, 0);
    assert_eq!(metadata.next_event_seq, 2);
}

#[test]
fn history_detects_corrupt_descriptors_deadlines_edges_and_projections_without_repair() {
    let (_temp, store, identity) = setup();
    let id = identity.instance_id;
    settled_batch_then_wait(&store, id);
    store.verify_workflow_history(id).unwrap();
    let faults = [
        "UPDATE workflow_steps SET config_sha256=zeroblob(32) WHERE ordinal=0",
        "UPDATE workflow_steps SET descriptor_sha256=zeroblob(32) WHERE ordinal=0",
        "UPDATE workflow_steps SET attempt_deadline_at_ms=103 WHERE ordinal=0",
        "UPDATE workflow_steps SET output_json=X'2D30' WHERE ordinal=0",
        "UPDATE workflow_steps SET error_code='unknown' WHERE ordinal=1",
        "UPDATE workflow_steps SET name_count=2 WHERE ordinal=2",
        "UPDATE workflow_steps SET due_at_ms=16 WHERE ordinal=2",
        "UPDATE workflow_steps SET started_at_ms=9007199254740992 WHERE ordinal=2",
        "UPDATE workflow_instances SET state_bytes=state_bytes+1 WHERE capability_version=1",
        "UPDATE workflow_instances SET registered_step_count=4 WHERE capability_version=1",
        "DELETE FROM workflow_step_dependencies WHERE child_ordinal=2 AND parent_ordinal=0",
        "DELETE FROM workflow_step_dependencies WHERE child_ordinal=2; DELETE FROM workflow_steps WHERE ordinal=2",
    ];
    let mut conn = store.lock().unwrap();
    for fault in faults {
        let tx = conn.transaction().unwrap();
        let triggers = tx
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'workflow_%'",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for trigger in triggers {
            tx.execute_batch(&format!("DROP TRIGGER \"{trigger}\";"))
                .unwrap();
        }
        // Scope test corruption to this disposable database; production never disables guards.
        tx.execute_batch(fault).unwrap();
        let changes_before: u64 = tx
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            inspection::verify_history_connection(&tx, id)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowInvariantViolation,
            "{fault}"
        );
        let changes_after: u64 = tx
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            changes_before, changes_after,
            "history verification must not repair {fault}"
        );
        tx.rollback().unwrap();
        inspection::verify_history_connection(&conn, id).unwrap();
    }
}

#[test]
fn durable_yield_keeps_the_lease_until_drain_and_rechecks_the_settled_frontier() {
    for settle_before_yield in [false, true] {
        let (temp, store, identity) = setup();
        let id = identity.instance_id;
        let fence = WorkflowFence {
            instance_id: id,
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([0x11; 32]),
        };
        {
            let mut conn = store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            activate(&tx, id);
            register(
                &tx,
                id,
                &descriptor(0, WorkflowStepKind::Sleep, json!({"duration":2})),
                1,
                Some(3),
                None,
            );
            tx.commit().unwrap();
        }
        assert_eq!(
            store.yield_workflow(&fence, 2).unwrap_err().code(),
            ErrorCode::WorkflowInstanceStateConflict
        );
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_instances SET yield_requested=1,updated_at_ms=2 WHERE id=?1",
                [id.to_string()],
            )
            .unwrap();
        let before = store.workflow_instance(id).unwrap().unwrap();
        assert_eq!(before.state, WorkflowState::Running);
        assert_eq!(before.run_token.as_ref(), Some(&fence.run_token));
        if settle_before_yield {
            store.lock().unwrap().execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,completed_at_ms=3,updated_at_ms=3
                WHERE instance_id=?1", [id.to_string()]).unwrap();
        }
        let expected = if settle_before_yield {
            WorkflowState::Queued
        } else {
            WorkflowState::Waiting
        };
        assert_eq!(store.yield_workflow(&fence, 4).unwrap(), expected);
        assert_eq!(
            store.yield_workflow(&fence, 4).unwrap_err().code(),
            ErrorCode::WorkflowRunStale
        );
        store.verify_workflow_history(id).unwrap();
        drop(store);
        let reopened =
            SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 5).unwrap();
        let record = reopened.workflow_instance(id).unwrap().unwrap();
        assert_eq!(record.state, expected);
        assert!(record.run_token.is_none());
        assert!(!record.durable.yield_requested);
        assert_eq!(
            record.durable.next_wake_at_ms,
            if settle_before_yield { None } else { Some(3) }
        );
    }
}

#[test]
fn expired_current_recovery_preserves_business_attempt_and_deadline_and_honors_pause() {
    for pause in [false, true] {
        let (temp, store, identity) = setup();
        let id = identity.instance_id;
        let fence = WorkflowFence {
            instance_id: id,
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([0x11; 32]),
        };
        {
            let mut conn = store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            activate(&tx, id);
            register(
                &tx,
                id,
                &descriptor(0, WorkflowStepKind::Do, json!({"timeout":2000})),
                1,
                None,
                None,
            );
            tx.execute("UPDATE workflow_steps SET state='running',attempt=1,attempt_started_at_ms=1,attempt_deadline_at_ms=2001,
                run_token=?2,step_token=?3 WHERE instance_id=?1", params![id.to_string(), &[0x11_u8;32][..], &[0x22_u8;32][..]]).unwrap();
            tx.execute(
                "UPDATE workflow_instances SET yield_requested=1,pause_requested=?2 WHERE id=?1",
                params![id.to_string(), pause],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            store.yield_workflow(&fence, 2).unwrap_err().code(),
            ErrorCode::WorkflowInstanceBusy
        );
        assert!(
            store
                .workflow_instance(id)
                .unwrap()
                .unwrap()
                .run_token
                .is_some()
        );
        let limits = WorkflowsConfig {
            recovery_backoff_ms: 10,
            ..WorkflowsConfig::default()
        };
        assert_eq!(store.recover_workflows(999, &limits, 10).unwrap(), 0);
        assert_eq!(store.recover_workflows(1000, &limits, 10).unwrap(), 1);
        assert_eq!(store.recover_workflows(1000, &limits, 10).unwrap(), 0);
        assert_eq!(
            store.yield_workflow(&fence, 1000).unwrap_err().code(),
            ErrorCode::WorkflowRunStale
        );
        store.verify_workflow_history(id).unwrap();
        drop(store);
        let reopened =
            SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 5000, 1001).unwrap();
        let record = reopened.workflow_instance(id).unwrap().unwrap();
        assert_eq!(
            record.state,
            if pause {
                WorkflowState::Paused
            } else {
                WorkflowState::Queued
            }
        );
        assert_eq!(record.next_run_at_ms, if pause { None } else { Some(1010) });
        assert_eq!(record.durable.next_wake_at_ms, Some(2001));
        assert!(!record.durable.pause_requested);
        assert!(!record.durable.yield_requested);
        let attempt: (u32, i64, i64, String, bool) = reopened.lock().unwrap().query_row("SELECT attempt,attempt_started_at_ms,
            attempt_deadline_at_ms,state,run_token IS NULL AND step_token IS NULL FROM workflow_steps WHERE instance_id=?1",
            [id.to_string()], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
        assert_eq!(attempt, (1, 1, 2001, "pending".into(), true));
        reopened.verify_workflow_history(id).unwrap();
    }
}

#[test]
fn history_verifies_retry_delay_absolute_sleep_event_timeout_and_retained_inbox() {
    for kind in [
        WorkflowStepKind::Do,
        WorkflowStepKind::SleepUntil,
        WorkflowStepKind::WaitEvent,
    ] {
        let (_temp, store, identity) = setup();
        let id = identity.instance_id;
        {
            let mut conn = store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            activate(&tx, id);
            match kind {
                WorkflowStepKind::Do => {
                    register(
                        &tx,
                        id,
                        &descriptor(
                            0,
                            kind,
                            json!({"timeout":10,"retries":{"limit":1,"delay":5}}),
                        ),
                        1,
                        None,
                        None,
                    );
                    tx.execute("UPDATE workflow_steps SET state='running',attempt=1,attempt_started_at_ms=1,attempt_deadline_at_ms=11,
                        run_token=?2,step_token=?3 WHERE instance_id=?1", params![id.to_string(),&[0x11_u8;32][..],&[0x22_u8;32][..]]).unwrap();
                    tx.execute("UPDATE workflow_steps SET state='retry_wait',run_token=NULL,step_token=NULL,due_at_ms=16,
                        updated_at_ms=11,error_json=?2,error_code='WORKFLOW_STEP_TIMEOUT' WHERE instance_id=?1", params![id.to_string(),failure_json().as_bytes()]).unwrap();
                }
                WorkflowStepKind::SleepUntil => {
                    register(
                        &tx,
                        id,
                        &descriptor(0, kind, json!({"timestamp":-5})),
                        1,
                        Some(-5),
                        None,
                    );
                    tx.execute("UPDATE workflow_steps SET state='complete',due_at_ms=NULL,updated_at_ms=1,completed_at_ms=1
                        WHERE instance_id=?1", [id.to_string()]).unwrap();
                }
                WorkflowStepKind::WaitEvent => {
                    register(
                        &tx,
                        id,
                        &descriptor(0, kind, json!({"type":"ok","timeout":0})),
                        1,
                        Some(1),
                        Some(0),
                    );
                    tx.execute("UPDATE workflow_steps SET state='failed',due_at_ms=NULL,updated_at_ms=1,completed_at_ms=1,
                        error_json=?2,error_code='WORKFLOW_EVENT_TIMEOUT' WHERE instance_id=?1", params![id.to_string(),failure_json().as_bytes()]).unwrap();
                    tx.execute(
                        "INSERT INTO workflow_events VALUES(?1,1,1,'ok',X'37',2,35)",
                        [id.to_string()],
                    )
                    .unwrap();
                }
                WorkflowStepKind::Sleep => unreachable!(),
            }
            tx.commit().unwrap();
        }
        store.verify_workflow_history(id).unwrap();
        assert_eq!(workflow_invalid_rows(&store.lock().unwrap()).unwrap(), 0);
        let mut conn = store.lock().unwrap();
        let tx = conn.transaction().unwrap();
        if kind == WorkflowStepKind::WaitEvent {
            tx.execute_batch("DROP TRIGGER workflow_event_immutable; UPDATE workflow_events SET payload_json=X'61';").unwrap();
            assert_eq!(
                inspection::verify_history_connection(&tx, id)
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowInvariantViolation
            );
        } else if kind == WorkflowStepKind::Do {
            tx.execute_batch("DROP TRIGGER workflow_step_transition_guard; UPDATE workflow_steps SET due_at_ms=17;").unwrap();
            assert_eq!(
                inspection::verify_history_connection(&tx, id)
                    .unwrap_err()
                    .code(),
                ErrorCode::WorkflowInvariantViolation
            );
        }
        tx.rollback().unwrap();
        inspection::verify_history_connection(&conn, id).unwrap();
    }
}
