use super::*;

mod workflow_migration;
use open_compute_core::{CronActivationId, InstanceId, QueueConsumerId, QueueId, WorkerId};

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
    SchedulerStore::open(
        &path,
        100,
        now_ms,
        "019c0000000070008000000000000001".parse().unwrap(),
    )
    .unwrap()
}

#[test]
fn queue_and_cron_projections_reject_another_instance() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let other = InstanceId::generate();
    assert_eq!(
        store
            .create_queue_projection(&QueueProjection {
                queue_id: QueueId::generate(),
                instance_id: other,
                lifecycle_generation: 1,
                config_generation: 1,
                config: crate::QueueConfig::default(),
                created_at_ms: 1,
                updated_at_ms: 1,
            })
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError
    );
    assert_eq!(
        store
            .ensure_cron_schedule_projection(&CronScheduleProjection {
                activation_id: CronActivationId::generate(),
                instance_id: other,
                worker_id: WorkerId::generate(),
                version_id: VersionId::generate(),
                execution_generation: 1,
                activation_generation: 1,
                expression: "* * * * *".to_owned(),
                expression_sha256: [1; 32],
                parser_version: 1,
                next_fire_at_ms: 60_000,
                updated_at_ms: 1,
            })
            .unwrap_err()
            .code(),
        ErrorCode::SchedulerInternalProtocolError
    );
    let rows: i64 = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM queue_state) +
                    (SELECT COUNT(*) FROM cron_schedules)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
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
