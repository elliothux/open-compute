use super::*;

#[test]
fn durable_object_queue_operation_survives_message_retention_until_finalize() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let store = open_store(&temp, 1);
    let queue_id = QueueId::generate();
    store
        .create_queue_projection(&QueueProjection {
            queue_id,
            account_id: AccountId::generate(),
            lifecycle_generation: 1,
            config_generation: 1,
            config: crate::QueueConfig {
                retention_seconds: 60,
                ..crate::QueueConfig::default()
            },
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        })
        .unwrap();
    let request_id = Uuid::now_v7();
    let request = QueueEnqueueRequest {
        queue_id,
        request_id,
        output_gate: true,
        lifecycle_generation: 1,
        config_generation: 1,
        batch_delay_seconds: None,
        messages: vec![QueueMessageInput {
            content_type: QueueContentType::Text,
            body: b"once".to_vec(),
            delay_seconds: None,
        }],
    };
    let original = store.enqueue_queue(&request, 1_000).unwrap();
    assert_eq!(original.metrics.backlog_count, 1);
    let swept = store.sweep_queue_retention(61_000, 10, 1024).unwrap();
    assert_eq!(swept.messages, 1);
    assert_eq!(
        store.queue_metrics(queue_id, 1, 1).unwrap().backlog_count,
        0
    );

    let connection = Connection::open(&path).unwrap();
    let unfinalized: (i64, Option<i64>, Option<i64>) = connection
        .query_row(
            "SELECT output_gate, finalized_at_ms, expires_at_ms
             FROM queue_enqueue_operations WHERE request_id = ?1",
            [request_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(unfinalized, (1, None, None));
    drop(connection);

    let replay = store.enqueue_queue(&request, 62_000).unwrap();
    assert_eq!(replay.message_ids, original.message_ids);
    assert_eq!(
        store.queue_metrics(queue_id, 1, 1).unwrap().backlog_count,
        0
    );
    let mut conflict = request.clone();
    conflict.output_gate = false;
    assert_eq!(
        store.enqueue_queue(&conflict, 62_000).unwrap_err().code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        store
            .finalize_queue_enqueue(request_id, queue_id, 2, 62_000)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );

    store
        .finalize_queue_enqueue(request_id, queue_id, 1, 62_000)
        .unwrap();
    store
        .finalize_queue_enqueue(request_id, queue_id, 1, 63_000)
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    let finalized: (i64, i64) = connection
        .query_row(
            "SELECT finalized_at_ms, expires_at_ms FROM queue_enqueue_operations
             WHERE request_id = ?1",
            [request_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(finalized, (62_000, 122_000));
    drop(connection);
    store.sweep_queue_retention(122_000, 10, 1024).unwrap();
    let connection = Connection::open(&path).unwrap();
    let remaining: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM queue_enqueue_operations WHERE request_id = ?1",
            [request_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    drop(connection);
    store
        .finalize_queue_enqueue(request_id, queue_id, 1, 123_000)
        .unwrap();
}
