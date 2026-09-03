//! Real pinned-workerd P0 aggregate Exit Gate.
//!
//! One Worker version owns every P0 product binding so cross-product composition, resource
//! isolation, backup/rebind, version fencing, process recovery, and failure isolation are
//! proven together rather than inferred from independent product Gates.

#![cfg(feature = "test-support")]

mod common;

#[path = "p0_exit_support/mod.rs"]
mod support;

use common::load_file_only_platform_config;

use axum::http::StatusCode;
use open_compute_artifacts::{Fault, MockS3};
use open_compute_core::clock::SystemClock;
use open_compute_core::{BindingKind, RequestId, ResourceAvailability, ResourceId};
use open_compute_service::backup_cli::{
    backup_attest_restore_smoke, backup_create, backup_inspect, backup_restore,
};
use open_compute_service::doctor::{DoctorMode, doctor_report};
use open_compute_storage::{D1Migration, PlatformStorage, ResourceRepository, WorkerRepository};
use open_compute_workers::{BundleLimits, ResourcePins, RuntimeValidator, VersionController};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use support::{
    GateStack, ProductBindings, admin_json, admin_router, assert_public_id, assert_v4_envelope,
    capacity_summary, corrupt_d1, create_product_resource, deploy, dispatch, kill_workerd, now_ms,
    open_scheduler, repo_root, reset_capacity_samples, storage_config, stores, v4_product_ids,
    version_request, wait_pid_change,
};

struct PlatformConfigInput<'a> {
    temp: &'a tempfile::TempDir,
    name: &'a str,
    data_dir: &'a std::path::Path,
    master_key: &'a std::path::Path,
    mock: &'a MockS3,
}

fn write_platform_config(input: &PlatformConfigInput<'_>) -> PathBuf {
    let access_key = input.temp.path().join("p1-s3-access-key");
    let secret_key = input.temp.path().join("p1-s3-secret-key");
    fs::write(&access_key, b"AKIAEXAMPLEKEYID01").unwrap();
    fs::write(&secret_key, b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY").unwrap();
    fs::set_permissions(&access_key, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&secret_key, fs::Permissions::from_mode(0o600)).unwrap();
    let admin_token = input
        .temp
        .path()
        .join(format!("{}-admin.token", input.name));
    fs::write(&admin_token, b"p0-exit-admin\n").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    let path = input.temp.path().join(format!("{}.toml", input.name));
    fs::write(
        &path,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"
admin_bind = "127.0.0.1:0"

[server.admin_auth]
file = "{admin_token}"

[storage]
data_dir = "{data_dir}"
master_key_file = "{master_key}"

[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
prefix = "system/"
r2_prefix = "tenant/r2/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000

[runtime]

[cache]
max_bytes = 67108864
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 67108864

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 1024
"#,
            data_dir = input.data_dir.display(),
            master_key = input.master_key.display(),
            endpoint = input.mock.endpoint,
            access_key = access_key.display(),
            secret_key = secret_key.display(),
            admin_token = admin_token.display(),
        ),
    )
    .unwrap();
    path
}

#[test]
fn p0_real_combined_exit_matrix() {
    std::thread::Builder::new()
        .name("p0-combined-exit".to_owned())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("P0 runtime")
                .block_on(p0_real_combined_exit_matrix_inner());
        })
        .expect("P0 Gate thread")
        .join()
        .expect("P0 Gate thread result");
}

