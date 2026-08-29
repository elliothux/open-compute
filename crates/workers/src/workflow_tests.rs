use super::*;
use open_compute_core::{DeploymentId, RequestId, StorageConfig, SystemClock};
use open_compute_storage::{NewDeployment, WorkerRepository};

#[path = "workflow_crash_tests.rs"]
mod crash_matrix;
#[path = "workflow_lifecycle_tests.rs"]
mod durable_lifecycle;

fn fixture() -> (
    tempfile::TempDir,
    PlatformStorage,
    SchedulerStore,
    WorkflowId,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5000,
        free_space_soft_bytes: 1024 * 1024 * 1024,
        free_space_hard_bytes: 256 * 1024 * 1024,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler =
        SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 5000, 0).unwrap();
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "workflow-owner",
            RequestId::generate(),
            0,
            1_000_000,
        )
        .unwrap();
    let deployment = DeploymentId::generate();
    workers
        .insert_staging_deployment(
            &NewDeployment {
                id: deployment,
                account_id: account,
                worker_id: worker.id,
                artifact_sha256: [1; 32],
                artifact_size: 100,
                artifact_schema_version: 1,
                main_module: "index.js".into(),
                compatibility_date: "2026-08-26".into(),
                compatibility_flags: vec![],
                limits: serde_json::json!({"profile":"default"}),
                worker_code_sha256: [2; 32],
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 0,
            },
            &open_compute_storage::NewDeploymentProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 1).unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows.create_definition(account, "flow", 2).unwrap();
    let version = workflows
        .stage_version(account, definition.id, deployment, "Flow", 3)
        .unwrap();
    workflows
        .finish_version(account, version.target.version_id, true, 4)
        .unwrap();
    (temp, storage, scheduler, definition.id)
}
