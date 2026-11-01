use super::*;
use crate::scheduler::SchedulerService;
use axum::{Json, Router, extract::State, routing::post};
use open_compute_core::SchedulerConfig;
use open_compute_workers::{
    CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput, ModuleType,
    QueueConsumerInput, RuntimeValidator, ValidationCandidate, VersionController,
};

#[derive(Clone)]
struct FakeCustomEventResponses {
    queue: Arc<Mutex<serde_json::Value>>,
    cron: Arc<Mutex<serde_json::Value>>,
}
async fn fake_queue_custom_event(
    State(responses): State<FakeCustomEventResponses>,
) -> Json<serde_json::Value> {
    let response = responses.queue.lock().unwrap().clone();
    if response["outcome"] == "pending" {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Json(response)
}
async fn fake_cron_custom_event(
    State(responses): State<FakeCustomEventResponses>,
) -> Json<serde_json::Value> {
    let response = responses.cron.lock().unwrap().clone();
    if response["outcome"] == "pending" {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Json(response)
}

struct Scenario<'fixture, 'storage> {
    controller: &'fixture VersionController<'storage>,
    target: &'fixture promotion_request::Target,
    storage: &'fixture Arc<open_compute_storage::PlatformStorage>,
    scheduler_store: &'fixture Arc<SchedulerStore>,
    worker: &'fixture open_compute_storage::worker_repository::WorkerRecord,
}

struct RuntimeFixture {
    first_consumer: open_compute_storage::queue_consumers::QueueConsumerRecord,
    responses: FakeCustomEventResponses,
    clock: Arc<open_compute_core::DeterministicSchedulerClock>,
    scheduler: Arc<SchedulerService>,
    custom_event_task: tokio::task::JoinHandle<()>,
    _custom_event_listener_dir: TempDir,
    custom_event_port: PathBuf,
}

#[tokio::test]
async fn scheduler_queue_cron_dispatch_and_operator_controls() {
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
    let scheduler_store = Arc::new(
        SchedulerStore::open(&scheduler_path, 100, 1, storage.identity().instance_id).unwrap(),
    );
    let account = storage.identity().instance_id;
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = open_compute_storage::queues::QueueConfig::default();
    let queues = open_compute_storage::queues::QueueRepository::new(storage.db());
    queues
        .insert_creating(account, queue_id, "promotion-queue", queue_config, 1)
        .unwrap();
    scheduler_store
        .create_queue_projection(&open_compute_storage::scheduler::QueueProjection {
            queue_id,
            instance_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config: queue_config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let workers = open_compute_storage::worker_repository::WorkerRepository::new(storage.db());
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
    let controller = VersionController::new(
        &storage,
        open_compute_artifacts::ArtifactStore::new(client),
        validator,
        BundleLimits::default(),
    );

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
        worker: &worker,
    };
    let runtime = establish_initial_products(&scenario).await;
    exercise_dispatch_and_operator_controls(&scenario, &runtime).await;
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
    let consumer_repo =
        open_compute_storage::queue_consumers::QueueConsumerRepository::new(storage.db());
    let first_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(
        first_consumer.state,
        open_compute_storage::queue_consumers::QueueConsumerState::Active
    );
    assert_eq!(first_consumer.version_id, first_id);
    assert!(
        scheduler_store
            .inspect_queue_consumer_runtime(queue_id, first_consumer.id, 1)
            .unwrap()
            .projection_exists
    );
    let first_crons = open_compute_storage::cron::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(first_crons.len(), 1);
    controller
        .promote(
            account,
            worker.id,
            first_id,
            open_compute_storage::worker_repository::DeploymentSource::VersionsApi,
            &std::collections::BTreeMap::new(),
            None,
            open_compute_core::RequestId::generate(),
            60_001,
        )
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
    let custom_event_listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let custom_event_listener_path = custom_event_listener_dir.path().join("runtime.sock");
    let custom_event_listener =
        tokio::net::UnixListener::bind(&custom_event_listener_path).unwrap();
    let custom_event_port = custom_event_listener_path.clone();
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

    let transport = WorkerdTransport::for_test_endpoint(&custom_event_port);
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
            &open_compute_storage::scheduler::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::scheduler::QueueMessageInput {
                    content_type: open_compute_storage::scheduler::QueueContentType::Json,
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

    let (kernel_shutdown, kernel_shutdown_rx) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(scheduler.clone().run(kernel_shutdown_rx));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = scheduler.inspect().unwrap();
            let queue_empty = scheduler_store.queue_backlog_totals().unwrap().0 == 0;
            let cron_complete = status
                .cron_activations
                .first()
                .and_then(|activation| activation.last_outcome)
                == Some(open_compute_storage::scheduler::CronRunState::Complete);
            if queue_empty && cron_complete {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "queue and Cron dispatch did not complete: {:?}, backlog={:?}",
            scheduler.inspect().unwrap(),
            scheduler_store.queue_backlog_totals().unwrap(),
        )
    });
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
            .and_then(|activation| activation.last_outcome),
        Some(open_compute_storage::scheduler::CronRunState::Complete)
    );
    kernel_shutdown.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    RuntimeFixture {
        first_consumer,
        responses,
        clock,
        scheduler,
        custom_event_task,
        _custom_event_listener_dir: custom_event_listener_dir,
        custom_event_port,
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
            &open_compute_storage::scheduler::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::scheduler::QueueMessageInput {
                    content_type: open_compute_storage::scheduler::QueueContentType::Text,
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
    assert!(scheduler.poll_once().await.unwrap() >= 1);
    clock.set_wall_time_ms(701_000);
    assert_eq!(scheduler.poll_once().await.unwrap(), 0);
    clock.set_wall_time_ms(706_000);
    for batch in scheduler_store
        .claim_queue_batches(706_000, 1, None)
        .unwrap()
    {
        scheduler.clone().dispatch_queue_batch(batch).await;
    }
    for run in scheduler_store.claim_cron_runs(706_000, 3, 1).unwrap() {
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
            .and_then(|activation| activation.last_outcome),
        Some(open_compute_storage::scheduler::CronRunState::Failed)
    );
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    exercise_dispatch_timeouts(scenario, runtime).await;
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
}

async fn exercise_dispatch_timeouts(scenario: &Scenario<'_, '_>, runtime: &RuntimeFixture) {
    use open_compute_storage::scheduler::{
        CronCompletion, QueueCompletionAction, QueueCompletionDecision,
    };
    let store = scenario.scheduler_store;
    let now = 1_205_000;
    store
        .enqueue_queue(
            &open_compute_storage::scheduler::QueueEnqueueRequest {
                queue_id: scenario.target.queue,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::scheduler::QueueMessageInput {
                    content_type: open_compute_storage::scheduler::QueueContentType::Text,
                    body: b"timeout".to_vec(),
                    delay_seconds: None,
                }],
            },
            now - 5_000,
        )
        .unwrap();
    runtime.clock.set_wall_time_ms(now);

    let config = SchedulerConfig {
        claim_lease_ms: 50,
        ..Default::default()
    };
    let scheduler = Arc::new(SchedulerService::new(
        Arc::clone(store),
        Arc::clone(scenario.storage),
        WorkerdTransport::for_test_endpoint(&runtime.custom_event_port),
        config,
        open_compute_core::WorkflowsConfig::default(),
        runtime.clock.clone(),
    ));
    let batch = scheduler.claim_queue_consumers(1).await.unwrap().remove(0);
    let run = scheduler.claim_cron(1).await.unwrap().remove(0);
    *runtime.responses.queue.lock().unwrap() = serde_json::json!({"outcome":"pending"});
    *runtime.responses.cron.lock().unwrap() = serde_json::json!({"outcome":"pending"});
    tokio::join!(
        scheduler.clone().dispatch_queue_batch(batch),
        scheduler.clone().dispatch_cron_run(run.clone())
    );
    assert_eq!(
        store
            .queue_consumer_workload_summary(now + 250)
            .unwrap()
            .claimed,
        0
    );
    let inspection = store
        .inspect_cron_runtime(run.activation_id, now + 250)
        .unwrap();
    assert_eq!(inspection.ready_runs, 1);
    assert_eq!(
        inspection.last_unknown_reason.as_deref(),
        Some("transport-timeout")
    );
    let batch = store
        .claim_queue_batches(now + 5_250, 1, None)
        .unwrap()
        .remove(0);
    assert_eq!(batch.messages[0].delivery_attempt, 2);
    store
        .complete_queue_batch(
            &batch,
            &[QueueCompletionDecision {
                message_id: batch.messages[0].id,
                action: QueueCompletionAction::Ack,
            }],
            now + 5_250,
        )
        .unwrap();
    let retry = store.claim_cron_runs(now + 250, 3, 1).unwrap().remove(0);
    assert_eq!(
        (retry.id, retry.attempt, retry.dispatch_deadline_at_ms),
        (run.id, 2, run.dispatch_deadline_at_ms)
    );
    store
        .complete_cron_run(&retry, CronCompletion::Success, now + 251, 3)
        .unwrap();
}

mod promotion_request {
    use super::*;

    pub(super) struct Target {
        pub(super) account: open_compute_core::InstanceId,
        pub(super) worker: open_compute_core::WorkerId,
        pub(super) queue: open_compute_core::QueueId,
    }

    pub(super) fn build(
        target: &Target,
        key: &str,
        label: &str,
        promote: bool,
        cron: &str,
        batch_size: u32,
    ) -> CreateVersionRequest {
        let source = format!(
            "export default {{ fetch() {{ return new Response('{label}'); }}, queue() {{}}, scheduled() {{}} }};"
        );
        let bundle = CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".to_owned(),
                module_type: ModuleType::EsModule,
                bytes: source.into_bytes(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        CreateVersionRequest {
            instance_id: target.account,
            worker_id: target.worker,
            idempotency_key: key.to_owned(),
            content: open_compute_workers::VersionContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: None,
            },
            vars: std::collections::BTreeMap::new(),
            secrets: std::collections::BTreeMap::new(),
            bindings: std::collections::BTreeMap::new(),
            services: std::collections::BTreeMap::new(),
            runtime_features: open_compute_workers::VersionRuntimeFeatures {
                compatibility_date: "2026-09-08".to_owned(),
                ..Default::default()
            },
            queue_consumers: vec![QueueConsumerInput {
                queue: target.queue,
                entrypoint: None,
                config: open_compute_storage::queue_consumers::QueueConsumerConfig {
                    max_batch_size: batch_size,
                    ..open_compute_storage::queue_consumers::QueueConsumerConfig::default()
                },
                dead_letter_queue: None,
            }],
            crons: vec![cron.to_owned()],
            deployment_source: promote
                .then_some(open_compute_storage::worker_repository::DeploymentSource::VersionsApi),
            observability: None,
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_000,
        }
    }

    pub(super) async fn remove_all_products(
        controller: &VersionController<'_>,
        target: &Target,
        storage: &Arc<open_compute_storage::PlatformStorage>,
        scheduler_path: &Path,
    ) {
        let mut request = build(target, "p23-empty", "empty", true, "ignored", 10);
        request.queue_consumers.clear();
        request.crons = Vec::new();
        let emptied_id = match controller.create_version(request).await.unwrap() {
            CreateVersionOutcome::Applied(result) => result.version.id,
            CreateVersionOutcome::Replay(_) => panic!("empty P2.3 version replayed"),
        };
        let workers = open_compute_storage::worker_repository::WorkerRepository::new(storage.db());
        assert_eq!(
            workers
                .get_worker(target.account, target.worker)
                .unwrap()
                .active_version_id,
            Some(emptied_id)
        );
        assert!(
            open_compute_storage::queue_consumers::QueueConsumerRepository::new(storage.db())
                .live_for_queue(target.queue)
                .unwrap()
                .is_none()
        );
        assert!(
            open_compute_storage::cron::CronRepository::new(storage.db())
                .live_for_worker(target.worker)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            open_compute_storage::scheduler::inspect_p23_cross_database(
                &storage.data_dir().control_db_path(),
                scheduler_path,
                100,
            )
            .unwrap(),
            open_compute_storage::scheduler::P23CrossDatabaseInspection::default()
        );
    }
}
