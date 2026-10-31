//! Cloudflare v4 HTTP surface sharing the real P0.2 runtime.

use super::*;

pub(super) async fn api_matrix(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    transport: WorkerdTransport,
    account: open_compute_core::InstanceId,
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
    let secrets = storage.data_dir().root().join("cf-auth");
    std::fs::create_dir(&secrets).unwrap();
    let server = open_compute_core::DaemonServerConfig {
        admin_auth: token_reference(&secrets.join("admin.token"), ADMIN_TOKEN),
        ..open_compute_core::DaemonServerConfig::default()
    };
    let auth = InstanceAuthConfig {
        deployer_auth: token_reference(&secrets.join("deployer.token"), CF_TOKEN),
        read_only_auth: token_reference(&secrets.join("read-only.token"), READ_ONLY_TOKEN),
    };
    let workflow_api = WorkflowApiState::new(
        storage.clone(),
        scheduler.clone(),
        transport.clone(),
        open_compute_core::WorkflowsConfig::default(),
    );

    let state = HttpState::new(health, metrics, false, false, &server, &auth)
        .unwrap()
        .with_worker_api(WorkerApiState::new(
            storage.clone(),
            artifacts,
            transport.with_test_request_body_limit(32 * 1024),
            VersionPins::new(),
            BundleLimits::default(),
            Duration::from_secs(5),
        ))
        .with_workflow_api(Some(workflow_api));
    let (state, public_account) =
        open_compute_service::cloudflare_v4_for_test(state, storage.clone());
    cf::exercise(state, storage, account, &public_account, CF_TOKEN).await;
}

fn token_reference(path: &Path, value: &str) -> SecretReference {
    write_secret(path, value);
    SecretReference {
        env: None,
        file: Some(path.to_owned()),
    }
}

const CF_TOKEN: &str = "p0-2-cf-deployer-secret";
const READ_ONLY_TOKEN: &str = "p0-2-read-only-secret";

pub(super) async fn cron_generation_cycle(
    controller: &VersionController<'_>,
    storage: &PlatformStorage,
    transport: &WorkerdTransport,
    account: open_compute_core::InstanceId,
    worker: open_compute_core::WorkerId,
) {
    let crons = open_compute_storage::cron::CronRepository::new(storage.db());
    let previous = crons.live_for_worker(worker).unwrap();
    assert!(!previous.is_empty());
    let mut empty = create_request(account, worker, "cron-off", "C", true, false);
    empty.crons.clear();
    controller.create_version(empty).await.unwrap();
    assert!(crons.live_for_worker(worker).unwrap().is_empty());
    let restored = deploy(controller, account, worker, "cron-on", "D", None).await;
    let live = crons.live_for_worker(worker).unwrap();
    assert_eq!(live.len(), 3);
    assert!(
        live.iter()
            .all(|activation| { !previous.iter().any(|old| old.id == activation.id) })
    );
    let activation = live
        .iter()
        .find(|activation| activation.expression == "*/5 * * * *")
        .unwrap();
    let target = dispatch_target(storage, account, worker, &restored, None);
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
