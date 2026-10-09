use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn queue_backlog_hot_paths_are_bounded_and_preserve_batch_deadlines() {
    let temp = tempfile::tempdir().unwrap();
    let store = open_store(&temp, 1);
    let queue_id = QueueId::generate();
    store
        .create_queue_projection(&QueueProjection {
            queue_id,
            instance_id: store.instance_id(),
            lifecycle_generation: 1,
            config_generation: 1,
            config: crate::queues::QueueConfig {
                retention_seconds: 60,
                ..crate::queues::QueueConfig::default()
            },
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    let consumer_id = QueueConsumerId::generate();
    store
        .ensure_queue_consumer_projection(&QueueConsumerProjection {
            consumer_id,
            queue_id,
            consumer_generation: 1,
            version_id: VersionId::generate(),
            worker_id: WorkerId::generate(),
            execution_generation: 1,
            entrypoint: None,
            config: crate::queue_consumers::QueueConsumerConfig {
                max_batch_size: crate::queue_consumers::QUEUE_CONSUMER_MAX_BATCH_SIZE,
                max_concurrency: 1,
                ..crate::queue_consumers::QueueConsumerConfig::default()
            },
            dead_letter_queue: None,
            descriptor_sha256: [1; 32],
            updated_at_ms: 1,
        })
        .unwrap();
    store.activate_queue_consumer(consumer_id, 1, 1).unwrap();
    let request = |count| QueueEnqueueRequest {
        queue_id,
        request_id: Uuid::now_v7(),
        output_gate: false,
        lifecycle_generation: 1,
        config_generation: 1,
        batch_delay_seconds: None,
        messages: (0..count)
            .map(|_| QueueMessageInput {
                content_type: QueueContentType::Text,
                body: b"job".to_vec(),
                delay_seconds: None,
            })
            .collect(),
    };
    store.enqueue_queue(&request(1), 10).unwrap();
    let waiting = store.queue_consumer_workload_summary(5_009).unwrap();
    assert_eq!(waiting.ready, 0);
    assert_eq!(waiting.oldest_due_at_ms, Some(10));
    assert_eq!(waiting.next_due_at_ms, Some(5_010));
    assert!(
        store
            .claim_queue_batches(5_009, 1_000, 5, 1, None)
            .unwrap()
            .0
            .is_empty()
    );
    let partial = store
        .claim_queue_batches(5_010, 1_000, 5, 1, None)
        .unwrap()
        .0
        .pop()
        .unwrap();
    assert_eq!(partial.messages.len(), 1);
    store
        .complete_queue_batch(
            &partial,
            &[QueueCompletionDecision {
                message_id: partial.messages[0].id,
                action: QueueCompletionAction::Ack,
            }],
            5_011,
        )
        .unwrap();
    for _ in 0..100 {
        store.enqueue_queue(&request(100), 6_000).unwrap();
    }
    let steps = Arc::new(AtomicUsize::new(0));
    let observed = steps.clone();
    // Bound work against an unexpired backlog without a wall-clock timing assertion.
    store.lock().unwrap().progress_handler(
        100,
        Some(move || observed.fetch_add(100, Ordering::Relaxed) >= 50_000),
    );
    let ready = store.queue_consumer_workload_summary(6_001).unwrap();
    assert_eq!(ready.ready, 1);
    assert_eq!(ready.oldest_due_at_ms, Some(6_000));
    assert_eq!(ready.next_due_at_ms, Some(11_000));
    steps.store(0, Ordering::Relaxed);
    let retention = store.queue_workload_summary(6_001).unwrap();
    assert_eq!(retention.ready, 0);
    assert_eq!(retention.oldest_due_at_ms, None);
    assert_eq!(retention.next_due_at_ms, Some(66_000));
    steps.store(0, Ordering::Relaxed);
    let batch = store
        .claim_queue_batches(6_001, 20_000, 5, 1, None)
        .unwrap()
        .0
        .pop()
        .unwrap();
    assert_eq!(batch.messages.len(), 100);
    steps.store(0, Ordering::Relaxed);
    let occupied = store.queue_consumer_workload_summary(6_002).unwrap();
    assert_eq!(occupied.ready, 0);
    assert_eq!(occupied.claimed, 1);
    assert_eq!(occupied.next_due_at_ms, Some(26_001));
    store
        .lock()
        .unwrap()
        .progress_handler(0, None::<fn() -> bool>);
    // A full consumer wakes at its claim lease, not an overdue backlog timeout.
    let saturated = store.queue_consumer_workload_summary(11_001).unwrap();
    assert_eq!(saturated.ready, 0);
    assert_eq!(saturated.expired, 0);
    assert_eq!(saturated.next_due_at_ms, Some(26_001));
    let lease_due = store.queue_consumer_workload_summary(26_001).unwrap();
    assert_eq!(lease_due.expired, 1);
    assert_eq!(lease_due.next_due_at_ms, Some(26_001));
    // Expired messages still contribute their exact count and deadline.
    let expired = store.queue_workload_summary(66_000).unwrap();
    assert_eq!(expired.ready, 9_900);
    assert_eq!(expired.oldest_due_at_ms, Some(66_000));
}
