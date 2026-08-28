use super::*;

#[path = "scheduler/workflow/migration_tests.rs"]
mod workflow_migration;
use open_compute_core::{
    AccountId, CronActivationId, QueueBatchId, QueueConsumerId, QueueId, QueueMessageId, WorkerId,
};

fn object(namespace: ResourceId, byte: u8) -> DurableObjectId {
    let mut bytes = [byte; open_compute_core::DURABLE_OBJECT_ID_BYTES];
    bytes[..open_compute_core::DURABLE_OBJECT_NAMESPACE_PREFIX_BYTES].copy_from_slice(
        &open_compute_core::durable_object_namespace_prefix(namespace),
    );
    DurableObjectId::for_namespace(bytes, namespace).unwrap()
}

fn projection(
    namespace: ResourceId,
    object_id: DurableObjectId,
    token: &str,
    due_at_ms: i64,
) -> AlarmProjection {
    AlarmProjection {
        namespace_resource_id: namespace,
        object_id,
        object_generation: 1,
        row_token: token.to_owned(),
        due_at_ms,
        target_deployment_id: DeploymentId::generate(),
        execution_generation: 3,
        retry_count: 0,
    }
}

fn open_store(temp: &tempfile::TempDir, now_ms: i64) -> SchedulerStore {
    let path = temp.path().join("scheduler.sqlite");
    if !path.exists() {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
    }
    SchedulerStore::open(&path, 100, now_ms).unwrap()
}

fn create_scheduler_fixture_at_version(path: &std::path::Path, version: usize) -> Connection {
    let mut connection = Connection::open(path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    for migration in SCHEDULER_MIGRATIONS.iter().take(version) {
        let tx = connection.transaction().unwrap();
        tx.execute_batch(migration.sql).unwrap();
        if migration.version == 1 {
            tx.execute(
                "INSERT INTO scheduler_meta
                 (singleton, schema_version, data_format, created_at_ms, updated_at_ms)
                 VALUES (1, 1, ?1, 1, 1)",
                [DATA_FORMAT],
            )
            .unwrap();
        } else {
            tx.execute(
                "UPDATE scheduler_meta SET schema_version = ?1, updated_at_ms = 1
                 WHERE singleton = 1",
                [migration.version],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO scheduler_migrations
             (version, name, checksum_sha256, applied_at_ms, app_version)
             VALUES (?1, ?2, ?3, 1, ?4)",
            params![
                migration.version,
                migration.name,
                migration.checksum.as_slice(),
                APP_VERSION,
            ],
        )
        .unwrap();
        tx.pragma_update(None, "user_version", migration.version)
            .unwrap();
        tx.commit().unwrap();
    }
    connection
}

#[test]
fn migration_003_upgrades_a_real_v2_backlog_and_preserves_producer_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let queue_id = QueueId::generate();
    let message_id = QueueMessageId::generate();
    let connection = create_scheduler_fixture_at_version(&path, 2);
    connection
        .execute(
            "INSERT INTO queue_state
             (queue_id, account_id, lifecycle_generation, config_generation, state,
              delivery_delay_seconds, retention_seconds, max_message_bytes,
              max_batch_messages, max_batch_bytes, max_backlog_bytes,
              message_count, message_bytes, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 1, 1, 'accepting', 0, 60, 1024, 100, 4096,
                     8192, 0, 0, 100, 100)",
            params![queue_id.to_string(), AccountId::generate().to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO queue_messages
             (id, queue_id, queue_generation, enqueued_at_ms, available_at_ms,
              expires_at_ms, content_type, body, body_bytes)
             VALUES (?1, ?2, 1, 100, 100, 60100, 'text', X'7632', 2)",
            params![message_id.to_string(), queue_id.to_string()],
        )
        .unwrap();
    drop(connection);

    let store = SchedulerStore::open(&path, 100, 200).unwrap();
    let metrics = store.queue_metrics(queue_id, 1, 1).unwrap();
    assert_eq!(metrics.backlog_count, 1);
    assert_eq!(metrics.backlog_bytes, 2);
    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"after-upgrade".to_vec(),
                    delay_seconds: None,
                }],
            },
            200,
        )
        .unwrap();
    assert_eq!(
        store.queue_metrics(queue_id, 1, 1).unwrap().backlog_count,
        2
    );
    drop(store);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT state, attempts, claim_batch_id, consumer_id,
                        consumer_generation FROM queue_messages WHERE id = ?1",
                [message_id.to_string()],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                )),
            )
            .unwrap(),
        ("ready".to_owned(), 0, None, None, None)
    );
}