async fn p0_real_combined_exit_matrix_inner() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    reset_capacity_samples();
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let assets = root.join("packages/runtime");
    let temp = tempfile::tempdir().unwrap();
    let data_root = temp.path().join("data");
    let config = storage_config(&data_root);
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let scheduler_store = open_scheduler(&storage);
    let mock = MockS3::spawn("open-compute").await;
    let (artifacts, objects) = stores(&mock);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler_store.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        workerd.clone(),
        lock.clone(),
        assets.clone(),
        "p0-exit-owner",
    )
    .await;

    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "p0-combined", RequestId::generate(), 10, 1_000_000)
        .unwrap();
    let router = admin_router(
        storage.clone(),
        artifacts.clone(),
        objects.clone(),
        &pins,
        &stack,
    );
    let (bindings, do_plan) =
        create_product_set(&storage, &objects, &pins, account, worker.id).await;
    let public_ids = v4_product_ids(&router).await;
    apply_primary_d1_migration(&stack, account, bindings.d1).await;

    let version_a = {
        let validator: Arc<dyn RuntimeValidator> = Arc::new(stack.transport.clone());
        let controller = VersionController::new(
            &storage,
            artifacts.clone(),
            validator,
            BundleLimits::default(),
        )
        .with_durable_object_migration(do_plan);
        deploy(
            &controller,
            version_request(
                account,
                worker.id,
                bindings,
                "p0-exit-deploy-a",
                "A",
                true,
                20,
            ),
            &stack.supervisor,
        )
        .await
    };
    let generation_a = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;

    let seeded = dispatch(
        &stack.transport,
        account,
        worker.id,
        &version_a,
        generation_a,
        "/seed",
    )
    .await;
    let seed = response_json(&seeded);
    assert_snapshot(&seed, "A", "seed-kv", "seed-d1");
    assert_eq!(seed["durableObject"]["rpc"]["count"], 1);
    assert_eq!(seed["durableObject"]["isolated"]["count"], 1);

    let websocket = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/websocket",
        )
        .await,
    );
    assert_eq!(websocket, json!({"text": true, "binary": true}));

    let saturation = futures::future::join_all((0..16).map(|_| {
        dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/snapshot",
        )
    }))
    .await;
    for response in saturation {
        assert_snapshot(&response_json(&response), "A", "seed-kv", "seed-d1");
    }

    let kv_backup = create_backup(
        &router,
        &format!(
            "/client/v4/accounts/{}/open-compute/kv/namespaces/{}/backups",
            public_ids.account, public_ids.kv
        ),
        "p0-exit-kv-backup",
    )
    .await;
    let d1_backup = create_backup(
        &router,
        &format!(
            "/client/v4/accounts/{}/open-compute/d1/databases/{}/backups",
            public_ids.account, public_ids.d1
        ),
        "p0-exit-d1-backup",
    )
    .await;
    assert!(
        mock.keys()
            .iter()
            .any(|key| key.contains("system/backups/kv/"))
    );
    assert!(
        mock.keys()
            .iter()
            .any(|key| key.contains("system/backups/d1/"))
    );

    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            "/mutate",
        )
        .await,
    );
    let restored_kv = restore_resource(
        &router,
        &storage,
        account,
        BindingKind::KvNamespace,
        &format!(
            "/client/v4/accounts/{}/open-compute/kv/backups/{kv_backup}/restore",
            public_ids.account
        ),
        "restored-kv",
        "p0-exit-kv-restore",
    )
    .await;
    let restored_d1 = restore_resource(
        &router,
        &storage,
        account,
        BindingKind::D1Database,
        &format!(
            "/client/v4/accounts/{}/open-compute/d1/backups/{d1_backup}/restore",
            public_ids.account
        ),
        "restored-d1",
        "p0-exit-d1-restore",
    )
    .await;
    assert_ne!(restored_kv, bindings.kv);
    assert_ne!(restored_d1, bindings.d1);

    let due = now_ms().saturating_sub(1).max(1);
    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_a,
            &format!("/set-alarm?time={due}"),
        )
        .await,
    );
    let bindings_b = ProductBindings {
        kv: restored_kv,
        d1: restored_d1,
        ..bindings
    };
    let version_b = {
        let validator: Arc<dyn RuntimeValidator> = Arc::new(stack.transport.clone());
        let controller = VersionController::new(
            &storage,
            artifacts.clone(),
            validator,
            BundleLimits::default(),
        );
        deploy(
            &controller,
            version_request(
                account,
                worker.id,
                bindings_b,
                "p0-exit-deploy-b",
                "B",
                true,
                30,
            ),
            &stack.supervisor,
        )
        .await
    };
    let generation_b = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert!(generation_b > generation_a);
    assert_eq!(stack.scheduler.poll_once().await.unwrap(), 1);
    let alarm_b = alarm_status(&stack, account, worker.id, &version_b, generation_b).await;
    assert_eq!(alarm_b["alarmDeliveries"], 1);
    assert_eq!(alarm_b["alarmRelease"], "B");
    assert_eq!(alarm_b["alarmRetryCount"], 0);
    assert_eq!(alarm_b["alarm"], Value::Null);

    let restored = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_b,
            generation_b,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&restored, "B", "seed-kv", "seed-d1");

    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_b,
            generation_b,
            &format!("/set-alarm?time={due}"),
        )
        .await,
    );
    workers
        .promote_checked(
            account,
            worker.id,
            version_a.id,
            Some(version_b.id),
            Some(generation_b),
            RequestId::generate(),
            40,
        )
        .unwrap();
    let generation_rollback = workers
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    assert_eq!(stack.scheduler.poll_once().await.unwrap(), 1);
    let alarm_a = alarm_status(&stack, account, worker.id, &version_a, generation_rollback).await;
    assert_eq!(alarm_a["alarmDeliveries"], 2);
    assert_eq!(alarm_a["alarmRelease"], "A");
    let rolled_back = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&rolled_back, "A", "mutated-kv", "mutated-d1");

    let killed_pid = stack.supervisor.snapshot().pid.unwrap();
    kill_workerd(killed_pid);
    wait_pid_change(&stack.supervisor, killed_pid, Duration::from_secs(30)).await;
    let after_workerd_crash = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&after_workerd_crash, "A", "mutated-kv", "mutated-d1");

    assert_ok(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            &format!("/set-alarm?time={due}"),
        )
        .await,
    );
    for id in all_resources(bindings_b) {
        assert_eq!(pins.count(id), 0);
    }
    drop(router);
    stack.stop().await;
    drop(scheduler_store);
    drop(storage);

    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let scheduler_store = open_scheduler(&storage);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler_store.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        workerd.clone(),
        lock.clone(),
        assets.clone(),
        "p0-exit-owner",
    )
    .await;
    let router = admin_router(
        storage.clone(),
        artifacts.clone(),
        objects.clone(),
        &pins,
        &stack,
    );
    let persisted_worker = WorkerRepository::new(storage.db())
        .get_worker(account, worker.id)
        .unwrap();
    assert_eq!(persisted_worker.active_version_id, Some(version_a.id));
    assert_eq!(persisted_worker.route_generation, generation_rollback);
    assert_eq!(stack.scheduler.poll_once().await.unwrap(), 1);
    let after_platform_restart =
        alarm_status(&stack, account, worker.id, &version_a, generation_rollback).await;
    assert_eq!(after_platform_restart["alarmDeliveries"], 3);
    assert_eq!(after_platform_restart["alarmRelease"], "A");
    let persisted = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&persisted, "A", "mutated-kv", "mutated-d1");

    drop(router);
    stack.stop().await;
    drop(storage);
    let recovery_key = temp.path().join("p1-recovery-master.key");
    fs::copy(data_root.join("keys/master.key"), &recovery_key).unwrap();
    fs::set_permissions(&recovery_key, fs::Permissions::from_mode(0o600)).unwrap();
    let source_platform_config = write_platform_config(&PlatformConfigInput {
        temp: &temp,
        name: "p1-source",
        data_dir: &data_root,
        master_key: &recovery_key,
        mock: &mock,
    });
    let source_loaded = load_file_only_platform_config(&source_platform_config);
    let full_snapshot = backup_create(&source_loaded, "p0-combined-fixture")
        .await
        .unwrap();
    assert!(
        backup_inspect(&source_loaded, &full_snapshot.snapshot_id, true)
            .await
            .unwrap()
            .verified
    );
    let retired_source = temp.path().join("p1-source-unavailable");
    fs::rename(&data_root, &retired_source).unwrap();

    let restored_root = fs::canonicalize(temp.path())
        .unwrap()
        .join("p1-restored-data");
    let restore_platform_config = write_platform_config(&PlatformConfigInput {
        temp: &temp,
        name: "p1-restore",
        data_dir: &restored_root,
        master_key: &recovery_key,
        mock: &mock,
    });
    let restored_loaded = load_file_only_platform_config(&restore_platform_config);
    let restored = backup_restore(&restored_loaded, &full_snapshot.snapshot_id)
        .await
        .unwrap();
    assert_eq!(restored.platform_id, full_snapshot.platform_id);
    let doctor = doctor_report(&restored_loaded, DoctorMode::Full).await;
    assert!(!doctor.failed(), "restored doctor: {doctor:?}");

    let storage = Arc::new(
        PlatformStorage::bootstrap(&restored_loaded.config.storage, &SystemClock).unwrap(),
    );
    let scheduler_store = open_scheduler(&storage);
    let (artifacts, objects) = stores(&mock);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler_store.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        workerd,
        lock,
        assets,
        "p0-exit-owner",
    )
    .await;
    let router = admin_router(storage.clone(), artifacts, objects, &pins, &stack);
    let restored_worker = WorkerRepository::new(storage.db())
        .get_worker(account, worker.id)
        .unwrap();
    assert_eq!(restored_worker.active_version_id, Some(version_a.id));
    assert_eq!(restored_worker.route_generation, generation_rollback);
    let restored_snapshot = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&restored_snapshot, "A", "mutated-kv", "mutated-d1");

    let s3_request = tokio::spawn({
        let transport = stack.transport.clone();
        let version = version_a.clone();
        async move {
            dispatch(
                &transport,
                account,
                worker.id,
                &version,
                generation_rollback,
                "/s3-fault?delay=250",
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    mock.set_fault(Fault::ServerError);
    let s3_failure = response_json(&s3_request.await.unwrap());
    assert!(
        s3_failure["r2Error"]
            .as_str()
            .unwrap()
            .contains("R2_PROVIDER_UNAVAILABLE")
    );
    assert_eq!(s3_failure["kv"], "mutated-kv");
    assert_eq!(s3_failure["d1"], "mutated-d1");
    let (failed_backup_status, failed_backup) = admin_json(
        &router,
        "POST",
        &format!(
            "/client/v4/accounts/{}/open-compute/kv/namespaces/{}/backups",
            public_ids.account, public_ids.kv_other
        ),
        Value::Null,
        Some("p0-exit-s3-failed-backup"),
    )
    .await;
    assert_v4_envelope(failed_backup_status, &failed_backup);
    assert_eq!(failed_backup_status, StatusCode::SERVICE_UNAVAILABLE);
    let failure_text = failed_backup.to_string();
    assert!(!failure_text.contains(&mock.endpoint));
    assert!(!failure_text.contains("sqlite"));
    mock.set_fault(Fault::None);
    let after_s3_recovery = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/snapshot",
        )
        .await,
    );
    assert_snapshot(&after_s3_recovery, "A", "mutated-kv", "mutated-d1");

    corrupt_d1(&storage, account, bindings.d1_corrupt);
    let isolated_corruption = response_json(
        &dispatch(
            &stack.transport,
            account,
            worker.id,
            &version_a,
            generation_rollback,
            "/corruption",
        )
        .await,
    );
    assert_eq!(isolated_corruption["primary"], "mutated-d1");
    assert_eq!(isolated_corruption["kv"], "mutated-kv");
    assert!(
        isolated_corruption["corruptError"]
            .as_str()
            .unwrap()
            .contains("D1_DATABASE_CORRUPT")
    );
    assert_eq!(
        ResourceRepository::new(storage.db())
            .get(account, bindings.d1_corrupt)
            .unwrap()
            .availability,
        ResourceAvailability::Unavailable
    );

    let deleted = dispatch(
        &stack.transport,
        account,
        worker.id,
        &version_a,
        generation_rollback,
        "/delete-r2",
    )
    .await;
    assert_eq!((deleted.status, deleted.body.as_str()), (200, "null"));
    for id in all_resources(bindings_b) {
        assert_eq!(pins.count(id), 0);
    }
    drop(router);
    stack.stop().await;
    drop(storage);
    let attestation =
        backup_attest_restore_smoke(&restored_loaded, &full_snapshot.snapshot_id, true)
            .await
            .unwrap();
    assert!(attestation.smoke_verified);
    println!("P1_CAPACITY {}", capacity_summary());
    println!("P0 combined Worker/KV/R2/D1/DO/alarm/WebSocket/backup/restart/failure matrix PASS");
}

async fn create_product_set(
    storage: &PlatformStorage,
    objects: &open_compute_artifacts::R2ObjectStore,
    pins: &ResourcePins,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
) -> (
    ProductBindings,
    open_compute_storage::DurableObjectMigrationPlan,
) {
    let kv = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::KvNamespace,
        "combined-kv",
        "p0-exit-create-kv",
        now_ms(),
    )
    .await;
    let kv_other = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::KvNamespace,
        "combined-kv-other",
        "p0-exit-create-kv-other",
        now_ms(),
    )
    .await;
    let r2 = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::R2Bucket,
        "combined-r2",
        "p0-exit-create-r2",
        now_ms(),
    )
    .await;
    let r2_other = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::R2Bucket,
        "combined-r2-other",
        "p0-exit-create-r2-other",
        now_ms(),
    )
    .await;
    let d1 = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::D1Database,
        "combined-d1",
        "p0-exit-create-d1",
        now_ms(),
    )
    .await;
    let d1_other = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::D1Database,
        "combined-d1-other",
        "p0-exit-create-d1-other",
        now_ms(),
    )
    .await;
    let d1_corrupt = create_product_resource(
        storage,
        objects,
        pins,
        account,
        BindingKind::D1Database,
        "combined-d1-corrupt",
        "p0-exit-create-d1-corrupt",
        now_ms(),
    )
    .await;
    let do_repository = open_compute_storage::DurableObjectRepository::new(storage);
    let do_plan = open_compute_storage::DurableObjectMigrationPlan {
        declarative: false,
        old_tag: None,
        new_tag: "p0-exit-v1".to_owned(),
        new_sqlite_classes: vec!["AppObject".to_owned(), "OtherObject".to_owned()],
        renamed_classes: Vec::new(),
        deleted_classes: Vec::new(),
    };
    do_repository
        .prepare_worker_migration(account, worker, &do_plan, 1_000_000)
        .unwrap();
    let objects = do_repository
        .namespace_for_worker_upload(account, worker, "AppObject", Some("p0-exit-v1"))
        .unwrap()
        .resource
        .id;
    let objects_other = do_repository
        .namespace_for_worker_upload(account, worker, "OtherObject", Some("p0-exit-v1"))
        .unwrap()
        .resource
        .id;
    (
        ProductBindings {
            kv,
            kv_other,
            r2,
            r2_other,
            d1,
            d1_other,
            d1_corrupt,
            objects,
            objects_other,
        },
        do_plan,
    )
}

