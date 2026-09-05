//! Cloudflare v4 HTTP surface sharing the real P0.2 runtime.

use super::*;

pub(super) async fn api_matrix(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    transport: WorkerdTransport,
    account: open_compute_core::AccountId,
    scheduler: Arc<SchedulerStore>,
) {
    let health = HealthCoordinator::new();
    for component in [
        ComponentName::Process,
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::ObjectStorage,
        ComponentName::Cache,
        ComponentName::Runtime,
    ] {
        health
            .set_component(
                component,
                ComponentState::Healthy,
                Some(ReadinessReason::Ready),
            )
            .unwrap();
    }
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "gate", "gate").unwrap());
    let secrets = storage.data_dir().root().join("wrangler-auth");
    std::fs::create_dir(&secrets).unwrap();
    let server = ServerConfig {
        admin_auth: token_reference(&secrets.join("admin.token"), ADMIN_TOKEN),
        deployer_auth: token_reference(&secrets.join("deployer.token"), WRANGLER_TOKEN),
        read_only_auth: token_reference(&secrets.join("read-only.token"), READ_ONLY_TOKEN),
        ..ServerConfig::default()
    };
    let workflow_api = WorkflowApiState::new(
        storage.clone(),
        scheduler,
        transport.clone(),
        open_compute_core::WorkflowsConfig::default(),
    );
    let state = HttpState::new(health, metrics, false, false, &server)
        .unwrap()
        .with_worker_api(WorkerApiState::new(
            storage.clone(),
            artifacts,
            transport.with_max_request_body(32 * 1024),
            VersionPins::new(),
            BundleLimits::default(),
            Duration::from_secs(5),
        ))
        .with_workflow_api(Some(workflow_api));
    let (state, public_account) =
        open_compute_service::cloudflare_v4_for_test(state, storage.clone());
    wrangler::exercise(
        merged_router(state),
        storage,
        account,
        &public_account,
        WRANGLER_TOKEN,
    )
    .await;
}

fn token_reference(path: &Path, value: &str) -> SecretReference {
    write_secret(path, value);
    SecretReference {
        env: None,
        file: Some(path.to_owned()),
    }
}

const WRANGLER_TOKEN: &str = "p0-2-wrangler-deployer-secret";
const READ_ONLY_TOKEN: &str = "p0-2-read-only-secret";

pub(super) async fn cron_generation_cycle(
    controller: &VersionController<'_>,
    storage: &PlatformStorage,
    transport: &WorkerdTransport,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
) {
    let crons = open_compute_storage::CronRepository::new(storage.db());
    let previous = crons.maximum_generation(worker).unwrap();
    assert!(previous > 0);
    let mut empty = create_request(account, worker, "cron-off", "C", true, false);
    empty.crons.clear();
    controller.create_version(empty).await.unwrap();
    assert!(crons.live_for_worker(worker).unwrap().is_empty());
    assert_eq!(crons.maximum_generation(worker).unwrap(), previous);
    let restored = deploy(controller, account, worker, "cron-on", "D", true, false).await;
    let live = crons.live_for_worker(worker).unwrap();
    assert_eq!(live.len(), 3);
    assert!(live.iter().all(|activation| {
        activation.activation_generation == previous + 1
            && activation.state == open_compute_storage::CronActivationState::Active
    }));
    let activation = live
        .iter()
        .find(|activation| activation.expression == "*/5 * * * *")
        .unwrap();
    let mut target = dispatch_target(account, worker, &restored, None);
    target.route_generation = i64::try_from(
        WorkerRepository::new(storage.db())
            .get_worker(account, worker)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let result = transport
        .dispatch_scheduled(
            &target,
            &ScheduledDispatchRequest {
                scheduled_time_ms: 1_787_700_060_000,
                cron: activation.expression.clone(),
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(result.outcome, "ok");
    assert!(result.no_retry);
}