#[test]
fn migration_004_preserves_a_real_v3_claim_without_mutating_queue_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let queue_id = QueueId::generate();
    let account_id = AccountId::generate();
    let message_id = QueueMessageId::generate();
    let consumer_id = QueueConsumerId::generate();
    let batch_id = QueueBatchId::generate();
    let deployment_id = DeploymentId::generate();
    let worker_id = WorkerId::generate();
    let token = [9_u8; 32];
    let connection = create_scheduler_fixture_at_version(&path, 3);
    connection
        .execute_batch(&format!(
            "INSERT INTO queue_state
             (queue_id, account_id, lifecycle_generation, config_generation, state,
              delivery_delay_seconds, retention_seconds, max_message_bytes,
              max_batch_messages, max_batch_bytes, max_backlog_bytes,
              message_count, message_bytes, created_at_ms, updated_at_ms)
             VALUES ('{queue_id}', '{account_id}', 1, 1, 'accepting', 0, 60, 1024,
                     100, 4096, 8192, 0, 0, 100, 100);
             INSERT INTO queue_messages
             (id, queue_id, queue_generation, enqueued_at_ms, available_at_ms,
              expires_at_ms, content_type, body, body_bytes)
             VALUES ('{message_id}', '{queue_id}', 1, 100, 100, 60100, 'text', X'7633', 2);
             INSERT INTO queue_consumer_state
             (consumer_id, queue_id, consumer_generation, deployment_id, worker_id,
              execution_generation, entrypoint, state, max_batch_size,
              max_batch_timeout_ms, max_retries, retry_delay_seconds,
              max_concurrency, dlq_queue_id, dlq_queue_generation,
              descriptor_sha256, updated_at_ms)
             VALUES ('{consumer_id}', '{queue_id}', 1, '{deployment_id}', '{worker_id}',
                     1, NULL, 'accepting', 10, 5000, 3, 0, 1, NULL, NULL,
                     zeroblob(32), 100);"
        ))
        .unwrap();
    connection
        .execute(
            "INSERT INTO queue_delivery_batches
             (id, queue_id, consumer_id, consumer_generation, deployment_id,
              execution_generation, entrypoint, claim_token, state, claimed_at_ms,
              claim_until_ms, message_count, created_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4, 1, NULL, ?5, 'claimed', 200, 10000, 1, 200)",
            params![
                batch_id.to_string(),
                queue_id.to_string(),
                consumer_id.to_string(),
                deployment_id.to_string(),
                token.as_slice(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE queue_messages SET state = 'claimed', claim_batch_id = ?1,
                    consumer_id = ?2, consumer_generation = 1, claim_token = ?3,
                    claim_until_ms = 10000, claimed_at_ms = 200
             WHERE id = ?4",
            params![
                batch_id.to_string(),
                consumer_id.to_string(),
                token.as_slice(),
                message_id.to_string(),
            ],
        )
        .unwrap();
    drop(connection);

    drop(SchedulerStore::open(&path, 100, 300).unwrap());
    let connection = Connection::open(&path).unwrap();
    let preserved: (String, String, Vec<u8>, i64, i64) = connection
        .query_row(
            "SELECT state, claim_batch_id, claim_token, claim_until_ms, attempts
             FROM queue_messages WHERE id = ?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        preserved,
        (
            "claimed".to_owned(),
            batch_id.to_string(),
            token.to_vec(),
            10_000,
            0,
        )
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM cron_schedules", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn migrates_and_reopens_the_independent_database() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let store = SchedulerStore::open(&path, 100, 10).unwrap();
    assert_eq!(store.summary(10).unwrap(), SchedulerSummary::default());
    drop(store);
    let reopened = SchedulerStore::open(&path, 100, 20).unwrap();
    reopened.quick_check().unwrap();
    drop(reopened);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE scheduler_migrations SET checksum_sha256 = zeroblob(32) WHERE version = 1",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 30).unwrap_err().code(),
        ErrorCode::SchedulerCorrupt
    );
}

#[test]
fn scheduler_registry_is_contiguous_and_future_schema_fails_closed() {
    let registry = scheduler_migration_registry();
    assert_eq!(registry.len(), 8);
    assert_eq!(registry[0].0, 1);
    assert_eq!(registry[0].1, "001_scheduler");
    assert_eq!(registry[1].0, 2);
    assert_eq!(registry[1].1, "002_queue_producer");
    assert_eq!(registry[2].0, 3);
    assert_eq!(registry[2].1, "003_queue_consumer");
    assert_eq!(registry[3].0, 4);
    assert_eq!(registry[3].1, "004_cron");
    assert_eq!(registry[4].0, 5);
    assert_eq!(registry[4].1, "005_workflow_core");
    assert_eq!(registry[5].0, 6);
    assert_eq!(registry[5].1, "006_workflow_durable_waiting");
    assert_eq!(registry[6].0, 7);
    assert_eq!(registry[6].1, "007_workflow_operation_progress");
    assert_eq!(registry[7].0, 8);
    assert_eq!(registry[7].1, "008_workflow_due_admission");

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 10);
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "user_version", current_scheduler_schema_version() + 1)
        .unwrap();
    drop(connection);
    assert_eq!(
        SchedulerStore::open(&path, 100, 20).unwrap_err().code(),
        ErrorCode::SchemaTooNew
    );
}

