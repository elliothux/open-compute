//! Shared Workflow unit-test authority fixture.

use super::*;
use crate::metrics::MetricsRegistry;
use open_compute_core::{MetricsConfig, RequestId, StorageConfig, SystemClock};
use open_compute_storage::{NewVersion, WorkerRepository};

pub(crate) struct Fixture {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) storage: Arc<PlatformStorage>,
    pub(crate) scheduler: Arc<SchedulerStore>,
    pub(crate) account: AccountId,
    pub(crate) version: VersionId,
    pub(crate) metrics: Arc<MetricsRegistry>,
}

pub(crate) fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .expect("storage"),
    );
    let scheduler = Arc::new(
        SchedulerStore::open(
            &storage
                .data_dir()
                .ensure_scheduler_db()
                .expect("scheduler path"),
            5000,
            0,
        )
        .expect("scheduler"),
    );
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "workflow-api", RequestId::generate(), 0, 1_000_000)
        .expect("worker");
    let version = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: open_compute_storage::VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 0,
            },
            &open_compute_storage::NewVersionProducts::default(),
            1_000_000,
        )
        .expect("version");
    workers.begin_validation(version).expect("validating");
    workers.mark_ready(version, 1).expect("ready");
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").expect("metrics"),
    );
    Fixture {
        _temp: temp,
        storage,
        scheduler,
        account,
        version,
        metrics,
    }
}
