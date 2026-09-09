use super::*;

mod workflow_migration;
use open_compute_core::{AccountId, CronActivationId, QueueConsumerId, QueueId, WorkerId};

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
        target_version_id: VersionId::generate(),
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

fn create_current_scheduler_fixture(path: &std::path::Path, definitions: usize) -> Connection {
    let mut connection = Connection::open(path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    for definition in SCHEDULER_MIGRATIONS.iter().take(definitions) {
        let tx = connection.transaction().unwrap();
        tx.execute_batch(definition.sql).unwrap();
        if definition.version == 1 {
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
                [definition.version],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO scheduler_migrations
             (version, name, checksum_sha256, applied_at_ms, app_version)
             VALUES (?1, ?2, ?3, 1, ?4)",
            params![
                definition.version,
                definition.name,
                definition.checksum.as_slice(),
                APP_VERSION,
            ],
        )
        .unwrap();
        tx.pragma_update(None, "user_version", definition.version)
            .unwrap();
        tx.commit().unwrap();
    }
    connection
}

mod migrates_and_reopens_the_independent_database;

mod scheduler_registry_is_contiguous_and_future_schema_fails_closed;

mod reopening_current_schema_preserves_definition_identity_and_alarm_rows;

mod overwrite_claim_and_conditional_completion_are_token_fenced;

mod due_order_is_stable_and_batches_are_bounded;

mod expired_lease_recovers_with_a_new_random_claim_token;

mod recovery_is_bounded_and_workload_summary_includes_lease_deadline;

mod concurrent_claim_transactions_never_duplicate_an_alarm;

mod committed_projection_mutations_wake_generation_waiters;

mod retry_and_discarding_transitions_keep_cross_database_ordering;

mod exact_delete_and_object_delete_do_not_cross_generation_or_token;

mod malformed_projection_is_rejected_before_sql;

mod queue_projection_enqueue_retention_and_repair_boundaries_are_complete;

mod durable_object_queue_operation_survives_message_retention_until_finalize;

mod queue_producer_persists_v8_content_type;

mod queue_consumer_claim_completion_recovery_and_dlq_are_token_fenced;

mod cron_slots_retries_and_unknown_recovery_preserve_logical_identity;

mod explicit_corrupt_recovery_quarantines_files_and_refuses_healthy_authority;
