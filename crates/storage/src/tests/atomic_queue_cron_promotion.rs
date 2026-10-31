use super::*;
use crate::cron::{NewCronConfig, NewCronDeclaration};
use crate::scheduler::{
    CronCompletion, CronCompletionResult, QueueCompletionAction, QueueCompletionDecision,
    QueueContentType, QueueEnqueueRequest, QueueMessageInput, QueueProjection, SchedulerStore,
};
use crate::worker_repository::{DeploymentSource, NewVersionProducts};
use open_compute_core::{CronActivationId, CronSchedule, QueueId, RequestId, StartupId};
use rusqlite::types::Value;
use sha2::{Digest as _, Sha256};
use std::time::Instant;

fn ready_product_version(
    storage: &PlatformStorage,
    worker: WorkerId,
    queue: QueueId,
    tag: u8,
) -> VersionId {
    let repo = WorkerRepository::new(storage.db());
    let id = VersionId::generate();
    let expression = if tag == 1 { "* * * * *" } else { "*/2 * * * *" };
    let expression_sha256 = Sha256::digest(
        CronSchedule::parse(expression)
            .unwrap()
            .normalized()
            .as_bytes(),
    )
    .into();
    let descriptor = serde_json::json!({"capabilityVersion":1,"declarations":[{
        "expression":expression,"expressionSha256":hex::encode(expression_sha256),
        "parserVersion":1,"scheduledHandler":true,"workflowBindings":[]} ]});
    let cron = NewCronConfig {
        capability_version: 1,
        descriptor_sha256: Sha256::digest(serde_json::to_vec(&descriptor).unwrap()).into(),
        declarations: vec![NewCronDeclaration {
            id: CronActivationId::generate(),
            expression: expression.into(),
            expression_sha256,
            parser_version: 1,
            scheduled_handler: true,
            workflow_bindings: Vec::new(),
        }],
    };
    let consumers = [NewQueueConsumerDeclaration {
        id: QueueConsumerId::generate(),
        queue_id: queue,
        queue_lifecycle_generation: 1,
        entrypoint: None,
        config: QueueConsumerConfig {
            max_batch_timeout_seconds: 0,
            max_retries: u32::from(tag),
            ..Default::default()
        },
        dead_letter_queue: None,
        capability_version: 1,
        descriptor_sha256: [tag; 32],
    }];
    repo.insert_staging_version(
        &NewVersion {
            id,
            instance_id: storage.identity().instance_id,
            worker_id: worker,
            content_kind: crate::worker_repository::VersionContentKind::Worker,
            artifact_sha256: Some([tag; 32]),
            artifact_size: Some(100),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".into()),
            worker_code_sha256: [tag; 32],
            compatibility_date: "2026-09-08".into(),
            compatibility_flags: Vec::new(),
            resource_limits: EffectiveResourceLimits::standard_defaults(),
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: RequestId::generate(),
            now_ms: i64::from(tag),
        },
        &NewVersionProducts {
            queue_consumers: &consumers,
            cron: Some(&cron),
            ..Default::default()
        },
        10,
    )
    .unwrap();
    repo.begin_validation(id).unwrap();
    repo.mark_ready(id, 10).unwrap();
    id
}

