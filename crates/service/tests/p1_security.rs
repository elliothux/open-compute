//! P1 security parser/path and release-artifact hygiene Gate.

use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{
    AccountId, BindingKind, DeploymentId, ErrorCode, PlatformStatus, RequestId, ResourceId,
    SystemClock, valid_restore_path,
};
use open_compute_service::metrics::MetricsRegistry;
use open_compute_storage::{
    NewDeployment, PlatformStorage, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, WorkerRepository,
};
use rusqlite::params;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace")
        .to_path_buf()
}

fn production_sources(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("read source tree") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            production_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "tests.rs" || name.ends_with("_tests.rs"))
            && !path.to_string_lossy().contains("/tests/")
            && !path.to_string_lossy().contains("mock_s3")
        {
            files.push(path);
        }
    }
}

#[test]
fn p1_path_corpus_and_production_fault_surface_fail_closed() {
    for value in [
        "",
        "/control.sqlite",
        "../control.sqlite",
        "do/../control.sqlite",
        "do//state",
        "do/./state",
        "do\\state",
        "cache/state",
        "runtime/socket",
        "do/\0state",
    ] {
        assert!(!valid_restore_path(value), "accepted {value:?}");
    }
    for value in [
        "control.sqlite",
        "scheduler.sqlite",
        "kv/account/resource/data.sqlite",
        "d1/account/resource/data.sqlite",
        "do/workerd/state.sqlite",
    ] {
        assert!(valid_restore_path(value), "rejected {value}");
    }

    let root = workspace();
    let mut sources = Vec::new();
    for crate_name in [
        "core",
        "storage",
        "artifacts",
        "runtime",
        "workers",
        "service",
    ] {
        production_sources(
            &root.join("crates").join(crate_name).join("src"),
            &mut sources,
        );
    }
    assert!(!sources.is_empty());
    for path in sources {
        let source = fs::read_to_string(&path).expect("utf8 Rust source");
        for forbidden in [
            "fault-injection-route",
            "x-open-compute-crash-after",
            "OPEN_COMPUTE_DISABLE_AUTH",
            "OPEN_COMPUTE_SKIP_RUNTIME_VERIFY",
        ] {
            assert!(
                !source.contains(forbidden),
                "{forbidden} in {}",
                path.display()
            );
        }
    }
}

fn insert_account(storage: &PlatformStorage, account: AccountId) {
    let connection = rusqlite::Connection::open(storage.data_dir().control_db_path())
        .expect("open control fixture");
    connection
        .execute(
            "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms)
             VALUES (?1, ?2, 1, NULL)",
            params![account.to_string(), format!("p1-{account}")],
        )
        .expect("insert second account");
}

fn ready_deployment(
    repository: WorkerRepository<'_>,
    account: AccountId,
    worker: open_compute_core::WorkerId,
    byte: u8,
    now_ms: i64,
) -> DeploymentId {
    let id = DeploymentId::generate();
    repository
        .insert_staging_deployment(&NewDeployment {
            id,
            account_id: account,
            worker_id: worker,
            artifact_sha256: [byte; 32],
            artifact_size: 1,
            artifact_schema_version: 1,
            main_module: "index.js".to_owned(),
            compatibility_date: "2026-08-22".to_owned(),
            compatibility_flags: Vec::new(),
            limits: serde_json::json!({"profile":"p1-isolation"}),
            worker_code_sha256: [byte; 32],
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: RequestId::generate(),
            now_ms,
        })
        .expect("insert deployment");
    repository.begin_validation(id).expect("begin validation");
    repository.mark_ready(id, now_ms + 1).expect("ready");
    id
}

