//! Route edits cannot replace the persisted dispatch epoch of Queue or Cron work.

use super::*;

#[tokio::test]
async fn route_edits_preserve_queue_and_cron_epochs_during_repromotion_and_restart_reconcile() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 1).unwrap());
    let account = storage.identity().default_account_id;
    let open_compute_workers::CreateQueueOutcome::Applied(queue) =
        open_compute_workers::QueueController::new(&storage, store.clone())
            .create(&open_compute_workers::CreateQueueRequest {
                account_id: account,
                name: "epoch-queue".into(),
                config: Default::default(),
                idempotency_key: "epoch-queue".into(),
                request_id: open_compute_core::RequestId::generate(),
                now_ms: 1,
            })
            .unwrap()
    else {
        panic!("queue must be new")
    };
    let workers = open_compute_storage::WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(
            account,
            "epoch-worker",
            open_compute_core::RequestId::generate(),
            1,
        )
        .unwrap()
        .0;
    let credentials = open_compute_artifacts::resolve_s3_credentials(&loaded.config.s3).unwrap();
    let client = open_compute_artifacts::S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let promoter = Arc::new(crate::p2_3_promotion::P23PromotionCoordinator::new(
        storage.clone(),
        store.clone(),
        Duration::from_millis(100),
    ));
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_: ValidationCandidate| async { Ok(()) });
    let controller = DeploymentController::new(
        &storage,
        open_compute_artifacts::ArtifactStore::new(client),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(promoter.clone());
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".into(),
            module_type: ModuleType::EsModule,
            bytes: b"export default {fetch(){return new Response('ok');},queue(){},scheduled(){}};"
                .to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let CreateDeploymentOutcome::Applied(result) = controller
        .create_deployment(CreateDeploymentRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: "epoch-deployment".into(),
            bundle: bundle.into_bytes().into(),
            compatibility_date: "2026-08-22".into(),
            compatibility_flags: vec![],
            vars: Default::default(),
            secrets: Default::default(),
            bindings: Default::default(),
            queue_consumers: vec![QueueConsumerInput {
                queue: queue.queue.id,
                entrypoint: None,
                config: Default::default(),
                dead_letter_queue: None,
            }],
            crons: Some(vec!["*/5 * * * *".into()]),
            limits: serde_json::json!({"profile":"default"}),
            promote: true,
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_000,
        })
        .await
        .unwrap()
    else {
        panic!("deployment must be new")
    };
    let consumer = open_compute_storage::QueueConsumerRepository::new(storage.db())
        .live_for_queue(queue.queue.id)
        .unwrap()
        .unwrap();
    let activation = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap()
        .remove(0);
    let queue_epoch = store
        .queue_consumer_execution_generation(consumer.id, consumer.consumer_generation)
        .unwrap()
        .unwrap();
    let cron_epoch = store
        .cron_execution_generation(activation.id, activation.activation_generation)
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .queue_consumer_execution_generation(consumer.id, consumer.consumer_generation + 1)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .cron_execution_generation(activation.id, activation.activation_generation + 1)
            .unwrap(),
        None
    );
    workers
        .create_exact_route(
            account,
            worker.id,
            "epoch.example",
            "/",
            None,
            None,
            open_compute_core::RequestId::generate(),
            60_001,
        )
        .unwrap();
    let worker = workers.get_worker(account, worker.id).unwrap();
    assert!(worker.route_generation > queue_epoch);
    assert!(worker.route_generation > cron_epoch);
    promoter
        .promote(ProductPromotionRequest {
            account_id: account,
            worker_id: worker.id,
            deployment_id: result.deployment.id,
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_002,
        })
        .await
        .unwrap();
    let reopened = Arc::new(SchedulerStore::open(&scheduler_path, 100, 60_003).unwrap());
    let service = SchedulerService::new(
        reopened.clone(),
        storage.clone(),
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
        SchedulerConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        Arc::new(open_compute_core::DeterministicSchedulerClock::new(60_003)),
    );
    assert!(service.repair_products(32).unwrap() >= 2);
    assert_eq!(
        reopened
            .queue_consumer_execution_generation(consumer.id, consumer.consumer_generation)
            .unwrap(),
        Some(queue_epoch)
    );
    assert_eq!(
        reopened
            .cron_execution_generation(activation.id, activation.activation_generation)
            .unwrap(),
        Some(cron_epoch)
    );
    assert_eq!(
        open_compute_storage::QueueConsumerRepository::new(storage.db())
            .get(consumer.id)
            .unwrap()
            .consumer_generation,
        consumer.consumer_generation
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(worker.id)
            .unwrap()[0]
            .activation_generation,
        activation.activation_generation
    );
}