fn fixture() -> (
    TempDir,
    PlatformStorage,
    SchedulerStore,
    WorkerId,
    QueueId,
    VersionId,
    VersionId,
) {
    let (dir, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().instance_id;
    let store = SchedulerStore::open(&root.join("control.sqlite"), 100, 1, account).unwrap();
    let queue = QueueId::generate();
    let config = crate::queues::QueueConfig::default();
    let queues = crate::queues::QueueRepository::new(storage.db());
    queues
        .insert_creating(account, queue, "promotion", config, 1)
        .unwrap();
    store
        .create_queue_projection(&QueueProjection {
            queue_id: queue,
            instance_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    queues.mark_ready(account, queue, 2).unwrap();
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(account, "promotion", RequestId::generate(), 1, 10)
        .unwrap()
        .0
        .id;
    let first = ready_product_version(&storage, worker, queue, 1);
    let second = ready_product_version(&storage, worker, queue, 2);
    workers
        .promote(account, worker, first, None, RequestId::generate(), 1_000)
        .unwrap();
    (dir, storage, store, worker, queue, first, second)
}

fn snapshot(storage: &PlatformStorage) -> Vec<Vec<Vec<Value>>> {
    storage
        .db()
        .with_read(|connection| {
            [
                "workers",
                "worker_deployments",
                "deployment_runtime_assessments",
                "queue_consumers",
                "queue_consumer_state",
                "cron_activations",
                "cron_schedules",
                "version_referrers",
            ]
            .iter()
            .map(|table| {
                let mut statement = connection
                    .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
                    .unwrap();
                let columns = statement.column_count();
                Ok(statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|i| row.get(i))
                            .collect::<rusqlite::Result<Vec<Value>>>()
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap())
            })
            .collect()
        })
        .unwrap()
}

#[test]
fn promotion_failure_rolls_back_deployment_consumers_and_schedules() {
    let (_dir, storage, _store, worker, _queue, first, second) = fixture();
    let before = snapshot(&storage);
    storage.db().with_immediate(|tx| {
        tx.execute_batch("CREATE TRIGGER reject_schedule BEFORE INSERT ON cron_schedules BEGIN SELECT RAISE(ABORT,'injected schedule failure'); END;").unwrap();Ok(())
    }).unwrap();
    let repo = WorkerRepository::new(storage.db());
    assert!(
        repo.promote(
            storage.identity().instance_id,
            worker,
            second,
            Some(first),
            RequestId::generate(),
            2_000
        )
        .is_err()
    );
    assert_eq!(snapshot(&storage), before);
}

#[test]
fn promotion_does_not_wait_for_queue_and_cron_and_old_claims_finish() {
    let (_dir, storage, store, worker, queue, first, second) = fixture();
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: queue,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"message".to_vec(),
                    delay_seconds: None,
                }],
            },
            60_000,
        )
        .unwrap();
    let batch = store
        .claim_queue_batches(60_000, 1, None)
        .unwrap()
        .remove(0);
    store.project_due_cron_slots(60_000, 300_000, 1).unwrap();
    store
        .complete_queue_batch(
            &batch,
            &[QueueCompletionDecision {
                message_id: batch.messages[0].id,
                action: QueueCompletionAction::Retry { delay_seconds: 0 },
            }],
            60_000,
        )
        .unwrap();
    let batch = store
        .claim_queue_batches(60_000, 1, None)
        .unwrap()
        .remove(0);
    let run = store.claim_cron_runs(60_000, 3, 1).unwrap().remove(0);
    assert_eq!((batch.version_id, run.version_id), (first, first));
    let repo = WorkerRepository::new(storage.db());
    let started = Instant::now();
    repo.promote(
        storage.identity().instance_id,
        worker,
        second,
        Some(first),
        RequestId::generate(),
        60_001,
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(batch.max_retries, 1);
    let completion = store
        .complete_queue_batch(
            &batch,
            &[QueueCompletionDecision {
                message_id: batch.messages[0].id,
                action: QueueCompletionAction::Retry { delay_seconds: 0 },
            }],
            60_002,
        )
        .unwrap();
    assert_eq!(completion.retried, 0);
    assert_eq!(completion.discarded, 1);
    assert!(!completion.stale);
    assert_eq!(
        store
            .complete_cron_run(&run, CronCompletion::Success, 60_002, 3)
            .unwrap(),
        CronCompletionResult::Terminal
    );
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: queue,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"new version".to_vec(),
                    delay_seconds: None,
                }],
            },
            60_003,
        )
        .unwrap();
    let new_batch = store
        .claim_queue_batches(60_003, 1, None)
        .unwrap()
        .remove(0);
    assert_eq!(new_batch.version_id, second);
    assert_eq!(new_batch.max_retries, 2);
}

