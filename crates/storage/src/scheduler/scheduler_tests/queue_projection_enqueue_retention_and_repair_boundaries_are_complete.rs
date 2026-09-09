use super::*;

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
    assert_eq!(QueueContentType::V8.as_str(), "v8");
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
        "v8".parse::<QueueContentType>().unwrap(),
        QueueContentType::V8
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
        request_id: Uuid::now_v7(),
        output_gate: false,
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
