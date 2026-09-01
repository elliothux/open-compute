use super::*;
use open_compute_core::config::StorageConfig;
use open_compute_core::{ErrorCode, SystemClock};
use open_compute_storage::{
    QueueContentType, QueueCreateReservation, QueueEnqueueRequest, QueueMessageInput,
    QueueRepository, QueueState,
};

fn storage() -> (tempfile::TempDir, PlatformStorage, Arc<SchedulerStore>) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    (temp, storage, scheduler)
}

#[test]
fn create_replay_config_backlog_and_force_delete_converge() {
    let (_temp, storage, scheduler) = storage();
    let account_id = storage.identity().default_account_id;
    let request = CreateQueueRequest {
        account_id,
        name: "events".to_owned(),
        config: QueueConfig::default(),
        idempotency_key: "create-events".to_owned(),
        request_id: RequestId::generate(),
        now_ms: 10,
    };
    let controller = QueueController::new(&storage, scheduler.clone());
    let created = match controller.create(&request).unwrap() {
        CreateQueueOutcome::Applied(value) => value.queue,
        CreateQueueOutcome::Replay(_) => panic!("first create unexpectedly replayed"),
    };
    assert_eq!(created.state, QueueState::Ready);
    let replay = match controller.create(&request).unwrap() {
        CreateQueueOutcome::Replay(bytes) => bytes,
        CreateQueueOutcome::Applied(_) => panic!("second create was not replayed"),
    };
    let replay: CreateQueueResult = serde_json::from_slice(&replay).unwrap();
    assert_eq!(replay.queue.id, created.id);

    let renamed = controller
        .rename(
            account_id,
            created.id,
            "renamed-events",
            RequestId::generate(),
            20,
        )
        .unwrap();
    assert_eq!(renamed.lifecycle_generation, 1);
    assert_eq!(renamed.config_generation, 1);
    let mut config = renamed.config;
    config.delivery_delay_seconds = 5;
    config.max_backlog_bytes = 1024;
    let configured = controller
        .update_config(account_id, created.id, 1, config, RequestId::generate(), 30)
        .unwrap();
    assert_eq!(configured.config_generation, 2);
    scheduler
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: created.id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 2,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"durable".to_vec(),
                    delay_seconds: Some(0),
                }],
            },
            31,
        )
        .unwrap();
    assert_eq!(
        controller
            .delete(account_id, created.id, 1, false, RequestId::generate(), 40,)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotEmpty
    );
    assert_eq!(
        QueueRepository::new(storage.db())
            .get(account_id, created.id)
            .unwrap()
            .state,
        QueueState::Ready
    );
    let deleted = controller
        .delete(account_id, created.id, 1, true, RequestId::generate(), 41)
        .unwrap();
    assert_eq!(deleted.queue.state, QueueState::Tombstoned);
    assert_eq!(deleted.purged_messages, 1);
    assert_eq!(deleted.purged_bytes, 7);
}

#[test]
fn create_rejects_invalid_config_and_idempotency_fingerprint_reuse() {
    let (_temp, storage, scheduler) = storage();
    let account_id = storage.identity().default_account_id;
    let controller = QueueController::new(&storage, scheduler);
    let mut request = CreateQueueRequest {
        account_id,
        name: "one".to_owned(),
        config: QueueConfig::default(),
        idempotency_key: "same-key".to_owned(),
        request_id: RequestId::generate(),
        now_ms: 10,
    };
    controller.create(&request).unwrap();
    request.name = "two".to_owned();
    assert_eq!(
        controller.create(&request).unwrap_err().code(),
        ErrorCode::IdempotencyConflict
    );
    request.idempotency_key = "bad-config".to_owned();
    request.config.retention_seconds = 1;
    assert_eq!(
        controller.create(&request).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        QueueRepository::new(storage.db())
            .reserve_create(
                account_id,
                QueueId::generate(),
                "over-quota",
                QueueConfig::default(),
                "quota-key",
                storage.crypto().fingerprint_key_id(),
                &[9; 32],
                11,
                12,
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QuotaExceeded
    );
}

#[test]
fn startup_reconcile_resumes_create_config_and_delete_transaction_boundaries() {
    let (_temp, storage, scheduler) = storage();
    let account_id = storage.identity().default_account_id;
    let request = CreateQueueRequest {
        account_id,
        name: "recovery".to_owned(),
        config: QueueConfig::default(),
        idempotency_key: "recovery-key".to_owned(),
        request_id: RequestId::generate(),
        now_ms: 10,
    };
    let fingerprint = storage
        .crypto()
        .fingerprint_request(&create_fingerprint(&request));
    let queue_id = QueueId::generate();
    let reserved = QueueRepository::new(storage.db())
        .reserve_create(
            account_id,
            queue_id,
            &request.name,
            request.config,
            &request.idempotency_key,
            storage.crypto().fingerprint_key_id(),
            &fingerprint,
            request.now_ms,
            request.now_ms + IDEMPOTENCY_TTL_MS,
            storage.hardening().max_resources_per_kind_per_account,
        )
        .unwrap();
    assert!(matches!(reserved, QueueCreateReservation::Reserved(_)));
    let controller = QueueController::new(&storage, scheduler.clone());
    assert_eq!(controller.reconcile_pending(10, 11).unwrap(), 1);
    let replay = controller.create(&request).unwrap();
    assert!(matches!(replay, CreateQueueOutcome::Replay(_)));

    let ready = QueueRepository::new(storage.db())
        .get(account_id, queue_id)
        .unwrap();
    scheduler.begin_queue_config(queue_id, 1, 1, 20).unwrap();
    let mut config = ready.config;
    config.retention_seconds = 120;
    QueueRepository::new(storage.db())
        .write_config_pending(account_id, queue_id, 1, config, 21)
        .unwrap();
    assert_eq!(controller.reconcile_pending(10, 22).unwrap(), 1);
    let configured = QueueRepository::new(storage.db())
        .get(account_id, queue_id)
        .unwrap();
    assert_eq!(configured.config_generation, 2);
    assert_eq!(configured.config.retention_seconds, 120);
    scheduler
        .verify_queue_projection(&projection(&configured))
        .unwrap();

    QueueRepository::new(storage.db())
        .begin_delete(account_id, queue_id, 1, 30)
        .unwrap();
    assert_eq!(controller.reconcile_pending(10, 31).unwrap(), 1);
    assert_eq!(
        QueueRepository::new(storage.db())
            .get(account_id, queue_id)
            .unwrap()
            .state,
        QueueState::Tombstoned
    );
}