#[test]
fn rollback_and_quarantine_replace_products_on_the_deployment_transaction() {
    let (_dir, storage, _store, worker, queue, first, second) = fixture();
    let account = storage.identity().instance_id;
    let repo = WorkerRepository::new(storage.db());
    let consumers = QueueConsumerRepository::new(storage.db());
    let original = consumers.live_for_queue(queue).unwrap().unwrap();
    consumers
        .pause(
            original.id,
            original.consumer_generation,
            RequestId::generate(),
            1_001,
        )
        .unwrap();
    let (_, _deployment) = repo
        .create_deployment_checked(
            account,
            worker,
            second,
            Some(first),
            None,
            DeploymentSource::VersionsApi,
            &BTreeMap::new(),
            None,
            RequestId::generate(),
            2_000,
            StartupId::generate(),
        )
        .unwrap();
    let (_, rollback) = repo
        .create_deployment_checked(
            account,
            worker,
            first,
            Some(second),
            None,
            DeploymentSource::Rollback,
            &BTreeMap::new(),
            None,
            RequestId::generate(),
            2_001,
            StartupId::generate(),
        )
        .unwrap();
    assert_eq!(rollback.source, DeploymentSource::Rollback);
    assert_eq!(
        consumers.live_for_queue(queue).unwrap().unwrap().version_id,
        first
    );
    assert_eq!(
        crate::cron::CronRepository::new(storage.db())
            .live_for_worker(worker)
            .unwrap()[0]
            .version_id,
        first
    );
    let (_, deployment) = repo
        .create_deployment_checked(
            account,
            worker,
            second,
            Some(first),
            None,
            DeploymentSource::VersionsApi,
            &BTreeMap::new(),
            None,
            RequestId::generate(),
            3_000,
            StartupId::generate(),
        )
        .unwrap();
    assert!(
        repo.quarantine_active_deployment(
            deployment.id,
            "RUNTIME_GENERATION_CHANGED",
            RequestId::generate(),
            3_001
        )
        .unwrap()
    );
    let restored = consumers.live_for_queue(queue).unwrap().unwrap();
    assert_eq!(restored.version_id, first);
    assert_eq!(
        restored.state,
        crate::queue_consumers::QueueConsumerState::Paused
    );
    assert_eq!(
        crate::cron::CronRepository::new(storage.db())
            .live_for_worker(worker)
            .unwrap()[0]
            .version_id,
        first
    );
    assert_eq!(
        repo.get_worker(account, worker).unwrap().active_version_id,
        Some(first)
    );
}

