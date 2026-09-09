use super::*;

#[test]
fn p2_2_concurrent_queue_enqueues_never_exceed_backlog_quota() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let queue_id = open_compute_core::QueueId::generate();
    scheduler
        .create_queue_projection(&crate::QueueProjection {
            queue_id,
            account_id: storage.identity().default_account_id,
            lifecycle_generation: 1,
            config_generation: 1,
            config: crate::QueueConfig {
                max_backlog_bytes: 10,
                ..crate::QueueConfig::default()
            },
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for _ in 0..8 {
        let scheduler = scheduler.clone();
        let barrier = barrier.clone();
        threads.push(thread::spawn(move || {
            barrier.wait();
            scheduler.enqueue_queue(
                &crate::QueueEnqueueRequest {
                    queue_id,
                    request_id: uuid::Uuid::now_v7(),
                    output_gate: false,
                    lifecycle_generation: 1,
                    config_generation: 1,
                    batch_delay_seconds: None,
                    messages: vec![crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Bytes,
                        body: vec![1, 2, 3],
                        delay_seconds: Some(0),
                    }],
                },
                2,
            )
        }));
    }
    barrier.wait();
    let mut accepted = 0_u64;
    for thread in threads {
        match thread.join().unwrap() {
            Ok(_) => accepted += 1,
            Err(error) => assert_eq!(error.code(), ErrorCode::QueueBacklogLimitExceeded),
        }
    }
    assert_eq!(accepted, 3);
    let metrics = scheduler.queue_metrics(queue_id, 1, 1).unwrap();
    assert_eq!(metrics.backlog_count, 3);
    assert_eq!(metrics.backlog_bytes, 9);
    assert!(scheduler.queue_counter_mismatches().unwrap().is_empty());
}