#[test]
fn reopening_v1_preserves_schema_sql_migration_identity_and_alarm_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 10);
    let namespace = ResourceId::generate();
    let alarm = projection(namespace, object(namespace, 9), "preserve-token-01", 50);
    store.upsert_alarm(&alarm, 10).unwrap();
    drop(store);
    let before = Connection::open(&path).unwrap();
    let schema_before: Vec<(String, String)> = {
        let mut statement = before
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type IN ('table', 'index') AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let row_before: (String, i64, String) = before
        .query_row(
            "SELECT kind, due_at_ms, row_token FROM scheduled_jobs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let migration_before: (String, Vec<u8>) = before
        .query_row(
            "SELECT name, checksum_sha256 FROM scheduler_migrations WHERE version = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    drop(before);

    drop(SchedulerStore::open(&path, 100, 20).unwrap());
    let after = Connection::open(&path).unwrap();
    let schema_after: Vec<(String, String)> = {
        let mut statement = after
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type IN ('table', 'index') AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(schema_after, schema_before);
    assert_eq!(
        after
            .query_row(
                "SELECT kind, due_at_ms, row_token FROM scheduled_jobs",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?
                )),
            )
            .unwrap(),
        row_before
    );
    assert_eq!(
        after
            .query_row(
                "SELECT name, checksum_sha256 FROM scheduler_migrations WHERE version = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .unwrap(),
        migration_before
    );
}

#[test]
fn overwrite_claim_and_conditional_completion_are_token_fenced() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 10);
    let namespace = ResourceId::generate();
    let object_id = object(namespace, 7);
    let old = projection(namespace, object_id, "old-token-0000001", 20);
    store.upsert_alarm(&old, 10).unwrap();
    let [claimed] = store.claim_due(20, 100, 1).unwrap().try_into().unwrap();
    assert_eq!(claimed.row_token, old.row_token);
    assert_eq!(claimed.claim_token.len(), 64);

    let mut current = projection(namespace, object_id, "new-token-0000002", 40);
    current.target_deployment_id = old.target_deployment_id;
    store.upsert_alarm(&current, 21).unwrap();
    assert!(
        !store
            .finish_claim(&claimed, ClaimResult::Delete, 22)
            .unwrap()
    );
    assert!(store.claim_due(39, 100, 1).unwrap().is_empty());
    let [replacement] = store.claim_due(40, 100, 1).unwrap().try_into().unwrap();
    assert_eq!(replacement.row_token, current.row_token);
}

#[test]
fn due_order_is_stable_and_batches_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    for (byte, due) in [(1, 30), (2, 20), (3, 20), (4, 10)] {
        store
            .upsert_alarm(
                &projection(
                    namespace,
                    object(namespace, byte),
                    &format!("token-{byte:016}"),
                    due,
                ),
                1,
            )
            .unwrap();
    }
    let first = store.claim_due(20, 100, 2).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].due_at_ms, 10);
    assert_eq!(first[1].due_at_ms, 20);
    assert!(first[0].id < first[1].id || first[0].due_at_ms < first[1].due_at_ms);
}

#[test]
fn expired_lease_recovers_with_a_new_random_claim_token() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 1), "recover-token-001", 10),
            1,
        )
        .unwrap();
    let [first] = store.claim_due(10, 50, 1).unwrap().try_into().unwrap();
    assert!(store.claim_due(59, 50, 1).unwrap().is_empty());
    let (second, recovered) = store.claim_due_with_recovery(60, 50, 1).unwrap();
    assert_eq!(recovered, 1);
    let [second] = second.try_into().unwrap();
    assert_ne!(first.claim_token, second.claim_token);
    assert!(!store.finish_claim(&first, ClaimResult::Delete, 61).unwrap());
    assert!(
        store
            .finish_claim(&second, ClaimResult::Delete, 61)
            .unwrap()
    );
}

