//! P1 security parser/path and release-artifact hygiene Gate.

use open_compute_core::config::{DataConfig, MetricsConfig};
use open_compute_core::{
    BindingKind, ErrorCode, InstanceId, PlatformStatus, RequestId, ResourceId, SystemClock,
    VersionId, valid_restore_path,
};
use open_compute_service::metrics::MetricsRegistry;
use open_compute_storage::{
    NewVersion, PlatformStorage, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, WorkerRepository,
};
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

fn ready_version(
    repository: WorkerRepository<'_>,
    account: InstanceId,
    worker: open_compute_core::WorkerId,
    byte: u8,
    now_ms: i64,
) -> VersionId {
    let id = VersionId::generate();
    repository
        .insert_staging_version(
            &NewVersion {
                id,
                instance_id: account,
                worker_id: worker,
                content_kind: open_compute_storage::VersionContentKind::Worker,
                artifact_sha256: Some([byte; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [byte; 32],
                compatibility_date: "2026-09-08".into(),
                compatibility_flags: Vec::new(),
                resource_limits: open_compute_storage::EffectiveResourceLimits::standard_defaults(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms,
            },
            &open_compute_storage::NewVersionProducts::default(),
            1_000_000,
        )
        .expect("insert version");
    repository.begin_validation(id).expect("begin validation");
    repository.mark_ready(id, now_ms + 1).expect("ready");
    id
}

#[test]
fn p1_two_instance_resource_and_version_matrix_has_no_existence_or_metric_oracle() {
    let temp = TempDir::new().expect("temp");
    let root = fs::canonicalize(temp.path()).expect("canonical temp");
    let stores = ["a", "b"].map(|name| {
        let path = root.join(name);
        PlatformStorage::bootstrap(
            &DataConfig {
                master_key_file: path.join("keys/master.key"),
                path,
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 1,
            },
            &SystemClock,
        )
        .expect("instance storage")
    });
    let [storage_a, storage_b] = &stores;
    let account_a = storage_a.identity().instance_id;
    let account_b = storage_b.identity().instance_id;
    assert_ne!(account_a, account_b);
    let resources_a = ResourceRepository::new(storage_a.db());
    let resources_b = ResourceRepository::new(storage_b.db());
    let mut by_account = Vec::new();
    for (storage, resources) in [(storage_a, resources_a), (storage_b, resources_b)] {
        let account = storage.identity().instance_id;
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
                let key = format!("p1-{kind_index}-{instance}");
                let fingerprint = storage.crypto().fingerprint_request(key.as_bytes());
                let outcome = resources
                    .reserve_create(
                        &ReserveResourceCreate {
                            instance_id: account,
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
                        },
                        1_000_000,
                    )
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
        resources_a
            .get(account_b, a_resource)
            .expect_err("wrong instance authority")
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        resources_b
            .get(account_b, a_resource)
            .expect_err("other instance resource")
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        resources_b
            .get(account_b, ResourceId::generate())
            .expect_err("unknown resource")
            .code(),
        ErrorCode::ResourceNotFound
    );
    resources_a
        .begin_delete(account_a, a_resource, 12)
        .expect("delete A resource");
    resources_a
        .mark_tombstoned(account_a, a_resource, RequestId::generate(), 13)
        .expect("tombstone A resource");
    assert_eq!(
        resources_b
            .get(account_b, b_resource)
            .expect("B survives")
            .instance_id,
        account_b
    );

    let workers_a = WorkerRepository::new(storage_a.db());
    let workers_b = WorkerRepository::new(storage_b.db());
    let (worker_a, _) = workers_a
        .create_worker(account_a, "app", RequestId::generate(), 20, 1_000_000)
        .expect("worker A");
    let (worker_b, _) = workers_b
        .create_worker(account_b, "app", RequestId::generate(), 21, 1_000_000)
        .expect("worker B");
    let a1 = ready_version(workers_a, account_a, worker_a.id, 1, 30);
    let a2 = ready_version(workers_a, account_a, worker_a.id, 2, 40);
    let b1 = ready_version(workers_b, account_b, worker_b.id, 3, 50);
    let b2 = ready_version(workers_b, account_b, worker_b.id, 4, 60);
    assert_eq!(
        workers_a
            .list_versions(account_a, worker_a.id)
            .expect("A list")
            .len(),
        2
    );
    assert_eq!(
        workers_b
            .list_versions(account_b, worker_b.id)
            .expect("B list")
            .len(),
        2
    );

    let promoted_a1 = workers_a
        .promote(account_a, worker_a.id, a1, None, RequestId::generate(), 70)
        .expect("promote A1");
    let promoted_a2 = workers_a
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
    let rolled_back = workers_a
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
    assert_eq!(rolled_back.active_version_id, Some(a1));
    assert_eq!(
        workers_a
            .promote(account_a, worker_a.id, b1, None, RequestId::generate(), 73)
            .expect_err("cross version")
            .code(),
        ErrorCode::VersionNotFound
    );
    assert_eq!(
        workers_a
            .promote(
                account_a,
                worker_a.id,
                VersionId::generate(),
                None,
                RequestId::generate(),
                73,
            )
            .expect_err("unknown version")
            .code(),
        ErrorCode::VersionNotFound
    );
    assert_eq!(
        workers_a
            .get_version(account_a, worker_a.id, b2)
            .expect_err("foreign version")
            .code(),
        ErrorCode::VersionNotFound
    );
    assert_eq!(
        workers_a
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
        workers_a
            .get_worker(account_a, worker_a.id)
            .expect("A active")
            .active_version_id,
        Some(a1)
    );
    assert_eq!(
        workers_b
            .get_worker(account_b, worker_b.id)
            .expect("B active")
            .active_version_id,
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