async fn apply_primary_d1_migration(
    stack: &GateStack,
    account: open_compute_core::AccountId,
    database: ResourceId,
) {
    let sql = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL)";
    let migration = D1Migration {
        id: 1,
        name: "0001_notes.sql".into(),
        sha256: Sha256::digest(sql.as_bytes()).into(),
        sql: sql.into(),
    };
    let applied = stack
        .d1
        .apply_migrations(account, database, vec![migration.clone()], now_ms())
        .await
        .unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        stack
            .d1
            .apply_migrations(account, database, vec![migration], now_ms())
            .await
            .unwrap(),
        applied
    );
}

async fn create_backup(router: &axum::Router, uri: &str, key: &str) -> String {
    let (status, body) = admin_json(router, "POST", uri, Value::Null, Some(key)).await;
    assert_v4_envelope(status, &body);
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["result"]["id"].as_str().unwrap().to_owned();
    let (replay_status, replay) = admin_json(router, "POST", uri, Value::Null, Some(key)).await;
    assert_v4_envelope(replay_status, &replay);
    assert_eq!(replay_status, StatusCode::OK, "{replay}");
    assert_eq!(replay["result"]["id"], id);
    id
}

async fn restore_resource(
    router: &axum::Router,
    storage: &PlatformStorage,
    account: open_compute_core::AccountId,
    kind: BindingKind,
    uri: &str,
    name: &str,
    key: &str,
) -> ResourceId {
    let body = json!({"name": name});
    let (status, restored) = admin_json(router, "POST", uri, body.clone(), Some(key)).await;
    assert_v4_envelope(status, &restored);
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_public_id(restored["result"]["id"].as_str().unwrap());
    assert_eq!(restored["result"]["name"], name);
    let (replay_status, replay) = admin_json(router, "POST", uri, body, Some(key)).await;
    assert_v4_envelope(replay_status, &replay);
    assert_eq!(replay_status, StatusCode::OK, "{replay}");
    assert_eq!(replay["result"]["id"], restored["result"]["id"]);
    ResourceRepository::new(storage.db())
        .list(account, Some(kind))
        .unwrap()
        .into_iter()
        .find(|resource| resource.name == name)
        .unwrap()
        .id
}