#[test]
fn recovery_is_bounded_and_workload_summary_includes_lease_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    for byte in 1..=3 {
        store
            .upsert_alarm(
                &projection(
                    namespace,
                    object(namespace, byte),
                    &format!("recover-{byte:016}"),
                    10,
                ),
                1,
            )
            .unwrap();
    }
    assert_eq!(store.claim_due(10, 50, 3).unwrap().len(), 3);
    let before = store.workload_summary(20).unwrap();
    assert_eq!(before.claimed, 3);
    assert_eq!(before.next_due_at_ms, Some(60));
    assert_eq!(store.recover_expired(60, 2).unwrap(), 2);
    let after = store.workload_summary(60).unwrap();
    assert_eq!(after.ready, 2);
    assert_eq!(after.claimed, 1);
    assert_eq!(after.expired, 1);
}

#[test]
fn concurrent_claim_transactions_never_duplicate_an_alarm() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(open_store(&temp, 1));
    let namespace = ResourceId::generate();
    for byte in 1..=8 {
        store
            .upsert_alarm(
                &projection(
                    namespace,
                    object(namespace, byte),
                    &format!("concurrent-{byte:016}"),
                    10,
                ),
                1,
            )
            .unwrap();
    }
    let threads = (0..2)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || store.claim_due(10, 100, 8).unwrap())
        })
        .collect::<Vec<_>>();
    let claimed = threads
        .into_iter()
        .flat_map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 8);
    let ids = claimed
        .iter()
        .map(|claim| claim.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 8);
}

#[tokio::test]
async fn committed_projection_mutations_wake_generation_waiters() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let wake = store.wake_signal();
    let observed = wake.generation();
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 1), "wake-token-00001", 10),
            1,
        )
        .unwrap();
    assert_eq!(wake.notified_since(observed).await, observed + 1);
}

#[test]
fn retry_and_discarding_transitions_keep_cross_database_ordering() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    store
        .upsert_alarm(
            &projection(namespace, object(namespace, 1), "retry-token-0001", 10),
            1,
        )
        .unwrap();
    let [first] = store.claim_due(10, 100, 1).unwrap().try_into().unwrap();
    assert!(
        store
            .finish_claim(
                &first,
                ClaimResult::Reschedule {
                    due_at_ms: 2_010,
                    retry_count: 1,
                    last_error_code: Some("DO_RUNTIME_EXCEPTION"),
                },
                11,
            )
            .unwrap()
    );
    let [second] = store.claim_due(2_010, 100, 1).unwrap().try_into().unwrap();
    assert_eq!(second.retry_count, 1);
    assert!(
        store
            .finish_claim(
                &second,
                ClaimResult::MarkDiscarding {
                    last_error_code: "DO_RUNTIME_EXCEPTION",
                },
                2_011,
            )
            .unwrap()
    );
    let summary = store.summary(2_011).unwrap();
    assert_eq!(summary.discarding, 1);
    assert_eq!(summary.claimed, 0);
    assert!(store.finish_discarding(&second).unwrap());
    assert_eq!(store.summary(2_012).unwrap(), SchedulerSummary::default());
}

#[test]
fn exact_delete_and_object_delete_do_not_cross_generation_or_token() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    let object_id = object(namespace, 1);
    store
        .upsert_alarm(&projection(namespace, object_id, "delete-token-001", 10), 1)
        .unwrap();
    assert!(
        !store
            .delete_alarm_exact(namespace, object_id, 1, "different-token-1")
            .unwrap()
    );
    assert_eq!(store.delete_object(namespace, object_id, 2).unwrap(), 0);
    assert_eq!(store.delete_object(namespace, object_id, 1).unwrap(), 1);
}

#[test]
fn malformed_projection_is_rejected_before_sql() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let namespace = ResourceId::generate();
    let invalid_projection = projection(namespace, object(namespace, 1), "short", 0);
    assert_eq!(
        store
            .upsert_alarm(&invalid_projection, 1)
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError
    );
}