#[test]
fn v22_preserves_api_consumer_overrides_and_pause() {
    let (_dir, storage, _store, worker, queue, version, _) = fixture();
    let consumers = QueueConsumerRepository::new(storage.db());
    let declaration = consumers
        .create_api_declaration(
            version,
            &NewQueueConsumerDeclaration {
                id: QueueConsumerId::generate(),
                queue_id: queue,
                queue_lifecycle_generation: 1,
                entrypoint: None,
                config: QueueConsumerConfig {
                    max_batch_size: 22,
                    ..Default::default()
                },
                dead_letter_queue: None,
                capability_version: 1,
                descriptor_sha256: [8; 32],
            },
            2_000,
        )
        .unwrap();
    let attachment = consumers
        .upsert_attachment(storage.identity().instance_id, worker, &declaration, 2_001)
        .unwrap();
    consumers
        .pause(
            attachment.id,
            attachment.consumer_generation,
            RequestId::generate(),
            2_002,
        )
        .unwrap();
    let mut released = Connection::open_in_memory().unwrap();
    crate::schema_migrations::migrate_to_for_test(
        &mut released,
        crate::schema_migrations::DatabaseKind::Control,
        21,
    );
    released.pragma_update(None, "foreign_keys", false).unwrap();
    let triggers = {
        let mut statement = released
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
        released
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
    }
    for table in [
        "instance_identity",
        "queues",
        "workers",
        "worker_versions",
        "worker_deployments",
        "version_queue_consumers",
        "queue_consumers",
        "queue_state",
        "queue_consumer_state",
        "version_cron_configs",
        "version_cron_declarations",
        "cron_activations",
        "cron_schedules",
    ] {
        let columns = {
            let mut statement = released
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        storage
            .db()
            .with_read(|source| {
                let mut statement = source.prepare(&format!("SELECT * FROM {table}")).unwrap();
                let source_columns = statement
                    .column_names()
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect::<Vec<_>>();
                let rows = statement
                    .query_map([], |row| {
                        columns
                            .iter()
                            .map(|column| {
                                if table == "queue_consumer_state" && column == "state" {
                                    return Ok(Value::Text("paused".into()));
                                }
                                if let Some(index) =
                                    source_columns.iter().position(|name| name == column)
                                {
                                    return row.get::<_, Value>(index);
                                }
                                Ok(match column.as_str() {
                                    "activation_generation" => Value::Integer(1),
                                    "state" => Value::Text(
                                        if table == "cron_schedules" {
                                            "accepting"
                                        } else {
                                            "active"
                                        }
                                        .into(),
                                    ),
                                    "availability" => Value::Text("healthy".into()),
                                    _ => Value::Null,
                                })
                            })
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap();
                for row in rows {
                    let row = row.unwrap();
                    let placeholders = vec!["?"; columns.len()].join(",");
                    released
                        .execute(
                            &format!("INSERT INTO {table} VALUES({placeholders})"),
                            rusqlite::params_from_iter(row),
                        )
                        .unwrap();
                }
                Ok(())
            })
            .unwrap();
    }
    for (_, sql) in triggers {
        released.execute_batch(&sql).unwrap();
    }
    released.pragma_update(None, "foreign_keys", true).unwrap();
    crate::schema_migrations::migrate(
        &mut released,
        crate::schema_migrations::DatabaseKind::Control,
    )
    .unwrap();
    let retained:(QueueConsumerId,String,i64)=released.query_row(
        "SELECT c.declaration_id,c.state,s.max_batch_size FROM queue_consumers c JOIN queue_consumer_state s ON s.consumer_id=c.id",
        [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(retained, (declaration.id, "paused".into(), 22));
    assert_eq!(
        released
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn v23_recovers_claimed_rows_and_preserves_queue_and_cron_accounting() {
    let (_dir, storage, store, _worker, queue, _first, _second) = fixture();
    let now = open_compute_core::wall_time_ms();
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: queue,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"interrupted".to_vec(),
                    delay_seconds: None,
                }],
            },
            now,
        )
        .unwrap();
    store.claim_queue_batches(now, 1, None).unwrap();
    store.project_due_cron_slots(now, 300_000, 1).unwrap();
    store.claim_cron_runs(now, 3, 1).unwrap();
    let mut released = Connection::open_in_memory().unwrap();
    crate::schema_migrations::migrate_to_for_test(
        &mut released,
        crate::schema_migrations::DatabaseKind::Control,
        22,
    );
    let triggers = {
        let mut statement = released
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
        released
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
    }
    for table in [
        "queue_state",
        "queue_consumer_state",
        "queue_messages",
        "queue_delivery_batches",
        "cron_schedules",
        "cron_runs",
    ] {
        let columns = {
            let mut statement = released
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        storage
            .db()
            .with_read(|source| {
                let mut statement = source.prepare(&format!("SELECT * FROM {table}")).unwrap();
                let source_columns = statement
                    .column_names()
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect::<Vec<_>>();
                let rows = statement
                    .query_map([], |row| {
                        columns
                            .iter()
                            .map(|column| {
                                if let Some(index) =
                                    source_columns.iter().position(|name| name == column)
                                {
                                    return row.get::<_, Value>(index);
                                }
                                Ok(match column.as_str() {
                                    "claim_token" => Value::Blob(vec![7; 32]),
                                    "claim_until_ms" => Value::Integer(now + 60_000),
                                    "claimed_at_ms" => Value::Integer(now),
                                    _ => panic!("unexpected removed column {column}"),
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap();
                for row in rows {
                    let placeholders = vec!["?"; columns.len()].join(",");
                    released
                        .execute(
                            &format!("INSERT INTO {table} VALUES({placeholders})"),
                            rusqlite::params_from_iter(row.unwrap()),
                        )
                        .unwrap();
                }
                Ok(())
            })
            .unwrap();
    }
    released
        .execute(
            "UPDATE sqlite_sequence SET seq=99 WHERE name='queue_messages'",
            [],
        )
        .unwrap();
    // An exhausted interrupted delivery is terminal even when its deadline remains live.
    released
        .execute_batch("UPDATE cron_runs SET attempt=4;")
        .unwrap();
    for (_, sql) in triggers {
        released.execute_batch(&sql).unwrap();
    }
    crate::schema_migrations::migrate(
        &mut released,
        crate::schema_migrations::DatabaseKind::Control,
    )
    .unwrap();
    assert_eq!(
        released
            .query_row(
                "SELECT state,attempts,body FROM queue_messages",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?
                ))
            )
            .unwrap(),
        ("ready".into(), 1, b"interrupted".to_vec())
    );
    assert_eq!(
        released
            .query_row("SELECT COUNT(*) FROM queue_delivery_batches", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        released
            .query_row(
                "SELECT state,attempt,error_code,last_unknown_reason FROM cron_runs",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?
                ))
            )
            .unwrap(),
        (
            "failed".into(),
            4,
            "CRON_RETRY_EXHAUSTED".into(),
            "transport-timeout".into()
        )
    );
    assert_eq!(
        released
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='queue_messages'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        99
    );
    for table in [
        "scheduled_jobs",
        "queue_messages",
        "queue_delivery_batches",
        "cron_runs",
    ] {
        let mut statement = released
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            !columns
                .iter()
                .any(|name| name == "claim_token" || name == "claim_until_ms")
        );
    }
    assert_eq!(
        released
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}
