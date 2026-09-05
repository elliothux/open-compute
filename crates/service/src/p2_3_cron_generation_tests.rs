//! Cron generations survive removal of every trigger and a platform restart.

use super::*;
use open_compute_storage::{CronActivationState, CronRepository, WorkerRepository};

#[tokio::test]
async fn cron_remove_all_restart_and_reenable_preserves_generation_and_retry_identity() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let (account, worker, version, retired) = {
        let storage = Arc::new(
            open_compute_storage::PlatformStorage::bootstrap(
                &loaded.config.data,
                &open_compute_core::SystemClock,
            )
            .unwrap(),
        );
        let scheduler = Arc::new(
            SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 100, 1)
                .unwrap(),
        );
        let account = storage.identity().default_account_id;
        let worker = WorkerRepository::new(storage.db())
            .create_worker(
                account,
                "cron-cycle",
                open_compute_core::RequestId::generate(),
                1,
                1_000_000,
            )
            .unwrap()
            .0
            .id;
        let crons = CronRepository::new(storage.db());
        assert_eq!(crons.maximum_generation(worker).unwrap(), 0);
        let s3 = loaded.config.object_storage.as_s3().unwrap();
        let backend = open_compute_artifacts::ObjectBackend::connect_s3(
            s3,
            &resolve_fixture_s3_credentials(s3),
            loaded.config.cache.max_artifact_bytes,
        )
        .unwrap();
        let promoter = Arc::new(crate::p2_3_promotion::P23PromotionCoordinator::new(
            storage.clone(),
            scheduler,
            Duration::from_millis(100),
        ));
        let validator: Arc<dyn RuntimeValidator> =
            Arc::new(|_: ValidationCandidate| async { Ok(()) });
        let controller = VersionController::new(
            &storage,
            open_compute_artifacts::ArtifactStore::new(backend),
            validator,
            BundleLimits::default(),
        )
        .with_product_promoter(promoter);
        let bundle = CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".into(),
                module_type: ModuleType::EsModule,
                bytes: b"export default {fetch(){return new Response('ok');},scheduled(){}};"
                    .to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        let request = CreateVersionRequest {
            account_id: account,
            worker_id: worker,
            idempotency_key: "cron-on".into(),
            content: open_compute_workers::VersionContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: None,
            },
            vars: Default::default(),
            secrets: Default::default(),
            bindings: Default::default(),
            services: Default::default(),
            runtime_features: Default::default(),
            queue_consumers: Vec::new(),
            crons: vec!["*/5 * * * *".into()],
            deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_000,
        };
        let CreateVersionOutcome::Applied(result) =
            controller.create_version(request.clone()).await.unwrap()
        else {
            panic!("first version must be new")
        };
        let retired = crons.live_for_worker(worker).unwrap().remove(0);
        assert_eq!(retired.activation_generation, 1);
        assert_eq!(retired.state, CronActivationState::Active);
        controller
            .create_version(CreateVersionRequest {
                idempotency_key: "cron-off".into(),
                crons: Vec::new(),
                now_ms: 60_001,
                request_id: open_compute_core::RequestId::generate(),
                ..request
            })
            .await
            .unwrap();
        assert!(crons.live_for_worker(worker).unwrap().is_empty());
        assert_eq!(
            crons.activation(retired.id).unwrap().state,
            CronActivationState::Tombstoned
        );
        assert_eq!(crons.maximum_generation(worker).unwrap(), 1);
        (account, worker, result.version.id, retired)
    };

    // Drop both stores and all controllers before reopening the same authority.
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(SchedulerStore::open(&scheduler_path, 100, 60_002).unwrap());
    let promoter = crate::p2_3_promotion::P23PromotionCoordinator::new(
        storage.clone(),
        scheduler.clone(),
        Duration::from_millis(100),
    );
    let request = ProductPromotionRequest {
        account_id: account,
        worker_id: worker,
        version_id: version,
        source: open_compute_storage::DeploymentSource::Rollback,
        annotations: Default::default(),
        request_id: open_compute_core::RequestId::generate(),
        now_ms: 60_002,
    };
    promoter.promote(request.clone()).await.unwrap();
    let crons = CronRepository::new(storage.db());
    let active = crons.live_for_worker(worker).unwrap().remove(0);
    assert_eq!(active.expression, retired.expression);
    assert_eq!(active.activation_generation, 2);
    assert_ne!(active.id, retired.id);
    assert_eq!(active.state, CronActivationState::Active);
    assert_eq!(crons.maximum_generation(worker).unwrap(), 2);
    let epoch = scheduler
        .cron_execution_generation(active.id, 2)
        .unwrap()
        .unwrap();
    promoter.promote(request).await.unwrap();
    let reopened = Arc::new(SchedulerStore::open(&scheduler_path, 100, 60_003).unwrap());
    let service = SchedulerService::new(
        reopened.clone(),
        storage.clone(),
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
        SchedulerConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        Arc::new(open_compute_core::DeterministicSchedulerClock::new(60_003)),
    );
    service.repair_products(32).unwrap();
    let live = crons.live_for_worker(worker).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, active.id);
    assert_eq!(live[0].activation_generation, 2);
    assert_eq!(
        reopened.cron_execution_generation(active.id, 2).unwrap(),
        Some(epoch)
    );
    assert_eq!(
        reopened.cron_execution_generation(active.id, 1).unwrap(),
        None
    );
    assert_eq!(
        crons.activation(retired.id).unwrap().state,
        CronActivationState::Tombstoned
    );
}
