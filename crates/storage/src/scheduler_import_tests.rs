use super::*;
use open_compute_core::SystemClock;
use rusqlite::types::Value;
use std::os::unix::fs::PermissionsExt as _;

fn fixture() -> (tempfile::TempDir, ControlDb, InstanceId) {
    let temp = tempfile::tempdir().unwrap();
    let owner = InstanceId::generate();
    let control = ControlDb::open(&temp.path().join("control.sqlite"), 100).unwrap();
    control.migrate(&SystemClock).unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let mut legacy = Connection::open(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    schema_migrations::migrate_to_for_test(&mut legacy, DatabaseKind::Scheduler, 3);
    legacy
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
        .unwrap();
    legacy
        .execute(
            "INSERT INTO scheduler_identity VALUES(1,?1)",
            [owner.as_str()],
        )
        .unwrap();
    legacy
        .execute(
            "INSERT INTO scheduler_meta VALUES(1,?1,1,1)",
            [crate::scheduler::DATA_FORMAT],
        )
        .unwrap();
    legacy.execute_batch(
        "INSERT INTO scheduled_jobs VALUES('alarm','do_alarm','namespace',
           '0000000000000000000000000000000000000000000000000000000000000001',
           1,'retained-alarm-token',100,'version',1,'claimed',0,'retained-claim-token',1000,NULL,1,1);
         INSERT INTO queue_state VALUES('00000000-0000-7000-8000-000000000001',
           1,1,'accepting',0,60,100,100,1000,10000,0,0,1,1);
         INSERT INTO queue_messages(seq,id,queue_id,queue_generation,enqueued_at_ms,
           available_at_ms,expires_at_ms,content_type,body,body_bytes)
           VALUES(42,'00000000-0000-7000-8000-000000000002',
           '00000000-0000-7000-8000-000000000001',1,1,1,60001,'text',X'616263',3);
         UPDATE sqlite_sequence SET seq=99 WHERE name='queue_messages';
         INSERT INTO queue_consumer_state VALUES('00000000-0000-7000-8000-000000000007',
           '00000000-0000-7000-8000-000000000001',1,'00000000-0000-7000-8000-000000000005',
           '00000000-0000-7000-8000-000000000004',1,NULL,'accepting',10,1000,3,0,1,NULL,NULL,zeroblob(32),1);
         INSERT INTO queue_delivery_batches VALUES('00000000-0000-7000-8000-000000000008',
           '00000000-0000-7000-8000-000000000001','00000000-0000-7000-8000-000000000007',
           1,'00000000-0000-7000-8000-000000000005',1,NULL,zeroblob(32),'claimed',1,1000,1,1);
         UPDATE queue_messages SET state='claimed',claim_token=zeroblob(32),claim_until_ms=1000,
           claimed_at_ms=1,claim_batch_id='00000000-0000-7000-8000-000000000008',
           consumer_id='00000000-0000-7000-8000-000000000007',consumer_generation=1;
         INSERT INTO cron_schedules VALUES('00000000-0000-7000-8000-000000000003',
           '00000000-0000-7000-8000-000000000004','00000000-0000-7000-8000-000000000005',
           1,1,'* * * * *',zeroblob(32),1,'accepting',60000,1);
         INSERT INTO cron_runs(id,activation_id,activation_generation,scheduled_at_ms,
           version_id,execution_generation,expression,state,next_attempt_at_ms,created_at_ms)
           VALUES('00000000-0000-7000-8000-000000000006','00000000-0000-7000-8000-000000000003',
           1,0,'00000000-0000-7000-8000-000000000005',1,'* * * * *','ready',1,1);
         UPDATE cron_runs SET state='claimed',attempt=1,next_attempt_at_ms=NULL,
           claim_token=zeroblob(32),claimed_at_ms=1,claim_until_ms=1000,
           first_dispatched_at_ms=1,dispatch_deadline_at_ms=900001;
         INSERT INTO workflow_instances(id,definition_id,definition_name,external_instance_id,
           workflow_version_id,worker_id,worker_version_id,worker_code_sha256,
           loader_schema_version,capability_version,descriptor_sha256,class_name,creation_nonce,
           creation_operation_id,creation_batch_id,instance_generation,state,input_json,
           next_run_at_ms,state_bytes,created_at_ms,updated_at_ms,success_retention_ms,error_retention_ms)
           VALUES('flow','definition','flow','external','workflow-version','worker','version',zeroblob(32),
           1,1,zeroblob(32),'Workflow',zeroblob(32),'create','create',1,'queued',X'7B7D',1,278,1,1,3600000,3600000);
         UPDATE workflow_instances SET state='running',next_run_at_ms=NULL,run_token=zeroblob(32),
           run_claimed_at_ms=1,run_lease_until_ms=1000,has_activated=1;
         INSERT INTO workflow_steps(instance_id,instance_generation,ordinal,name,name_count,kind,
           config_json,descriptor_sha256,state,attempt,started_at_ms,updated_at_ms,config_sha256)
           VALUES('flow',1,0,'step',1,'do',CAST('{\"rollbackStep\":false}' AS BLOB),zeroblob(32),
           'pending',0,1,1,zeroblob(32));"
    ).unwrap();
    legacy
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .unwrap();
    drop(legacy);
    (temp, control, owner)
}