#[test]
fn queue_projection_enqueue_retention_and_repair_boundaries_are_complete() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 1);
    let queue_id = QueueId::generate();
    let account_id = AccountId::generate();
    let config = crate::QueueConfig {
        retention_seconds: 60,
        max_message_bytes: 4,
        max_batch_messages: 2,
        max_batch_bytes: 6,
        max_backlog_bytes: 12,
        ..crate::QueueConfig::default()
    };
    let projection = QueueProjection {
        queue_id,
        account_id,
        lifecycle_generation: 1,
        config_generation: 1,
        config,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
    };

    assert_eq!(QueueContentType::Json.as_str(), "json");
    assert_eq!(QueueContentType::Text.as_str(), "text");
    assert_eq!(QueueContentType::Bytes.as_str(), "bytes");
    assert_eq!(
        "json".parse::<QueueContentType>().unwrap(),
        QueueContentType::Json
    );
    assert_eq!(
        "text".parse::<QueueContentType>().unwrap(),
        QueueContentType::Text
    );
    assert_eq!(
        "bytes".parse::<QueueContentType>().unwrap(),
        QueueContentType::Bytes
    );
    assert_eq!(
        "xml".parse::<QueueContentType>().unwrap_err().code(),
        ErrorCode::QueueContentTypeUnsupported
    );

    let mut invalid_projection = projection.clone();
    invalid_projection.lifecycle_generation = 0;
    assert_eq!(
        store
            .create_queue_projection(&invalid_projection)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );
    store.create_queue_projection(&projection).unwrap();
    assert_eq!(
        store
            .create_queue_projection(&projection)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );
    store.ensure_queue_projection(&projection).unwrap();
    store.verify_queue_projection(&projection).unwrap();
    let mut mismatched = projection.clone();
    mismatched.config.max_backlog_bytes += 1;
    assert_eq!(
        store
            .ensure_queue_projection(&mismatched)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );
    let mut missing = projection.clone();
    missing.queue_id = QueueId::generate();
    assert_eq!(
        store.verify_queue_projection(&missing).unwrap_err().code(),
        ErrorCode::QueueInvariantViolation
    );
    assert_eq!(
        store
            .begin_queue_config(QueueId::generate(), 1, 1, 2)
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    assert_eq!(
        store
            .finish_queue_config(queue_id, 9, 9, 2)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );

    let message = |body: &[u8], delay| QueueMessageInput {
        content_type: QueueContentType::Bytes,
        body: body.to_vec(),
        delay_seconds: delay,
    };
    let request = |messages: Vec<QueueMessageInput>| QueueEnqueueRequest {
        queue_id,
        lifecycle_generation: 1,
        config_generation: 1,
        batch_delay_seconds: None,
        messages,
    };
    assert_eq!(
        store
            .enqueue_queue(&request(Vec::new()), 2_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvalidMessage
    );
    let mut bad_generation = request(vec![message(b"x", None)]);
    bad_generation.lifecycle_generation = 0;
    assert_eq!(
        store
            .enqueue_queue(&bad_generation, 2_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvalidMessage
    );
    let too_many = request(
        (0..=crate::QUEUE_MAX_BATCH_MESSAGES)
            .map(|_| message(b"", None))
            .collect(),
    );
    assert_eq!(
        store.enqueue_queue(&too_many, 2_000).unwrap_err().code(),
        ErrorCode::QueueBatchLimitExceeded
    );
    let mut delayed = request(vec![message(b"x", None)]);
    delayed.batch_delay_seconds = Some(crate::QUEUE_MAX_DELAY_SECONDS + 1);
    assert_eq!(
        store.enqueue_queue(&delayed, 2_000).unwrap_err().code(),
        ErrorCode::QueueDelayInvalid
    );
    let delayed_message = request(vec![message(
        b"x",
        Some(crate::QUEUE_MAX_DELAY_SECONDS + 1),
    )]);
    assert_eq!(
        store
            .enqueue_queue(&delayed_message, 2_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueDelayInvalid
    );
    let oversized = request(vec![message(
        &vec![0; usize::try_from(crate::QUEUE_MAX_MESSAGE_BYTES).unwrap() + 1],
        None,
    )]);
    assert_eq!(
        store.enqueue_queue(&oversized, 2_000).unwrap_err().code(),
        ErrorCode::QueueMessageTooLarge
    );
    let dynamic_count = request(vec![
        message(b"a", None),
        message(b"b", None),
        message(b"c", None),
    ]);
    assert_eq!(
        store
            .enqueue_queue(&dynamic_count, 2_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueBatchLimitExceeded
    );
    let dynamic_message = request(vec![message(b"12345", None)]);
    assert_eq!(
        store
            .enqueue_queue(&dynamic_message, 2_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueMessageTooLarge
    );
    let dynamic_batch = request(vec![message(b"1234", None), message(b"1234", None)]);
    assert_eq!(
        store
            .enqueue_queue(&dynamic_batch, 2_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueBatchLimitExceeded
    );
    let mut stale = request(vec![message(b"x", None)]);
    stale.lifecycle_generation = 2;
    assert_eq!(
        store.enqueue_queue(&stale, 2_000).unwrap_err().code(),
        ErrorCode::QueueInvariantViolation
    );
    assert_eq!(
        store.queue_metrics(queue_id, 2, 1).unwrap_err().code(),
        ErrorCode::QueueConfigPending
    );
    assert_eq!(store.queue_backlog_totals().unwrap(), (0, 0));
    assert_eq!(
        store.sweep_queue_retention(1, 0, 1).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        store.purge_queue(queue_id, 1, 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        store.purge_queue(queue_id, 1, 1).unwrap_err().code(),
        ErrorCode::QueueInvariantViolation
    );

    store
        .enqueue_queue(
            &request(vec![message(b"1234", Some(0)), message(b"12", None)]),
            2_000,
        )
        .unwrap();
    assert_eq!(store.queue_backlog_totals().unwrap(), (2, 6));
    let workload = store.queue_workload_summary(61_999).unwrap();
    assert_eq!(workload.ready, 0);
    assert_eq!(workload.next_due_at_ms, Some(62_000));
    let limited = store.sweep_queue_retention(62_000, 10, 4).unwrap();
    assert_eq!(limited.messages, 1);
    assert_eq!(limited.bytes, 4);
    assert!(limited.expired_remaining);
    assert_eq!(
        store
            .delete_queue_projection(queue_id, 1)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );

    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE queue_state SET message_count = message_count + 1 WHERE queue_id = ?1",
            [queue_id.to_string()],
        )
        .unwrap();
    drop(connection);
    let mismatch = store.queue_counter_mismatches().unwrap().pop().unwrap();
    assert_eq!(mismatch.queue_id, queue_id);
    assert!(store.repair_queue_counter(mismatch).unwrap());
    assert!(!store.repair_queue_counter(mismatch).unwrap());
    assert!(store.queue_counter_mismatches().unwrap().is_empty());

    let metrics = store.fence_queue_delete(queue_id, 1, 63_000).unwrap();
    assert_eq!(metrics.backlog_count, 1);
    assert_eq!(
        store
            .enqueue_queue(&request(vec![message(b"x", None)]), 63_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    let purged = store.purge_queue(queue_id, 10, 10).unwrap();
    assert_eq!(purged.messages, 1);
    assert_eq!(purged.bytes, 2);
    assert!(!purged.expired_remaining);
    store.delete_queue_projection(queue_id, 1).unwrap();
    store.delete_queue_projection(queue_id, 1).unwrap();
    assert_eq!(
        store
            .fence_queue_delete(queue_id, 1, 64_000)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );

    let config_queue = QueueId::generate();
    let mut config_projection = projection.clone();
    config_projection.queue_id = config_queue;
    store.create_queue_projection(&config_projection).unwrap();
    store
        .begin_queue_config(config_queue, 1, 1, 70_000)
        .unwrap();
    let mut next = config_projection.clone();
    next.config_generation = 2;
    next.config.delivery_delay_seconds = 3;
    next.updated_at_ms = 70_001;
    store.reconcile_queue_config(&next).unwrap();
    store.reconcile_queue_config(&next).unwrap();
    store
        .finish_queue_config(config_queue, 1, 2, 70_002)
        .unwrap();
    store.verify_queue_projection(&next).unwrap();
    let mut bad_next = next.clone();
    bad_next.config_generation = 4;
    assert_eq!(
        store.reconcile_queue_config(&bad_next).unwrap_err().code(),
        ErrorCode::QueueInvariantViolation
    );
    let mut zero_generation = next;
    zero_generation.config_generation = 0;
    assert_eq!(
        store
            .project_queue_config(&zero_generation)
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );
}

#[test]
fn queue_consumer_claim_completion_recovery_and_dlq_are_token_fenced() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let account_id = AccountId::generate();
    let source_id = QueueId::generate();
    let dlq_id = QueueId::generate();
    let queue_config = crate::QueueConfig {
        retention_seconds: 60,
        max_message_bytes: 1024,
        max_batch_messages: 100,
        max_batch_bytes: 4096,
        max_backlog_bytes: 4096,
        ..crate::QueueConfig::default()
    };
    for queue_id in [source_id, dlq_id] {
        store
            .create_queue_projection(&QueueProjection {
                queue_id,
                account_id,
                lifecycle_generation: 1,
                config_generation: 1,
                config: queue_config,
                created_at_ms: 1,
                updated_at_ms: 1,
            })
            .unwrap();
    }
    let consumer_id = QueueConsumerId::generate();
    let deployment_id = DeploymentId::generate();
    let worker_id = WorkerId::generate();
    let consumer = QueueConsumerProjection {
        consumer_id,
        queue_id: source_id,
        consumer_generation: 1,
        deployment_id,
        worker_id,
        execution_generation: 1,
        entrypoint: None,
        config: crate::QueueConsumerConfig {
            max_batch_size: 2,
            max_batch_timeout_seconds: 0,
            max_retries: 1,
            retry_delay_seconds: 0,
            max_concurrency: 1,
        },
        dead_letter_queue: Some((dlq_id, 1)),
        descriptor_sha256: [7; 32],
        updated_at_ms: 1,
    };
    store.ensure_queue_consumer_projection(&consumer).unwrap();
    store.activate_queue_consumer(consumer_id, 1, 2).unwrap();
    let enqueue = store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: source_id,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![
                    QueueMessageInput {
                        content_type: QueueContentType::Text,
                        body: b"first".to_vec(),
                        delay_seconds: None,
                    },
                    QueueMessageInput {
                        content_type: QueueContentType::Bytes,
                        body: vec![0, 255],
                        delay_seconds: None,
                    },
                ],
            },
            10,
        )
        .unwrap();
    let [first] = store
        .claim_queue_batches(10, 100, 5, 1)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(first.messages.len(), 2);
    assert_eq!(first.messages[0].delivery_attempt, 1);
    let summary = store
        .complete_queue_batch(
            &first,
            &[
                QueueCompletionDecision {
                    message_id: first.messages[0].id,
                    action: QueueCompletionAction::Ack,
                },
                QueueCompletionDecision {
                    message_id: first.messages[1].id,
                    action: QueueCompletionAction::Retry { delay_seconds: 0 },
                },
            ],
            11,
        )
        .unwrap();
    assert_eq!(summary.acknowledged, 1);
    assert_eq!(summary.retried, 1);
    assert_eq!(
        store.queue_metrics(source_id, 1, 1).unwrap().backlog_count,
        1
    );
    assert!(
        store
            .complete_queue_batch(&first, &[], 12)
            .unwrap_err()
            .code()
            == ErrorCode::QueueDispositionInvalid
    );

    let [second] = store
        .claim_queue_batches(12, 100, 5, 1)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(second.messages[0].delivery_attempt, 2);
    store.begin_queue_config(dlq_id, 1, 1, 12).unwrap();
    let pending = store
        .complete_queue_batch(
            &second,
            &[QueueCompletionDecision {
                message_id: second.messages[0].id,
                action: QueueCompletionAction::Retry { delay_seconds: 0 },
            }],
            13,
        )
        .unwrap();
    assert_eq!(pending.dlq_pending, 1);
    assert_eq!(
        store.queue_metrics(source_id, 1, 1).unwrap().backlog_count,
        1
    );
    store.finish_queue_config(dlq_id, 1, 1, 14).unwrap();
    let forwarded = store.forward_queue_dlq_pending(1_013, 100, 10).unwrap();
    assert_eq!(forwarded.moved, 1);
    assert_eq!(
        store.queue_metrics(source_id, 1, 1).unwrap().backlog_count,
        0
    );
    assert_eq!(store.queue_metrics(dlq_id, 1, 1).unwrap().backlog_count, 1);
    assert_eq!(enqueue.message_ids.len(), 2);

    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: source_id,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Json,
                    body: b"{}".to_vec(),
                    delay_seconds: None,
                }],
            },
            2_000,
        )
        .unwrap();
    let [unknown] = store
        .claim_queue_batches(2_000, 10, 5, 1)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(store.recover_expired_queue_batches(2_010, 5, 1).unwrap(), 1);
    let [recovered] = store
        .claim_queue_batches(2_015, 10, 5, 1)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        unknown.messages[0].delivery_attempt,
        recovered.messages[0].delivery_attempt
    );
    assert_ne!(unknown.claim_token, recovered.claim_token);
    store
        .complete_queue_batch(
            &recovered,
            &[QueueCompletionDecision {
                message_id: recovered.messages[0].id,
                action: QueueCompletionAction::Ack,
            }],
            2_016,
        )
        .unwrap();

    store
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: source_id,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"retention-race".to_vec(),
                    delay_seconds: None,
                }],
            },
            4_000,
        )
        .unwrap();
    let [_claimed_at_expiry] = store
        .claim_queue_batches(4_000, 70_000, 5, 1)
        .unwrap()
        .try_into()
        .unwrap();
    store.sweep_queue_retention(65_000, 10, 4096).unwrap();
    assert_eq!(
        store.queue_metrics(source_id, 1, 1).unwrap().backlog_count,
        1,
        "retention must not delete an in-flight claim"
    );
    assert_eq!(
        store.recover_expired_queue_batches(74_000, 5, 1).unwrap(),
        1
    );
    assert_eq!(
        store
            .sweep_queue_retention(74_000, 10, 4096)
            .unwrap()
            .messages,
        1,
        "an expired message becomes retention-eligible after lease recovery"
    );
}

