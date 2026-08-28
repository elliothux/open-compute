use super::*;
use open_compute_core::CronRunId;

#[test]
fn workflow_v7_due_indexes_and_rejection_guards_upgrade_atomically() {
    for fault in [
        SchedulerMigrationFault::BeforeExecution,
        SchedulerMigrationFault::BeforeMigrationRow,
        SchedulerMigrationFault::AfterCommit,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("scheduler.sqlite");
        let store = SchedulerStore {
            connection: Mutex::new(create_scheduler_fixture_at_version(&path, 7)),
            wake: Arc::new(SchedulerWakeSignal::default()),
        };
        let before = legacy_bytes(&store);
        assert!(store.migrate(10, Some(fault)).is_err());
        assert_eq!(legacy_bytes(&store), before);
        let committed = fault == SchedulerMigrationFault::AfterCommit;
        assert_eq!(
            store
                .lock()
                .unwrap()
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            if committed { 8 } else { 7 }
        );
        let index: bool = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='workflow_instances_fair')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index, committed);
        drop(store);
        let store = SchedulerStore::open(&path, 5000, 11).unwrap();
        assert_eq!(legacy_bytes(&store), before);
        workflow::verify_operation_progress(&store.lock().unwrap()).unwrap();
    }
}

#[test]
fn workflow_v6_upgrade_retains_restart_and_gc_proofs_through_migration_faults() {
    use crate::{WorkflowInstanceIdentity, WorkflowTarget};
    use open_compute_core::{
        WorkflowId, WorkflowInstanceId, WorkflowOperationId, WorkflowToken, WorkflowVersionId,
        WorkflowsConfig,
    };
    for fault in [
        SchedulerMigrationFault::BeforeExecution,
        SchedulerMigrationFault::BeforeMigrationRow,
        SchedulerMigrationFault::AfterCommit,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("scheduler.sqlite");
        let store = SchedulerStore {
            connection: Mutex::new(create_scheduler_fixture_at_version(&path, 6)),
            wake: Arc::new(SchedulerWakeSignal::default()),
        };
        let mut target = WorkflowTarget {
            account_id: AccountId::generate(),
            definition_id: WorkflowId::generate(),
            definition_name: "legacy-durable".into(),
            version_id: WorkflowVersionId::generate(),
            worker_id: WorkerId::generate(),
            deployment_id: DeploymentId::generate(),
            worker_code_sha256: [1; 32],
            loader_schema_version: 1,
            capability_version: 2,
            descriptor_sha256: [0; 32],
            class_name: "Flow".into(),
        };
        target.descriptor_sha256 = crate::workflows::helpers::version_digest(&target).unwrap();
        let identity = WorkflowInstanceIdentity {
            instance_id: WorkflowInstanceId::generate(),
            external_instance_id: "restart".into(),
            target,
            instance_generation: 1,
            creation_nonce: WorkflowToken::from_bytes([2; 32]),
            created_at_ms: 0,
        };
        let limits = WorkflowsConfig::default();
        let retention = open_compute_core::workflow::WorkflowRetention {
            success_retention_ms: 3600000,
            error_retention_ms: 3600000,
        };
        store
            .insert_workflow(&identity, "null", Some(&retention), &limits)
            .unwrap();
        let restart = WorkflowOperationId::generate();
        {
            let mut conn = store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            // The schema-six context is the historical authority, before progress/sequence existed.
            tx.execute(
                "INSERT INTO workflow_mutation_context VALUES(?1,?2,?3,1,2,'restart',10)",
                params![
                    identity.instance_id.to_string(),
                    restart.to_string(),
                    identity.creation_nonce.as_bytes().as_slice()
                ],
            )
            .unwrap();
            tx.execute("UPDATE workflow_instances SET instance_generation=2,last_restart_operation_id=?2,updated_at_ms=10 WHERE id=?1",
                params![identity.instance_id.to_string(),restart.to_string()]).unwrap();
            tx.execute("DELETE FROM workflow_mutation_context", [])
                .unwrap();
            tx.commit().unwrap();
        }
        let mut deleted = identity.clone();
        deleted.instance_id = WorkflowInstanceId::generate();
        deleted.external_instance_id = "purge".into();
        store
            .insert_workflow(&deleted, "null", Some(&retention), &limits)
            .unwrap();
        store
            .modify_workflow_v2(&deleted, WorkflowInstanceAction::Terminate, 20, &limits)
            .unwrap();
        let purge = WorkflowOperationId::generate();
        {
            let mut conn = store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO workflow_mutation_context VALUES(?1,?2,?3,1,1,'purge',3600020)",
                params![
                    deleted.instance_id.to_string(),
                    purge.to_string(),
                    deleted.creation_nonce.as_bytes().as_slice()
                ],
            )
            .unwrap();
            tx.execute(
                "DELETE FROM workflow_instances WHERE id=?1",
                [deleted.instance_id.to_string()],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO workflow_gc_receipts VALUES(?1,?2,?3,1,3600020)",
                params![
                    purge.to_string(),
                    deleted.instance_id.to_string(),
                    deleted.creation_nonce.as_bytes().as_slice()
                ],
            )
            .unwrap();
            tx.execute("DELETE FROM workflow_mutation_context", [])
                .unwrap();
            tx.commit().unwrap();
        }
        let before = legacy_bytes(&store);
        assert!(store.migrate(3600021, Some(fault)).is_err());
        assert_eq!(legacy_bytes(&store), before);
        assert_eq!(
            store
                .lock()
                .unwrap()
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            if fault == SchedulerMigrationFault::AfterCommit {
                7
            } else {
                6
            }
        );
        drop(store);
        let store = SchedulerStore::open(&path, 5000, 3600021).unwrap();
        assert_eq!(legacy_bytes(&store), before);
        let record = store
            .workflow_instance(identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.identity.instance_generation, 2);
        assert_eq!(
            record.durable.unwrap().last_restart_operation_id,
            Some(restart)
        );
        let receipt = store.workflow_gc_receipts(None, 10).unwrap().remove(0);
        assert_eq!(receipt.instance_id(), deleted.instance_id);
        let progress: u64 = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM workflow_operation_progress WHERE operation_sequence=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(progress, 2);
        store.verify_workflow_history(identity.instance_id).unwrap();
    }
}

