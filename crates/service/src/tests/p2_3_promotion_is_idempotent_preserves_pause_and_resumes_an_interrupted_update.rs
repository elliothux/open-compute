use super::*;

struct Scenario<'fixture, 'storage> {
    controller: &'fixture VersionController<'storage>,
    target: &'fixture promotion_request::Target,
    storage: &'fixture Arc<open_compute_storage::PlatformStorage>,
    scheduler_store: &'fixture Arc<SchedulerStore>,
    promoter: &'fixture Arc<crate::p2_3_promotion::P23PromotionCoordinator>,
    scheduler_path: &'fixture Path,
    worker: &'fixture open_compute_storage::WorkerRecord,
}

struct RuntimeFixture {
    first_consumer: open_compute_storage::QueueConsumerRecord,
    responses: FakeCustomEventResponses,
    clock: Arc<open_compute_core::DeterministicSchedulerClock>,
    scheduler: Arc<SchedulerService>,
    custom_event_task: tokio::task::JoinHandle<()>,
}

#[tokio::test]
async fn p2_3_promotion_is_idempotent_preserves_pause_and_resumes_an_interrupted_update() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler_store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 1).unwrap());
    let account = storage.identity().default_account_id;
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = open_compute_storage::QueueConfig::default();
    let queues = open_compute_storage::QueueRepository::new(storage.db());
    queues
        .insert_creating(account, queue_id, "promotion-queue", queue_config, 1)
        .unwrap();
    scheduler_store
        .create_queue_projection(&open_compute_storage::QueueProjection {
            queue_id,
            account_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config: queue_config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let workers = open_compute_storage::WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "p2-3-promotion",
            open_compute_core::RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap();
    let s3 = loaded.config.object_storage.as_s3().expect("S3 config");
    let credentials = resolve_fixture_s3_credentials(s3);
    let client = open_compute_artifacts::ObjectBackend::connect_s3(
        s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_: ValidationCandidate| async { Ok(()) });
    let promoter = Arc::new(crate::p2_3_promotion::P23PromotionCoordinator::new(
        storage.clone(),
        scheduler_store.clone(),
        Duration::from_millis(100),
    ));
    let controller = VersionController::new(
        &storage,
        open_compute_artifacts::ArtifactStore::new(client),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(promoter.clone());

    let request_target = promotion_request::Target {
        account,
        worker: worker.id,
        queue: queue_id,
    };

    let scenario = Scenario {
        controller: &controller,
        target: &request_target,
        storage: &storage,
        scheduler_store: &scheduler_store,
        promoter: &promoter,
        scheduler_path: &scheduler_path,
        worker: &worker,
    };
    let runtime = establish_initial_products(&scenario).await;
    exercise_dispatch_and_operator_controls(&scenario, &runtime).await;
    exercise_interrupted_update_recovery(&scenario, &runtime).await;
    retarget::exercise_retarget_and_repair(&scenario, &runtime).await;
    promotion_request::remove_all_products(&controller, &request_target, &storage, &scheduler_path)
        .await;
    runtime.custom_event_task.abort();
    let _ = runtime.custom_event_task.await;
}

async fn establish_initial_products(scenario: &Scenario<'_, '_>) -> RuntimeFixture {
    let Scenario {
        controller,
        target: request_target,
        storage,
        scheduler_store,
        promoter,
        worker,
        ..
    } = scenario;
    let account = request_target.account;
    let queue_id = request_target.queue;
    let first = controller
        .create_version(promotion_request::build(
            request_target,
            "p23-first",
            "first",
            true,
            "*/5 * * * *",
            10,
        ))
        .await
        .unwrap();
    let first_id = match first {
        CreateVersionOutcome::Applied(result) => result.version.id,
        CreateVersionOutcome::Replay(_) => panic!("first P2.3 version replayed"),
    };
    let consumer_repo = open_compute_storage::QueueConsumerRepository::new(storage.db());
    let first_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(
        first_consumer.state,
        open_compute_storage::QueueConsumerState::Active
    );
    assert_eq!(first_consumer.version_id, first_id);
    assert!(
        scheduler_store
            .inspect_queue_consumer_runtime(queue_id, first_consumer.id, 1)
            .unwrap()
            .projection_exists
    );
    let first_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(first_crons.len(), 1);
    assert_eq!(
        first_crons[0].state,
        open_compute_storage::CronActivationState::Active
    );

    promoter
        .promote(ProductPromotionRequest {
            account_id: account,
            worker_id: worker.id,
            version_id: first_id,
            source: open_compute_storage::DeploymentSource::VersionsApi,
            annotations: std::collections::BTreeMap::new(),
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_001,
        })
        .await
        .unwrap();
    assert_eq!(
        consumer_repo
            .live_for_queue(queue_id)
            .unwrap()
            .unwrap()
            .consumer_generation,
        1
    );

    let responses = FakeCustomEventResponses {
        queue: Arc::new(Mutex::new(serde_json::json!({
            "outcome": "ok",
            "ackAll": true,
            "retryBatch": {"retry": false},
            "explicitAcks": [],
            "retryMessages": []
        }))),
        cron: Arc::new(Mutex::new(serde_json::json!({
            "outcome": "ok",
            "noRetry": false
        }))),
    };
    let custom_event_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let custom_event_port = custom_event_listener.local_addr().unwrap().port();
    let server_responses = responses.clone();
    let custom_event_task = tokio::spawn(async move {
        axum::serve(
            custom_event_listener,
            Router::new()
                .route("/internal/queue", post(fake_queue_custom_event))
                .route("/internal/scheduled", post(fake_cron_custom_event))
                .with_state(server_responses)
                .into_make_service(),
        )
        .await
        .unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("11".repeat(32)));
    let transport = WorkerdTransport::for_test_endpoint(auth, custom_event_port);
    let clock = Arc::new(open_compute_core::DeterministicSchedulerClock::new(300_000));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "test").unwrap());
    let scheduler = Arc::new(
        SchedulerService::new(
            Arc::clone(*scheduler_store),
            Arc::clone(*storage),
            transport,
            SchedulerConfig::default(),
            open_compute_core::WorkflowsConfig::default(),
            clock.clone(),
        )
        .with_metrics(metrics),
    );
    scheduler_store
        .enqueue_queue(
            &open_compute_storage::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::QueueMessageInput {
                    content_type: open_compute_storage::QueueContentType::Json,
                    body: br#"{"event":"first"}"#.to_vec(),
                    delay_seconds: None,
                }],
            },
            300_000,
        )
        .unwrap();
    clock.set_wall_time_ms(305_000);
    let before_dispatch = scheduler.inspect().unwrap();
    assert_eq!(before_dispatch.queue_consumers.len(), 1);
    assert_eq!(before_dispatch.cron_activations.len(), 1);
    for kind in [
        SchedulerKind::Alarm,
        SchedulerKind::Queue,
        SchedulerKind::Cron,
        SchedulerKind::Workflow,
    ] {
        scheduler.pause_kind(kind).unwrap();
        assert!(scheduler.is_kind_paused(kind).unwrap());
        scheduler.resume_kind(kind).unwrap();
        assert!(!scheduler.is_kind_paused(kind).unwrap());
    }
    assert!(!scheduler.is_kind_paused(SchedulerKind::Workflow).unwrap());
    assert_eq!(
        scheduler.repair_products(0).unwrap_err().code(),
        ErrorCode::SchedulerUnavailable
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);

    let (kernel_shutdown, kernel_shutdown_rx) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(scheduler.clone().run(kernel_shutdown_rx));
    for _ in 0..10_000 {
        let queue_empty = scheduler_store.queue_backlog_totals().unwrap().0 == 0;
        let cron_complete = scheduler
            .inspect()
            .unwrap()
            .cron_activations
            .first()
            .and_then(|activation| activation.last_outcome.as_deref())
            == Some("complete");
        if queue_empty && cron_complete {
            break;
        }
        tokio::task::yield_now().await;
    }
    let dispatched = scheduler.inspect().unwrap();
    assert_eq!(
        scheduler_store.queue_backlog_totals().unwrap(),
        (0, 0),
        "{dispatched:?}"
    );
    assert_eq!(
        scheduler
            .inspect()
            .unwrap()
            .cron_activations
            .first()
            .and_then(|activation| activation.last_outcome.as_deref()),
        Some("complete")
    );
    kernel_shutdown.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    RuntimeFixture {
        first_consumer,
        responses,
        clock,
        scheduler,
        custom_event_task,
    }
}