fn rows(connection: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn assert_imported(control: &ControlDb, source: &Connection) {
    control
        .with_read(|connection| {
            for table in TABLES {
                if matches!(*table,"queue_consumer_state"|"cron_schedules") {
                    assert!(rows(connection,table).is_empty(),"unowned legacy target {table}");
                    continue;
                }
                if matches!(*table, "scheduled_jobs"|"queue_messages"|"queue_delivery_batches"|"cron_runs") {
                    match *table {
                        "scheduled_jobs" => assert_eq!(connection.query_row("SELECT state FROM scheduled_jobs", [], |row|row.get::<_,String>(0)).unwrap(), "scheduled"),
                        "queue_messages" => assert_eq!(connection.query_row("SELECT state,attempts,body FROM queue_messages", [], |row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,Vec<u8>>(2)?))).unwrap(), ("ready".into(),1,b"abc".to_vec())),
                        "queue_delivery_batches" => assert!(rows(connection,table).is_empty()),
                        "cron_runs" => assert_eq!(connection.query_row("SELECT state,attempt,error_code FROM cron_runs", [], |row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?))).unwrap(), ("failed".into(),1,"CRON_DISPATCH_DEADLINE".into())),
                        _ => unreachable!(),
                    }
                    continue;
                }
                let columns = {
                    let mut statement=connection.prepare(&format!("PRAGMA table_info({table})")).unwrap();
                    statement.query_map([],|row|row.get::<_,String>(1)).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
                };
                let select=columns.iter().map(|column|match (*table,column.as_str()) {
                    ("queue_consumer_state","state") => "CASE WHEN state='paused' THEN 'paused' ELSE 'active' END".to_owned(),
                    ("cron_runs","worker_id") => "(SELECT worker_id FROM cron_schedules s WHERE s.activation_id=source.activation_id)".to_owned(),
                    _=>format!("source.{column}"),
                }).collect::<Vec<_>>().join(",");
                let mut statement=source.prepare(&format!("SELECT {select} FROM {table} source ORDER BY 1")).unwrap();
                let expected=statement.query_map([], |row|(0..columns.len()).map(|i|row.get::<_,Value>(i)).collect::<Result<Vec<_>,_>>()).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
                assert_eq!(rows(connection,table),expected,"{table}");
            }
            assert_eq!(
                rows(connection, "sqlite_sequence"),
                rows(source, "sqlite_sequence")
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        control.query_meta(MARKER).unwrap().as_deref(),
        Some("complete")
    );
    control
        .with_connection_mut(|connection| {
            schema_migrations::inspect(connection, DatabaseKind::Control)
        })
        .unwrap();
}

