use super::*;

#[test]
fn p2_2_queue_catalog_projection_and_config_fences_are_exact() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap();
    let account_id = storage.identity().default_account_id;
    let queue_id = open_compute_core::QueueId::generate();
    let repository = crate::QueueRepository::new(storage.db());
    let queue = repository
        .insert_creating(
            account_id,
            queue_id,
            "events",
            crate::QueueConfig::default(),
            10,
        )
        .unwrap();
    assert_eq!(queue.state, crate::QueueState::Creating);
    assert_eq!(queue.availability, crate::QueueAvailability::Degraded);
    let projection = crate::QueueProjection {
        queue_id,
        account_id,
        lifecycle_generation: queue.lifecycle_generation,
        config_generation: queue.config_generation,
        config: queue.config,
        created_at_ms: queue.created_at_ms,
        updated_at_ms: queue.updated_at_ms,
    };
    scheduler.create_queue_projection(&projection).unwrap();
    scheduler.verify_queue_projection(&projection).unwrap();
    let ready = repository.mark_ready(account_id, queue_id, 11).unwrap();
    assert_eq!(ready.state, crate::QueueState::Ready);
    assert_eq!(ready.availability, crate::QueueAvailability::Healthy);
    assert_eq!(
        repository
            .insert_creating(
                account_id,
                open_compute_core::QueueId::generate(),
                "events",
                crate::QueueConfig::default(),
                12,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueNameConflict
    );
    let raw = Connection::open(storage.data_dir().control_db_path()).unwrap();
    assert!(
        raw.execute(
            "UPDATE queues SET delivery_delay_seconds = 4 WHERE id = ?1",
            [queue_id.to_string()],
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "UPDATE queues SET config_generation = config_generation + 1 WHERE id = ?1",
            [queue_id.to_string()],
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "UPDATE queues SET name = 'combined', delivery_delay_seconds = 4,
                    config_generation = config_generation + 1,
                    availability = 'degraded', availability_code = 'QUEUE_CONFIG_PENDING'
             WHERE id = ?1",
            [queue_id.to_string()],
        )
        .is_err()
    );
    drop(raw);

    scheduler.begin_queue_config(queue_id, 1, 1, 20).unwrap();
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
                        body: b"blocked".to_vec(),
                        delay_seconds: None,
                    }],
                },
                20,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    let mut next_config = ready.config;
    next_config.delivery_delay_seconds = 9;
    next_config.max_backlog_bytes = 4096;
    let pending = repository
        .write_config_pending(account_id, queue_id, 1, next_config, 21)
        .unwrap();
    let next_projection = crate::QueueProjection {
        queue_id,
        account_id,
        lifecycle_generation: 1,
        config_generation: 2,
        config: next_config,
        created_at_ms: pending.created_at_ms,
        updated_at_ms: pending.updated_at_ms,
    };
    scheduler.project_queue_config(&next_projection).unwrap();
    let healthy = repository
        .mark_config_healthy(
            account_id,
            queue_id,
            2,
            open_compute_core::RequestId::generate(),
            22,
        )
        .unwrap();
    scheduler.finish_queue_config(queue_id, 1, 2, 23).unwrap();
    scheduler.verify_queue_projection(&next_projection).unwrap();
    assert_eq!(healthy.config_generation, 2);
    assert_eq!(healthy.config.delivery_delay_seconds, 9);
}
