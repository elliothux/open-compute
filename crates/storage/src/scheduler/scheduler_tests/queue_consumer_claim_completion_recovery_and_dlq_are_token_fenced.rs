use super::*;

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
    let version_id = VersionId::generate();
    let worker_id = WorkerId::generate();
    let consumer = QueueConsumerProjection {
        consumer_id,
        queue_id: source_id,
        consumer_generation: 1,
        version_id,
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
                request_id: Uuid::now_v7(),
                output_gate: false,
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
        .claim_queue_batches(10, 100, 5, 1, None)
        .map(|(items, _)| items)
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
        .claim_queue_batches(12, 100, 5, 1, None)
        .map(|(items, _)| items)
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
                request_id: Uuid::now_v7(),
                output_gate: false,
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
        .claim_queue_batches(2_000, 10, 5, 1, None)
        .map(|(items, _)| items)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(store.recover_expired_queue_batches(2_010, 5, 1).unwrap(), 1);
    let [recovered] = store
        .claim_queue_batches(2_015, 10, 5, 1, None)
        .map(|(items, _)| items)
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
                request_id: Uuid::now_v7(),
                output_gate: false,
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
        .claim_queue_batches(4_000, 70_000, 5, 1, None)
        .map(|(items, _)| items)
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