#[test]
fn imports_v3_rows_without_changing_claims_counters_steps_or_schema() {
    let (temp, control, owner) = fixture();
    import(&control, temp.path(), 100, owner).unwrap();
    assert!(!temp.path().join("scheduler.sqlite").exists());
    assert!(temp.path().join("scheduler.sqlite.imported-wal").exists());
    assert!(temp.path().join("scheduler.sqlite.imported-shm").exists());
    let source = Connection::open(temp.path().join("scheduler.sqlite.imported")).unwrap();
    assert_imported(&control, &source);
    // A second startup does not replay the import over newer scheduler writes.
    control
        .with_immediate(|tx| {
            tx.execute("UPDATE scheduled_jobs SET due_at_ms=200", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    import(&control, temp.path(), 100, owner).unwrap();
    control
        .with_read(|connection| {
            assert_eq!(
                connection
                    .query_row("SELECT due_at_ms FROM scheduled_jobs", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                200
            );
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM queue_messages", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn crash_before_commit_rolls_back_rows_triggers_and_marker_then_reruns() {
    let (temp, control, owner) = fixture();
    assert!(
        import_inner(
            &control,
            temp.path(),
            100,
            owner,
            Some(ImportFault::BeforeCommit)
        )
        .is_err()
    );
    assert!(control.query_meta(MARKER).unwrap().is_none());
    control
        .with_read(|connection| {
            for table in TABLES {
                assert!(rows(connection, table).is_empty(), "{table}");
            }
            Ok(())
        })
        .unwrap();
    control
        .with_connection_mut(|connection| {
            schema_migrations::inspect(connection, DatabaseKind::Control)
        })
        .unwrap();
    assert!(temp.path().join("scheduler.sqlite").exists());
    import(&control, temp.path(), 100, owner).unwrap();
    assert_imported(
        &control,
        &Connection::open(temp.path().join("scheduler.sqlite.imported")).unwrap(),
    );
}

#[test]
fn crash_after_commit_finishes_rename_without_importing_twice() {
    let (temp, control, owner) = fixture();
    assert!(
        import_inner(
            &control,
            temp.path(),
            100,
            owner,
            Some(ImportFault::AfterCommit)
        )
        .is_err()
    );
    assert_eq!(
        control.query_meta(MARKER).unwrap().as_deref(),
        Some("complete")
    );
    assert!(temp.path().join("scheduler.sqlite").exists());
    import(&control, temp.path(), 100, owner).unwrap();
    assert_imported(
        &control,
        &Connection::open(temp.path().join("scheduler.sqlite.imported")).unwrap(),
    );
}

#[test]
fn populated_destination_without_marker_and_wrong_source_owner_fail_closed() {
    let (temp, control, owner) = fixture();
    control
        .with_immediate(|tx| {
            tx.execute("INSERT INTO scheduler_meta VALUES(1,'occupied',1,1)", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    let error = import(&control, temp.path(), 100, owner).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("already contains scheduler rows")
    );
    assert!(control.query_meta(MARKER).unwrap().is_none());
    assert!(temp.path().join("scheduler.sqlite").exists());
    let error = import(&control, temp.path(), 100, InstanceId::generate()).unwrap_err();
    assert!(error.to_string().contains("instance identity"));
}

#[test]
fn fresh_start_creates_one_database() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = open_compute_core::DataConfig {
        path: root.clone(),
        master_key_file: temp.path().join("master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 100,
        free_space_soft_bytes: 2,
        free_space_hard_bytes: 1,
    };
    let storage = crate::PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let owner = storage.identity().instance_id;
    drop(
        crate::scheduler::SchedulerStore::open(
            &storage.data_dir().ensure_scheduler_db().unwrap(),
            100,
            1,
            owner,
        )
        .unwrap(),
    );
    assert!(!root.join("scheduler.sqlite").exists());
    drop(storage);
    let storage = crate::PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    assert_eq!(
        storage.data_dir().scheduler_db_path(),
        storage.data_dir().control_db_path()
    );
    assert!(!root.join("scheduler.sqlite").exists());
}

#[test]
fn legacy_snapshot_restore_keeps_scheduler_file_for_startup_import() {
    use open_compute_core::{
        ObjectStorageKind, PlatformReleaseIdentityV1, PlatformSnapshotManifestV1, SnapshotFileRole,
        SnapshotFileV1, SnapshotTotalsV1,
    };
    use sha2::{Digest as _, Sha256};
    let (temp, _, _) = fixture();
    let config = open_compute_core::DataConfig {
        path: std::fs::canonicalize(temp.path()).unwrap().join("restored"),
        master_key_file: temp.path().join("master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 100,
        free_space_soft_bytes: 2,
        free_space_hard_bytes: 1,
    };
    let key = crate::master_key::resolve(&config).unwrap();
    let legacy_control = temp.path().join("legacy-control.sqlite");
    let mut connection = Connection::open(&legacy_control).unwrap();
    schema_migrations::migrate_to_for_test(&mut connection, DatabaseKind::Control, 20);
    drop(connection);
    let control = ControlDb::open(&legacy_control, 100).unwrap();
    let identity = crate::identity::bootstrap(&control, &SystemClock, key.fingerprint()).unwrap();
    drop(control);
    let legacy = temp.path().join("scheduler.sqlite");
    let connection = Connection::open(&legacy).unwrap();
    connection
        .execute(
            "UPDATE scheduler_identity SET instance_id=?1",
            [identity.instance_id.as_str()],
        )
        .unwrap();
    drop(connection);
    let restore = crate::platform_restore::RestoreTarget::acquire(&config.path).unwrap();
    let mut files = Vec::new();
    for (source, name, role, id) in [
        (
            &legacy_control,
            "control.sqlite",
            SnapshotFileRole::ControlSqlite,
            "control",
        ),
        (
            &legacy,
            "scheduler.sqlite",
            SnapshotFileRole::SchedulerSqlite,
            "scheduler",
        ),
    ] {
        let bytes = std::fs::read(source).unwrap();
        let destination = restore.destination_for(name).unwrap();
        std::fs::write(&destination, &bytes).unwrap();
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600)).unwrap();
        files.push(SnapshotFileV1 {
            role,
            logical_id: id.into(),
            restore_path: name.into(),
            object_key: format!("system/snapshots/v1/legacy/objects/{id}.bin"),
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
            mode: 0o600,
        });
    }
    let manifest = PlatformSnapshotManifestV1 {
        schema_version: 1,
        snapshot_id: uuid::Uuid::now_v7().to_string(),
        instance_id: identity.instance_id.to_string(),
        label: "legacy".into(),
        created_at_ms: 1,
        source_release: PlatformReleaseIdentityV1 {
            schema_version: 1,
            platform_version: "0.3.0".into(),
            git_revision: "test".into(),
            rust_msrv: "1.98.0".into(),
            workerd_version: "workerd test".into(),
            workerd_lock_sha256: "a".repeat(64),
            runtime_assets_sha256: "b".repeat(64),
            dashboard_assets_sha256: "c".repeat(64),
            facade_capability_version: 1,
            snapshot_format_version: 1,
        },
        master_key_fingerprint: key.fingerprint().into(),
        object_backend_kind: ObjectStorageKind::Local,
        object_authority_fingerprint: "d".repeat(64),
        r2_prefix_fingerprint: "e".repeat(64),
        config_policy_sha256: "f".repeat(64),
        excluded_local_state: vec![],
        immutable_references: vec![],
        totals: SnapshotTotalsV1 {
            files: 2,
            bytes: files.iter().map(|file| file.size).sum(),
        },
        files,
        manifest_mac: "0".repeat(64),
    };
    restore
        .validate_and_publish(&manifest, key.fingerprint(), 100, b"{}")
        .unwrap();
    assert!(config.path.join("scheduler.sqlite").exists());
    assert!(!config.path.join("scheduler.sqlite.imported").exists());
    let storage = crate::PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    assert_eq!(
        storage.db().query_meta(MARKER).unwrap().as_deref(),
        Some("complete")
    );
    assert_imported(
        storage.db(),
        &Connection::open(config.path.join("scheduler.sqlite.imported")).unwrap(),
    );
}

#[test]
fn v22_keeps_in_flight_queue_and_cron_rows_when_removing_old_targets() {
    let (temp, _control, _owner) = fixture();
    let legacy = Connection::open(temp.path().join("scheduler.sqlite")).unwrap();
    let mut upgraded = Connection::open_in_memory().unwrap();
    schema_migrations::migrate_to_for_test(&mut upgraded, DatabaseKind::Control, 21);
    let triggers = {
        let mut statement = upgraded
            .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    for (name, _) in &triggers {
        upgraded
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
    }
    for table in TABLES {
        let values = rows(&legacy, table);
        for value in values {
            let placeholders = vec!["?"; value.len()].join(",");
            upgraded
                .execute(
                    &format!("INSERT INTO {table} VALUES({placeholders})"),
                    rusqlite::params_from_iter(value),
                )
                .unwrap();
        }
    }
    for (_, sql) in triggers {
        upgraded.execute_batch(&sql).unwrap();
    }
    let messages = rows(&upgraded, "queue_messages");
    let batches = rows(&upgraded, "queue_delivery_batches");
    let runs = rows(&upgraded, "cron_runs");
    schema_migrations::migrate_to_for_test(&mut upgraded, DatabaseKind::Control, 22);
    assert_eq!(rows(&upgraded, "queue_messages"), messages);
    assert_eq!(rows(&upgraded, "queue_delivery_batches"), batches);
    let mapped = rows(&upgraded, "cron_runs");
    assert_eq!(mapped.len(), runs.len());
    // The redundant generation column becomes the pinned Worker identity; all other data survives.
    for (after, before) in mapped.iter().zip(&runs) {
        assert_eq!(&after[..2], &before[..2]);
        assert_eq!(&after[3..], &before[3..]);
        assert_eq!(
            after[2],
            Value::Text("00000000-0000-7000-8000-000000000004".into())
        );
    }
    assert!(rows(&upgraded, "queue_consumer_state").is_empty());
    assert!(rows(&upgraded, "cron_schedules").is_empty());
}
