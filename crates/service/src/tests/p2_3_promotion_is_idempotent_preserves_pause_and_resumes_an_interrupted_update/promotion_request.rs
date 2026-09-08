use super::*;

pub(super) struct Target {
    pub(super) account: open_compute_core::AccountId,
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
        account_id: target.account,
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
        runtime_features: Default::default(),
        queue_consumers: vec![QueueConsumerInput {
            queue: target.queue,
            entrypoint: None,
            config: open_compute_storage::QueueConsumerConfig {
                max_batch_size: batch_size,
                ..open_compute_storage::QueueConsumerConfig::default()
            },
            dead_letter_queue: None,
        }],
        crons: vec![cron.to_owned()],
        deployment_source: promote.then_some(open_compute_storage::DeploymentSource::VersionsApi),
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
    let workers = open_compute_storage::WorkerRepository::new(storage.db());
    assert_eq!(
        workers
            .get_worker(target.account, target.worker)
            .unwrap()
            .active_version_id,
        Some(emptied_id)
    );
    assert!(
        open_compute_storage::QueueConsumerRepository::new(storage.db())
            .live_for_queue(target.queue)
            .unwrap()
            .is_none()
    );
    assert!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(target.worker)
            .unwrap()
            .is_empty()
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
