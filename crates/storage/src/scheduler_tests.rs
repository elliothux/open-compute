use super::*;

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
    assert_eq!(registry.len(), 1);
    assert_eq!(registry[0].0, 1);
    assert_eq!(registry[0].1, "001_scheduler");

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 10);
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
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
    let data_dir = crate::DataDir::acquire(&config).unwrap();
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