async fn alarm_status(
    stack: &GateStack,
    account: open_compute_core::AccountId,
    worker: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    generation: u64,
) -> Value {
    let response = dispatch(
        &stack.transport,
        account,
        worker,
        version,
        generation,
        "/alarm-status",
    )
    .await;
    assert_eq!(
        response.status,
        200,
        "{}; runtime={:?}; diagnostics={:?}",
        response.body,
        stack.supervisor.snapshot(),
        stack.supervisor.last_diagnostics()
    );
    serde_json::from_str(&response.body).unwrap()
}

#[track_caller]
fn response_json(response: &support::DispatchResponse) -> Value {
    assert_ok(response);
    serde_json::from_str(&response.body).unwrap()
}

#[track_caller]
fn assert_ok(response: &support::DispatchResponse) {
    assert_eq!(response.status, 200, "{}", response.body);
}

#[track_caller]
fn assert_snapshot(value: &Value, release: &str, kv: &str, d1: &str) {
    assert_eq!(value["release"], release);
    for key in ["kv", "r2", "d1", "durableObject"] {
        assert_eq!(value["facade"][key], true, "{key}: {value}");
    }
    for key in [
        "workers",
        "kv",
        "r2",
        "d1",
        "durableObjects",
        "websocket",
        "adversarialValues",
        "maliciousWorker",
    ] {
        assert_eq!(value["conformance"][key], true, "{key}: {value}");
    }
    assert_eq!(value["kv"]["text"], kv);
    assert_eq!(value["kv"]["json"], json!({"ok": true, "product": "kv"}));
    assert_eq!(value["kv"]["binary"], json!([1, 2]));
    assert_eq!(value["kv"]["stream"], "stream-value");
    assert_eq!(value["kv"]["isolated"], "isolated-kv");
    assert_eq!(value["r2"]["body"], "hello-r2");
    assert_eq!(value["r2"]["range"], "ell");
    assert_eq!(value["r2"]["size"], 8);
    assert_eq!(value["r2"]["custom"], "seed");
    assert_eq!(value["r2"]["contentType"], "text/plain");
    assert_eq!(value["r2"]["metadataReturnUndefined"], true);
    assert_eq!(value["r2"]["isolated"], "isolated-r2");
    assert_eq!(value["d1"]["first"], d1);
    assert_eq!(value["d1"]["sessionCount"], 3);
    assert_eq!(value["d1"]["isolated"], "isolated-d1");
    assert_eq!(value["durableObject"]["rpc"]["count"], 1);
    assert_eq!(value["durableObject"]["fetch"]["count"], 1);
    assert_eq!(value["durableObject"]["isolated"]["count"], 1);
    assert_eq!(value["durableObject"]["rpc"]["alarmConformance"], true);
}

fn all_resources(bindings: ProductBindings) -> [ResourceId; 9] {
    [
        bindings.kv,
        bindings.kv_other,
        bindings.r2,
        bindings.r2_other,
        bindings.d1,
        bindings.d1_other,
        bindings.d1_corrupt,
        bindings.objects,
        bindings.objects_other,
    ]
}