#[test]
fn p1_two_account_resource_and_deployment_matrix_has_no_existence_or_metric_oracle() {
    let temp = TempDir::new().expect("temp");
    let root = fs::canonicalize(temp.path())
        .expect("canonical temp")
        .join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 1,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("storage");
    let account_a = storage.identity().default_account_id;
    let account_b = AccountId::generate();
    insert_account(&storage, account_b);

    let resources = ResourceRepository::new(storage.db());
    let mut by_account = Vec::new();
    for (account_index, account) in [account_a, account_b].into_iter().enumerate() {
        let mut ids = Vec::new();
        for (kind_index, kind) in [
            BindingKind::KvNamespace,
            BindingKind::R2Bucket,
            BindingKind::D1Database,
            BindingKind::DoNamespace,
        ]
        .into_iter()
        .enumerate()
        {
            for instance in 0..2 {
                let id = ResourceId::generate();
                let key = format!("p1-{account_index}-{kind_index}-{instance}");
                let fingerprint = storage.crypto().fingerprint_request(key.as_bytes());
                let outcome = resources
                    .reserve_create(&ReserveResourceCreate {
                        account_id: account,
                        kind,
                        name: &key,
                        idempotency_key: &key,
                        fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                        request_fingerprint: &fingerprint,
                        resource_id: id,
                        driver_schema_version: 1,
                        request_id: RequestId::generate(),
                        now_ms: 10,
                        expires_at_ms: 1_000,
                    })
                    .expect("reserve resource");
                assert!(matches!(outcome, ResourceCreateReservation::Reserved(_)));
                resources.mark_ready(id, 11).expect("resource ready");
                ids.push(id);
            }
        }
        by_account.push(ids);
    }
    let a_resource = by_account[0][0];
    let b_resource = by_account[1][0];
    assert_eq!(
        resources
            .get(account_b, a_resource)
            .expect_err("cross account")
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        resources
            .get(account_b, ResourceId::generate())
            .expect_err("unknown resource")
            .code(),
        ErrorCode::ResourceNotFound
    );
    resources
        .begin_delete(account_a, a_resource, 12)
        .expect("delete A resource");
    resources
        .mark_tombstoned(account_a, a_resource, RequestId::generate(), 13)
        .expect("tombstone A resource");
    assert_eq!(
        resources
            .get(account_b, b_resource)
            .expect("B survives")
            .account_id,
        account_b
    );

    let workers = WorkerRepository::new(storage.db());
    let (worker_a, _) = workers
        .create_worker(account_a, "account-a", RequestId::generate(), 20)
        .expect("worker A");
    let (worker_b, _) = workers
        .create_worker(account_b, "account-b", RequestId::generate(), 21)
        .expect("worker B");
    let a1 = ready_deployment(workers, account_a, worker_a.id, 1, 30);
    let a2 = ready_deployment(workers, account_a, worker_a.id, 2, 40);
    let b1 = ready_deployment(workers, account_b, worker_b.id, 3, 50);
    let b2 = ready_deployment(workers, account_b, worker_b.id, 4, 60);
    assert_eq!(
        workers
            .list_deployments(account_a, worker_a.id)
            .expect("A list")
            .len(),
        2
    );
    assert_eq!(
        workers
            .list_deployments(account_b, worker_b.id)
            .expect("B list")
            .len(),
        2
    );

    let promoted_a1 = workers
        .promote(account_a, worker_a.id, a1, None, RequestId::generate(), 70)
        .expect("promote A1");
    let promoted_a2 = workers
        .promote_checked(
            account_a,
            worker_a.id,
            a2,
            Some(a1),
            Some(promoted_a1.route_generation),
            RequestId::generate(),
            71,
        )
        .expect("promote A2");
    let rolled_back = workers
        .promote_checked(
            account_a,
            worker_a.id,
            a1,
            Some(a2),
            Some(promoted_a2.route_generation),
            RequestId::generate(),
            72,
        )
        .expect("rollback A1");
    assert_eq!(rolled_back.active_deployment_id, Some(a1));
    assert_eq!(
        workers
            .promote(account_a, worker_a.id, b1, None, RequestId::generate(), 73)
            .expect_err("cross deployment")
            .code(),
        ErrorCode::DeploymentNotFound
    );
    assert_eq!(
        workers
            .promote(
                account_a,
                worker_a.id,
                DeploymentId::generate(),
                None,
                RequestId::generate(),
                73,
            )
            .expect_err("unknown deployment")
            .code(),
        ErrorCode::DeploymentNotFound
    );
    assert_eq!(
        workers
            .get_deployment(account_a, worker_a.id, b2)
            .expect_err("foreign deployment")
            .code(),
        ErrorCode::DeploymentNotFound
    );
    assert_eq!(
        workers
            .promote_checked(
                account_a,
                worker_a.id,
                a2,
                Some(a1),
                Some(promoted_a1.route_generation),
                RequestId::generate(),
                74,
            )
            .expect_err("stale generation")
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        workers
            .get_worker(account_a, worker_a.id)
            .expect("A active")
            .active_deployment_id,
        Some(a1)
    );
    assert_eq!(
        workers
            .get_worker(account_b, worker_b.id)
            .expect("B active")
            .active_deployment_id,
        None
    );

    let metrics =
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").expect("metrics");
    let rendered = metrics.render(&PlatformStatus::starting());
    for tenant_id in [account_a.to_string(), account_b.to_string()] {
        assert!(
            !rendered.contains(&tenant_id),
            "tenant ID leaked into metrics"
        );
    }
}