#[test]
fn cron_slots_retries_and_unknown_recovery_preserve_logical_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let activation_id = CronActivationId::generate();
    let projection = CronScheduleProjection {
        activation_id,
        account_id: AccountId::generate(),
        worker_id: WorkerId::generate(),
        deployment_id: DeploymentId::generate(),
        execution_generation: 1,
        activation_generation: 1,
        expression: "* * * * *".to_owned(),
        expression_sha256: [9; 32],
        parser_version: 1,
        next_fire_at_ms: 60_000,
        updated_at_ms: 1,
    };
    store.ensure_cron_schedule_projection(&projection).unwrap();
    store.activate_cron_schedule(activation_id, 1, 2).unwrap();
    assert_eq!(
        store
            .project_due_cron_slots(60_000, 300_000, 10)
            .unwrap()
            .projected,
        1
    );
    assert_eq!(
        store
            .project_due_cron_slots(60_000, 300_000, 10)
            .unwrap()
            .projected,
        0
    );
    let [first] = store
        .claim_cron_runs(60_000, 100, 5, 10)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(first.scheduled_at_ms, 60_000);
    assert_eq!(
        store
            .complete_cron_run(
                &first,
                CronCompletion::Failure {
                    no_retry: false,
                    error_code: "CRON_RUNTIME_EXCEPTION",
                },
                60_001,
                3,
            )
            .unwrap(),
        CronCompletionResult::Retried
    );
    let retry_at = store
        .cron_workload_summary(i64::MAX)
        .unwrap()
        .oldest_due_at_ms
        .unwrap();
    let [retry] = store
        .claim_cron_runs(retry_at, 10, 5, 10)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(retry.id, first.id);
    assert_eq!(retry.attempt, 1);
    assert_eq!(
        store
            .recover_expired_cron_runs(retry_at + 10, 5, 1)
            .unwrap(),
        1
    );
    let [recovered] = store
        .claim_cron_runs(retry_at + 15, 10, 5, 10)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(recovered.id, first.id);
    assert_eq!(recovered.attempt, 1);
    assert_ne!(recovered.claim_token, retry.claim_token);
    assert_eq!(
        store
            .complete_cron_run(&recovered, CronCompletion::Success, retry_at + 16, 3)
            .unwrap(),
        CronCompletionResult::Terminal
    );
    assert_eq!(
        store
            .complete_cron_run(&retry, CronCompletion::Success, retry_at + 17, 3)
            .unwrap(),
        CronCompletionResult::Stale
    );
    assert_eq!(store.gc_cron_history(retry_at + 18, 1, 100).unwrap(), 1);
}

