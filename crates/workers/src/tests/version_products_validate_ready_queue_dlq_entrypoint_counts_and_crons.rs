use super::*;

#[tokio::test]
async fn version_products_validate_ready_queue_dlq_entrypoint_counts_and_crons() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&tmp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "products", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let queues = open_compute_storage::QueueRepository::new(storage.db());
    let source = open_compute_core::QueueId::generate();
    let dlq = open_compute_core::QueueId::generate();
    let pending = open_compute_core::QueueId::generate();
    for (id, name, ready) in [
        (source, "product-source", true),
        (dlq, "product-dlq", true),
        (pending, "product-pending", false),
    ] {
        queues
            .insert_creating(
                account,
                id,
                name,
                open_compute_storage::QueueConfig::default(),
                2,
            )
            .unwrap();
        if ready {
            queues.mark_ready(account, id, 3).unwrap();
        }
    }
    let mock = MockS3::spawn("open-compute").await;
    let validator: Arc<dyn RuntimeValidator> = Arc::new(AcceptAllValidator);
    let controller = VersionController::new(
        &storage,
        artifact_store(&mock),
        validator,
        BundleLimits::default(),
    )
    .with_queue_consumer_limit(2);
    let consumer = QueueConsumerInput {
        queue: source,
        entrypoint: Some("Named_$1".to_owned()),
        config: open_compute_storage::QueueConsumerConfig {
            max_concurrency: 2,
            ..open_compute_storage::QueueConsumerConfig::default()
        },
        dead_letter_queue: Some(dlq),
    };
    let mut valid = version_request(account, worker.id, "products-valid", "secret");
    valid.deployment_source = None;
    valid.queue_consumers = vec![consumer.clone()];
    valid.crons = vec!["*/5 * * * *".to_owned(), "*/5 * * * *".to_owned()];
    let version = match controller.create_version(valid).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("product version replayed"),
    };
    let declarations = open_compute_storage::QueueConsumerRepository::new(storage.db())
        .version_declarations(version.id)
        .unwrap();
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].dlq_queue_id, Some(dlq));
    assert_eq!(declarations[0].dlq_lifecycle_generation, Some(1));
    let cron = open_compute_storage::CronRepository::new(storage.db())
        .version_config(version.id)
        .unwrap();
    assert_eq!(cron.declarations.len(), 1);
    assert_eq!(cron.declarations[0].expression, "*/5 * * * *");

    let mut cases = Vec::new();
    let mut duplicate = version_request(account, worker.id, "products-duplicate", "secret");
    duplicate.deployment_source = None;
    duplicate.queue_consumers = vec![consumer.clone(), consumer.clone()];
    cases.push((duplicate, ErrorCode::QueueConsumerConflict));

    let mut self_dlq = version_request(account, worker.id, "products-self-dlq", "secret");
    self_dlq.deployment_source = None;
    self_dlq.queue_consumers = vec![QueueConsumerInput {
        dead_letter_queue: Some(source),
        ..consumer.clone()
    }];
    cases.push((self_dlq, ErrorCode::QueueDlqInvalid));

    let mut pending_dlq = version_request(account, worker.id, "products-pending-dlq", "secret");
    pending_dlq.deployment_source = None;
    pending_dlq.queue_consumers = vec![QueueConsumerInput {
        dead_letter_queue: Some(pending),
        ..consumer.clone()
    }];
    cases.push((pending_dlq, ErrorCode::QueueDlqInvalid));

    let mut bad_entry = version_request(account, worker.id, "products-entry", "secret");
    bad_entry.deployment_source = None;
    bad_entry.queue_consumers = vec![QueueConsumerInput {
        entrypoint: Some("1-invalid".to_owned()),
        ..consumer.clone()
    }];
    cases.push((bad_entry, ErrorCode::EntrypointNotFound));

    let mut not_ready = version_request(account, worker.id, "products-not-ready", "secret");
    not_ready.deployment_source = None;
    not_ready.queue_consumers = vec![QueueConsumerInput {
        queue: pending,
        dead_letter_queue: None,
        ..consumer.clone()
    }];
    cases.push((not_ready, ErrorCode::QueueConsumerNotReady));

    let mut invalid_config = version_request(account, worker.id, "products-config", "secret");
    invalid_config.deployment_source = None;
    invalid_config.queue_consumers = vec![QueueConsumerInput {
        config: open_compute_storage::QueueConsumerConfig {
            max_concurrency: 3,
            ..consumer.config
        },
        ..consumer.clone()
    }];
    cases.push((invalid_config, ErrorCode::LimitInvalid));

    let mut invalid_cron = version_request(account, worker.id, "products-cron", "secret");
    invalid_cron.deployment_source = None;
    invalid_cron.crons = vec!["not a cron".to_owned()];
    cases.push((invalid_cron, ErrorCode::CronExpressionInvalid));

    let mut too_many = version_request(account, worker.id, "products-count", "secret");
    too_many.deployment_source = None;
    too_many.queue_consumers = vec![consumer; 65];
    cases.push((too_many, ErrorCode::QuotaExceeded));

    for (request, expected) in cases {
        assert_eq!(
            controller.create_version(request).await.unwrap_err().code(),
            expected
        );
    }
}
