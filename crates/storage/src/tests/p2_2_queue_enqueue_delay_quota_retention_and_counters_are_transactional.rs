use super::*;

#[test]
fn p2_2_queue_enqueue_delay_quota_retention_and_counters_are_transactional() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap();
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = crate::QueueConfig {
        delivery_delay_seconds: 7,
        retention_seconds: 60,
        max_backlog_bytes: 8,
        ..crate::QueueConfig::default()
    };
    scheduler
        .create_queue_projection(&crate::QueueProjection {
            queue_id,
            account_id: storage.identity().default_account_id,
            lifecycle_generation: 1,
            config_generation: 1,
            config: queue_config,
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        })
        .unwrap();
    let result = scheduler
        .enqueue_queue(
            &crate::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: Some(3),
                messages: vec![
                    crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Json,
                        body: b"{}".to_vec(),
                        delay_seconds: None,
                    },
                    crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Bytes,
                        body: vec![1, 2, 3],
                        delay_seconds: Some(0),
                    },
                ],
            },
            1_000,
        )
        .unwrap();
    assert_eq!(result.message_ids.len(), 2);
    assert_eq!(result.metrics.backlog_count, 2);
    assert_eq!(result.metrics.backlog_bytes, 5);
    let reader = Connection::open(&scheduler_path).unwrap();
    let rows = reader
        .prepare(
            "SELECT available_at_ms, expires_at_ms, content_type, body
             FROM queue_messages WHERE queue_id = ?1 ORDER BY seq",
        )
        .unwrap()
        .query_map([queue_id.to_string()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows[0], (4_000, 61_000, "json".to_owned(), b"{}".to_vec()));
    assert_eq!(rows[1], (1_000, 61_000, "bytes".to_owned(), vec![1, 2, 3]));
    assert_eq!(
        scheduler
            .enqueue_queue(
                &crate::QueueEnqueueRequest {
                    queue_id,
                    request_id: uuid::Uuid::now_v7(),
                    output_gate: false,
                    lifecycle_generation: 1,
                    config_generation: 1,
                    batch_delay_seconds: None,
                    messages: vec![crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Text,
                        body: b"more".to_vec(),
                        delay_seconds: None,
                    }],
                },
                2_000,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueBacklogLimitExceeded
    );
    assert_eq!(
        scheduler
            .queue_metrics(queue_id, 1, 1)
            .unwrap()
            .backlog_count,
        2
    );
    let swept = scheduler.sweep_queue_retention(61_000, 100, 1024).unwrap();
    assert_eq!(swept.messages, 2);
    assert_eq!(swept.bytes, 5);
    assert_eq!(
        scheduler
            .queue_metrics(queue_id, 1, 1)
            .unwrap()
            .backlog_count,
        0
    );
    assert!(scheduler.queue_counter_mismatches().unwrap().is_empty());
    drop(reader);
    drop(scheduler);
    let inspection = crate::inspect_scheduler_db(&scheduler_path, 5_000, 61_000).unwrap();
    assert_eq!(
        inspection.schema_version,
        crate::current_scheduler_schema_version()
    );
    assert_eq!(inspection.queue.queues, 1);
    assert_eq!(inspection.queue.backlog_messages, 0);
    assert_eq!(inspection.queue.counter_mismatches, 0);
}