#[test]
fn explicit_corrupt_recovery_quarantines_files_and_refuses_healthy_authority() {
    use std::os::unix::fs::OpenOptionsExt as _;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = open_compute_core::StorageConfig {
        data_dir: root.clone(),
        master_key_file: temp.path().join("master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 100,
        free_space_soft_bytes: 2,
        free_space_hard_bytes: 1,
    };
    let storage =
        crate::PlatformStorage::bootstrap(&config, &open_compute_core::SystemClock).unwrap();
    let data_dir = storage.data_dir();
    let scheduler = data_dir.ensure_scheduler_db().unwrap();
    std::fs::write(&scheduler, b"not a sqlite database").unwrap();
    let wal = std::path::PathBuf::from(format!("{}-wal", scheduler.display()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&wal)
        .unwrap();

    assert!(
        data_dir
            .recover_corrupt_scheduler_db("invalid", 100, 10)
            .is_err()
    );
    let backup = data_dir
        .recover_corrupt_scheduler_db("scheduler-corrupt-test", 100, 10)
        .unwrap();
    assert_eq!(
        std::fs::read(backup.join("scheduler.sqlite")).unwrap(),
        b"not a sqlite database"
    );
    assert!(backup.join("scheduler.sqlite-wal").is_file());
    assert!(inspect_scheduler_db(&scheduler, 100, 10).is_ok());
    assert!(
        data_dir
            .recover_corrupt_scheduler_db("scheduler-corrupt-second", 100, 10)
            .is_err()
    );
}