async fn exercise_dispatch_and_operator_controls(
    scenario: &Scenario<'_, '_>,
    runtime: &RuntimeFixture,
) {
    let storage = scenario.storage;
    let scheduler_store = scenario.scheduler_store;
    let queue_id = scenario.target.queue;
    let first_consumer = &runtime.first_consumer;
    let responses = &runtime.responses;
    let clock = &runtime.clock;
    let scheduler = &runtime.scheduler;
    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "exception",
        "ackAll": false,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "exception",
        "noRetry": false
    });
    clock.set_wall_time_ms(600_000);
    scheduler_store
        .enqueue_queue(
            &open_compute_storage::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::QueueMessageInput {
                    content_type: open_compute_storage::QueueContentType::Text,
                    body: b"retry".to_vec(),
                    delay_seconds: None,
                }],
            },
            600_000,
        )
        .unwrap();
    clock.set_wall_time_ms(605_000);
    let [retry_batch] = scheduler
        .claim_queue_consumers(1)
        .await
        .unwrap()
        .try_into()
        .unwrap();
    let [retry_run] = scheduler.claim_cron(1).await.unwrap().try_into().unwrap();
    scheduler
        .clone()
        .dispatch_queue_batch(retry_batch.clone())
        .await;
    scheduler.clone().dispatch_cron_run(retry_run.clone()).await;
    assert_eq!(scheduler_store.queue_backlog_totals().unwrap().0, 1);

    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "ackAll": true,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    scheduler
        .clone()
        .dispatch_queue_batch(retry_batch.clone())
        .await;
    scheduler.clone().dispatch_cron_run(retry_run.clone()).await;

    let mut missing_queue_authority = retry_batch.clone();
    missing_queue_authority.worker_id = open_compute_core::WorkerId::generate();
    scheduler
        .clone()
        .dispatch_queue_batch(missing_queue_authority)
        .await;
    let mut missing_queue_version = retry_batch.clone();
    missing_queue_version.version_id = open_compute_core::VersionId::generate();
    scheduler
        .clone()
        .dispatch_queue_batch(missing_queue_version)
        .await;
    let mut invalid_queue_generation = retry_batch.clone();
    invalid_queue_generation.execution_generation = u64::MAX;
    scheduler
        .clone()
        .dispatch_queue_batch(invalid_queue_generation)
        .await;
    let mut missing_cron_authority = retry_run.clone();
    missing_cron_authority.worker_id = open_compute_core::WorkerId::generate();
    scheduler
        .clone()
        .dispatch_cron_run(missing_cron_authority)
        .await;
    let mut invalid_cron_generation = retry_run.clone();
    invalid_cron_generation.execution_generation = u64::MAX;
    scheduler
        .clone()
        .dispatch_cron_run(invalid_cron_generation)
        .await;

    clock.set_wall_time_ms(610_000);
    let [unknown_batch] = scheduler
        .claim_queue_consumers(1)
        .await
        .unwrap()
        .try_into()
        .unwrap();
    let [unknown_run] = scheduler.claim_cron(1).await.unwrap().try_into().unwrap();
    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "ackAll": false,
        "retryBatch": {"retry": false},
        "explicitAcks": [open_compute_core::QueueMessageId::generate().to_string()],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "aborted",
        "noRetry": false
    });
    scheduler
        .clone()
        .dispatch_queue_batch(unknown_batch.clone())
        .await;
    scheduler
        .clone()
        .dispatch_cron_run(unknown_run.clone())
        .await;
    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "aborted",
        "ackAll": false,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    scheduler
        .clone()
        .dispatch_queue_batch(unknown_batch.clone())
        .await;
    *responses.queue.lock().unwrap() = serde_json::json!({"outcome": "forged"});
    *responses.cron.lock().unwrap() = serde_json::json!({"outcome": "ok"});
    scheduler.clone().dispatch_queue_batch(unknown_batch).await;
    scheduler.clone().dispatch_cron_run(unknown_run).await;

    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "ackAll": true,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    clock.set_wall_time_ms(700_000);
    assert_eq!(scheduler.poll_once().await.unwrap(), 0);
    clock.set_wall_time_ms(701_000);
    assert!(scheduler.poll_once().await.unwrap() >= 1);
    clock.set_wall_time_ms(706_000);
    for batch in scheduler_store
        .claim_queue_batches(706_000, 60_000, 250, 1, None)
        .map(|(items, _)| items)
        .unwrap()
    {
        scheduler.clone().dispatch_queue_batch(batch).await;
    }
    for run in scheduler_store
        .claim_cron_runs(706_000, 60_000, 250, 1)
        .map(|(items, _)| items)
        .unwrap()
    {
        scheduler.clone().dispatch_cron_run(run).await;
    }
    assert_eq!(scheduler_store.queue_backlog_totals().unwrap(), (0, 0));
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "exception",
        "noRetry": true
    });
    clock.set_wall_time_ms(900_000);
    let [terminal_run] = scheduler.claim_cron(1).await.unwrap().try_into().unwrap();
    scheduler.clone().dispatch_cron_run(terminal_run).await;
    assert_eq!(
        scheduler
            .inspect()
            .unwrap()
            .cron_activations
            .first()
            .and_then(|activation| activation.last_outcome.as_deref()),
        Some("failed")
    );
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    let audit_count = || {
        let connection = rusqlite::Connection::open(storage.data_dir().control_db_path()).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM control_audit_events", [], |row| {
                row.get(0)
            })
            .unwrap();
        u64::try_from(count).unwrap()
    };
    let audit_before = audit_count();
    for result in [
        scheduler.pause_queue_consumer_operator(
            first_consumer.id,
            2,
            open_compute_core::RequestId::generate(),
        ),
        scheduler.resume_queue_consumer_operator(
            first_consumer.id,
            2,
            open_compute_core::RequestId::generate(),
        ),
    ] {
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::QueueConsumerGenerationStale
        );
    }
    scheduler
        .pause_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    scheduler
        .pause_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    assert_eq!(audit_count(), audit_before + 1);
    scheduler
        .resume_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    scheduler
        .resume_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    scheduler
        .pause_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    assert_eq!(audit_count(), audit_before + 3);
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
}