#[test]
fn workflow_scheduler_v4_upgrade_preserves_cron_claim_and_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let connection = create_scheduler_fixture_at_version(&path, 4);
    let activation = CronActivationId::generate();
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let deployment = DeploymentId::generate();
    let run = CronRunId::generate();
    connection.execute_batch(&format!("INSERT INTO cron_schedules
        (activation_id,account_id,worker_id,deployment_id,execution_generation,activation_generation,
         expression,expression_sha256,parser_version,state,next_fire_at_ms,updated_at_ms)
        VALUES ('{activation}','{account}','{worker}','{deployment}',1,1,'* * * * *',zeroblob(32),1,'accepting',60000,0);
        INSERT INTO cron_runs(id,activation_id,activation_generation,scheduled_at_ms,deployment_id,
          execution_generation,expression,state,next_attempt_at_ms,created_at_ms)
        VALUES ('{run}','{activation}',1,60000,'{deployment}',1,'* * * * *','ready',60000,0);
        UPDATE cron_runs SET state='claimed',next_attempt_at_ms=NULL,claim_token=zeroblob(32),
            claimed_at_ms=60000,claim_until_ms=120000 WHERE id='{run}';")).unwrap();
    drop(connection);
    for now_ms in [60001, 60002] {
        let store = SchedulerStore::open(&path, 1000, now_ms).unwrap();
        assert_eq!(store.inspect_workflows(now_ms).unwrap(), Default::default());
        drop(store);
    }
    let connection = Connection::open(path).unwrap();
    let preserved = connection
        .query_row(
            "SELECT state,claim_until_ms,attempt,length(claim_token) FROM cron_runs WHERE id=?1",
            [run.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(preserved, ("claimed".into(), 120000, 0, 32));
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        current_scheduler_schema_version()
    );
}

fn legacy_workflows() -> (
    tempfile::TempDir,
    SchedulerStore,
    Vec<open_compute_core::WorkflowInstanceId>,
) {
    use crate::{WorkflowInstanceIdentity, WorkflowTarget};
    use open_compute_core::{
        WorkflowId, WorkflowInstanceId, WorkflowToken, WorkflowVersionId, WorkflowsConfig,
    };
    let temp = tempfile::tempdir().unwrap();
    let connection = create_scheduler_fixture_at_version(&temp.path().join("scheduler.sqlite"), 5);
    let store = SchedulerStore {
        connection: Mutex::new(connection),
        wake: Arc::new(SchedulerWakeSignal::default()),
    };
    let limits = WorkflowsConfig {
        lease_ms: 100,
        heartbeat_ms: 20,
        dispatch_timeout_ms: 1000,
        recovery_backoff_ms: 1,
        ..Default::default()
    };
    let mut target = WorkflowTarget {
        account_id: AccountId::generate(),
        definition_id: WorkflowId::generate(),
        definition_name: "legacy".into(),
        version_id: WorkflowVersionId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        worker_code_sha256: [1; 32],
        class_name: "Flow".into(),
        loader_schema_version: 1,
        capability_version: 1,
        descriptor_sha256: [0; 32],
    };
    target.descriptor_sha256 = crate::workflows::helpers::version_digest(&target).unwrap();
    let mut ids = Vec::new();
    for state in ["pending", "queued", "running", "complete", "errored"] {
        let identity = WorkflowInstanceIdentity {
            instance_id: WorkflowInstanceId::generate(),
            external_instance_id: state.into(),
            target: target.clone(),
            instance_generation: 1,
            creation_nonce: WorkflowToken::from_bytes([ids.len() as u8; 32]),
            created_at_ms: 0,
        };
        // The fixture writes the frozen schema-five contract. Current production
        // admission always runs after migrations and must not acquire a legacy write path.
        store.lock().unwrap().execute("INSERT INTO workflow_instances(id,account_id,definition_id,definition_name,external_instance_id,
            version_id,worker_id,deployment_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,
            class_name,creation_nonce,instance_generation,state,input_json,next_run_at_ms,state_bytes,created_at_ms,updated_at_ms)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,1,?10,?11,?12,1,'queued',?13,0,?14,0,0)",
            params![identity.instance_id.to_string(),target.account_id.to_string(),target.definition_id.to_string(),target.definition_name,
                identity.external_instance_id,target.version_id.to_string(),target.worker_id.to_string(),target.deployment_id.to_string(),
                target.worker_code_sha256.as_slice(),target.descriptor_sha256.as_slice(),target.class_name,
                identity.creation_nonce.as_bytes().as_slice(),br#"{"retained":true}"#.as_slice(),br#"{"retained":true}"#.len()]).unwrap();
        ids.push(identity.instance_id);
        if state == "queued" {
            continue;
        }
        let run_token = WorkflowToken::from_bytes([0x61; 32]);
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE workflow_instances SET state='running',run_token=?2,run_claimed_at_ms=200,
            run_lease_until_ms=300,next_run_at_ms=NULL,updated_at_ms=200 WHERE id=?1",
                params![
                    identity.instance_id.to_string(),
                    run_token.as_bytes().as_slice()
                ],
            )
            .unwrap();
        let fence = open_compute_core::WorkflowFence {
            instance_id: identity.instance_id,
            instance_generation: 1,
            run_token,
        };
        let step = WorkflowStepIdentity {
            ordinal: 0,
            name: "preserved".into(),
            name_count: 1,
            config_json: "null".into(),
        };
        let WorkflowStepGrant::Run { step_token } = store
            .claim_workflow_step(&fence, &step, 201, &limits)
            .unwrap()
        else {
            panic!("run grant")
        };
        match state {
            "pending" => {
                store.recover_workflows(300, &limits, 100).unwrap();
            }
            "complete" => {
                store
                    .complete_workflow_step(
                        &fence,
                        0,
                        &step_token,
                        r#"{"result":42}"#,
                        202,
                        &limits,
                    )
                    .unwrap();
                store
                    .finish_workflow(
                        &fence,
                        &WorkflowCompletion::Complete {
                            output_json: "42".into(),
                            final_ordinal: 1,
                        },
                        203,
                        &limits,
                    )
                    .unwrap();
            }
            "errored" => {
                store
                    .fail_workflow_step(
                        &fence,
                        0,
                        &step_token,
                        ErrorCode::WorkflowExecutionFailed,
                        202,
                        &limits,
                    )
                    .unwrap();
                store
                    .finish_workflow(
                        &fence,
                        &WorkflowCompletion::Errored {
                            code: ErrorCode::WorkflowExecutionFailed,
                        },
                        203,
                        &limits,
                    )
                    .unwrap();
            }
            _ => {}
        }
    }
    for id in &ids {
        store.verify_workflow_history(*id).unwrap();
    }
    (temp, store, ids)
}

fn legacy_bytes(store: &SchedulerStore) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = store.lock().unwrap();
    let instance = "id,account_id,definition_id,definition_name,external_instance_id,version_id,worker_id,deployment_id,worker_code_sha256,loader_schema_version,capability_version,descriptor_sha256,class_name,creation_nonce,instance_generation,state,input_json,output_json,error_json,error_code,next_run_at_ms,run_token,run_claimed_at_ms,run_lease_until_ms,completed_step_count,state_bytes,created_at_ms,updated_at_ms,terminal_at_ms";
    let step = "instance_id,instance_generation,ordinal,name,name_count,kind,config_json,descriptor_sha256,state,attempt,run_token,step_token,output_json,error_json,error_code,started_at_ms,updated_at_ms,completed_at_ms";
    [("workflow_instances", instance), ("workflow_steps", step)]
        .into_iter()
        .map(|(table, cols)| {
            let mut statement = conn
                .prepare(&format!("SELECT {cols} FROM {table} ORDER BY 1,2,3"))
                .unwrap();
            let columns = statement.column_count();
            statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| row.get(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
}

#[test]
fn workflow_v5_rebuild_preserves_claims_results_and_accounting_at_all_fault_boundaries() {
    for fault in [
        SchedulerMigrationFault::BeforeExecution,
        SchedulerMigrationFault::AfterWorkflowRebuild,
        SchedulerMigrationFault::BeforeMigrationRow,
        SchedulerMigrationFault::AfterCommit,
    ] {
        let (temp, store, ids) = legacy_workflows();
        let before = legacy_bytes(&store);
        assert!(store.migrate(1000, Some(fault)).is_err());
        assert_eq!(legacy_bytes(&store), before);
        {
            let conn = store.lock().unwrap();
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                if fault == SchedulerMigrationFault::AfterCommit {
                    6
                } else {
                    5
                }
            );
            assert_eq!(
                conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_temp_master WHERE name LIKE 'saved_workflow_%'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
        }
        drop(store);
        let store =
            SchedulerStore::open(&temp.path().join("scheduler.sqlite"), 1000, 1001).unwrap();
        assert_eq!(legacy_bytes(&store), before);
        for id in ids {
            store.verify_workflow_history(id).unwrap();
        }
        store.quick_check().unwrap();
    }
}

#[test]
fn workflow_upgrade_checks_old_registry_and_history_before_writing() {
    for corruption in [
        "UPDATE scheduler_migrations SET checksum_sha256=zeroblob(32) WHERE version=5;",
        "DROP TRIGGER workflow_step_identity_guard; UPDATE workflow_steps SET descriptor_sha256=zeroblob(32) WHERE state='pending';",
    ] {
        let (_temp, store, _) = legacy_workflows();
        store.lock().unwrap().execute_batch(corruption).unwrap();
        let before = legacy_bytes(&store);
        assert!(store.migrate(1000, None).is_err());
        let conn = store.lock().unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='workflow_events'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        drop(conn);
        assert_eq!(legacy_bytes(&store), before);
    }
}