async fn exercise_interrupted_update_recovery(
    scenario: &Scenario<'_, '_>,
    runtime: &RuntimeFixture,
) {
    let Scenario {
        controller,
        target: request_target,
        storage,
        promoter,
        scheduler_path,
        worker,
        ..
    } = scenario;
    let queue_id = request_target.queue;
    let account = request_target.account;
    let scheduler = &runtime.scheduler;
    let consumer_repo = open_compute_storage::QueueConsumerRepository::new(storage.db());
    let workers = open_compute_storage::WorkerRepository::new(storage.db());
    let second = controller
        .create_version(promotion_request::build(
            request_target,
            "p23-second",
            "second",
            true,
            "0 * * * *",
            20,
        ))
        .await
        .unwrap();
    let second_id = match second {
        CreateVersionOutcome::Applied(result) => result.version.id,
        CreateVersionOutcome::Replay(_) => panic!("second P2.3 version replayed"),
    };
    let second_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(second_consumer.consumer_generation, 2);
    assert_eq!(second_consumer.version_id, second_id);
    assert_eq!(
        second_consumer.state,
        open_compute_storage::QueueConsumerState::Paused
    );

    let third = controller
        .create_version(promotion_request::build(
            request_target,
            "p23-third",
            "third",
            false,
            "30 * * * *",
            30,
        ))
        .await
        .unwrap();
    let third_id = match third {
        CreateVersionOutcome::Applied(result) => result.version.id,
        CreateVersionOutcome::Replay(_) => panic!("third P2.3 version replayed"),
    };
    let third_declaration = consumer_repo
        .version_declarations(third_id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert!(
        consumer_repo
            .begin_update(second_consumer.id, 2, worker.id, &third_declaration, 60_002,)
            .unwrap()
    );
    for result in [
        scheduler.pause_queue_consumer_operator(
            second_consumer.id,
            3,
            open_compute_core::RequestId::generate(),
        ),
        scheduler.resume_queue_consumer_operator(
            second_consumer.id,
            3,
            open_compute_core::RequestId::generate(),
        ),
    ] {
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::QueueConsumerGenerationStale
        );
    }
    assert!(scheduler.repair_products(1_000).unwrap() > 0);
    let reconciled = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(
        reconciled.state,
        open_compute_storage::QueueConsumerState::Updating
    );
    assert_eq!(reconciled.version_id, third_id);
    assert_eq!(reconciled.pending_version_id, None);
    let pre_promote_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(pre_promote_crons.len(), 1);
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .retire_before(
                worker.id,
                pre_promote_crons[0].activation_generation + 1,
                60_003,
            )
            .unwrap(),
        1
    );
    assert!(scheduler.repair_products(1_000).unwrap() > 0);
    workers
        .promote(
            account,
            worker.id,
            third_id,
            Some(second_id),
            open_compute_core::RequestId::generate(),
            60_003,
        )
        .unwrap();
    assert!(scheduler.repair_products(1_000).unwrap() > 0);
    assert_eq!(
        consumer_repo
            .live_for_queue(queue_id)
            .unwrap()
            .unwrap()
            .state,
        open_compute_storage::QueueConsumerState::Paused
    );
    promoter
        .promote(ProductPromotionRequest {
            account_id: account,
            worker_id: worker.id,
            version_id: third_id,
            source: open_compute_storage::DeploymentSource::VersionsApi,
            annotations: std::collections::BTreeMap::new(),
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_003,
        })
        .await
        .unwrap();
    let recovered = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(recovered.consumer_generation, 3);
    assert_eq!(recovered.version_id, third_id);
    assert_eq!(
        recovered.state,
        open_compute_storage::QueueConsumerState::Paused
    );
    assert_eq!(
        workers
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(third_id)
    );
    let live_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(live_crons.len(), 1);
    assert_eq!(live_crons[0].expression, "30 * * * *");
    assert_eq!(
        live_crons[0].state,
        open_compute_storage::CronActivationState::Active
    );
    assert_eq!(
        open_compute_storage::inspect_p23_cross_database(
            &storage.data_dir().control_db_path(),
            scheduler_path,
            100,
        )
        .unwrap(),
        open_compute_storage::P23CrossDatabaseInspection::default()
    );
}

mod promotion_request;
mod retarget;
