//! Real filesystem, lock, control-database, and AEAD tests.

use crate::data_dir::{expected_directories, future_resource_paths};
use crate::fs as sfs;
use crate::master_key;
use crate::migrations::MigrationFault;
use crate::{
    DataDir, DeploymentState, IdempotencyReservation, NewDeployment, NewQueueConsumerDeclaration,
    PlatformStorage, QueueConsumerConfig, QueueConsumerRepository, ReserveResourceCreate,
    ResourceCreateReservation, ResourceRepository, SecretCrypto, StoredDeploymentSecret,
    WorkerRepository, atomic_write, inspect_durable_object_storage,
};
use open_compute_core::clock::{DeterministicClock, SystemClock};
use open_compute_core::config::StorageConfig;
use open_compute_core::{
    AccountId, BindingKind, DeploymentId, ErrorCode, HardeningConfig, PlatformReleaseIdentityV1,
    QueueConsumerId, ResourceId, SecretBytes, WorkerId,
};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

fn storage_config(root: &Path) -> StorageConfig {
    StorageConfig {
        data_dir: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn unique_root() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("data");
    (tmp, root)
}

fn restore_writable(path: &Path) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
        }
    }
}

#[test]
fn clean_and_repeat_bootstrap_preserves_identity() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let clock = DeterministicClock::new(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    let first = PlatformStorage::bootstrap(&config, &clock).expect("first");
    let platform_id = first.identity().platform_id;
    let account = first.identity().default_account_id;
    let created = first.identity().created_at_ms;
    drop(first);
    clock.advance(Duration::from_secs(60));
    let second = PlatformStorage::bootstrap(&config, &clock).expect("second");
    assert_eq!(second.identity().platform_id, platform_id);
    assert_eq!(second.identity().default_account_id, account);
    assert_eq!(second.identity().created_at_ms, created);
    assert_eq!(
        second
            .db()
            .query_meta("last_started_version")
            .unwrap()
            .as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn p1_control_inventory_returns_only_fixed_aggregate_counts() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let empty = crate::inspect_control_inventory(storage.db()).unwrap();
    assert_eq!(empty.accounts, 1);
    assert_eq!(empty.workers, 0);
    assert_eq!(empty.deployments, 0);
    assert_eq!(empty.routes, 0);
    assert_eq!(empty.kv_namespaces, 0);

    WorkerRepository::new(storage.db())
        .create_worker(
            storage.identity().default_account_id,
            "inventory-worker",
            open_compute_core::RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let populated = crate::inspect_control_inventory(storage.db()).unwrap();
    assert_eq!(populated.accounts, 1);
    assert_eq!(populated.workers, 1);
    assert_eq!(populated.routes, 1);
}

#[test]
fn p1_owned_schema_inspection_sees_uncheckpointed_bootstrap_wal() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    assert!(crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).is_err());
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());

    let state = crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).unwrap();
    assert_eq!(
        i64::from(state.control),
        crate::migrations::current_schema_version()
    );
    assert_eq!(
        state.scheduler,
        u32::try_from(crate::current_scheduler_schema_version()).unwrap()
    );
    assert_eq!(state.kv_files, 0);
    assert_eq!(state.d1_files, 0);
}

#[test]
fn p1_readonly_schema_fence_sees_uncheckpointed_bootstrap_wal() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();

    let readonly =
        crate::ControlDb::open_readonly_wal_aware(&root.join("control.sqlite"), 5_000).unwrap();
    assert_eq!(
        crate::migrations::inspect_schema(&readonly).unwrap(),
        crate::migrations::current_schema_version()
    );
    drop(storage);
}

#[test]
fn p1_schema_inspection_checks_current_kv_and_d1_files_without_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let account = storage.identity().default_account_id;

    let reserve = |kind, name: &str, key: &str| {
        let fingerprint = storage.crypto().fingerprint_request(key.as_bytes());
        let reserved = ResourceRepository::new(storage.db())
            .reserve_create(
                &ReserveResourceCreate {
                    account_id: account,
                    kind,
                    name,
                    idempotency_key: key,
                    fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                    request_fingerprint: &fingerprint,
                    resource_id: ResourceId::generate(),
                    driver_schema_version: 1,
                    request_id: open_compute_core::RequestId::generate(),
                    now_ms: 1,
                    expires_at_ms: 10,
                },
                1_000_000,
            )
            .unwrap();
        let ResourceCreateReservation::Reserved(resource) = reserved else {
            panic!("resource must be newly reserved");
        };
        resource
    };

    let kv = reserve(BindingKind::KvNamespace, "schema-kv", "schema-kv");
    let kv_paths = crate::KvPaths::open(&root).unwrap();
    let kv_key = crate::KvPaths::storage_key(account, kv.id);
    crate::KvNamespaceRepository::new(storage.db())
        .ensure_namespace(&kv, &kv_key, crate::KV_SCHEMA_VERSION, 256 * 1024 * 1024)
        .unwrap();
    let kv_staging = kv_paths.create_namespace_staging(kv.id).unwrap();
    drop(
        crate::KvEngine::create(
            &kv_staging.join("data.sqlite"),
            account,
            kv.id,
            1,
            256 * 1024 * 1024,
        )
        .unwrap(),
    );
    kv_paths
        .publish_staging(&kv_staging, account, kv.id)
        .unwrap();

    let d1 = reserve(BindingKind::D1Database, "schema-d1", "schema-d1");
    let d1_paths = crate::D1Paths::open(&root).unwrap();
    let d1_key = crate::D1Paths::storage_key(account, d1.id);
    crate::D1DatabaseRepository::new(storage.db())
        .ensure_database(
            &d1,
            &d1_key,
            crate::D1_DATABASE_SCHEMA_VERSION,
            64 * 1024 * 1024,
        )
        .unwrap();
    let d1_staging = d1_paths.create_database_staging(d1.id).unwrap();
    drop(
        crate::D1Engine::create(
            &d1_staging.join("data.sqlite"),
            account,
            d1.id,
            1,
            64 * 1024 * 1024,
        )
        .unwrap(),
    );
    d1_paths
        .publish_staging(&d1_staging, account, d1.id)
        .unwrap();
    for resource in [kv.id, d1.id] {
        ResourceRepository::new(storage.db())
            .mark_ready(resource, 2)
            .unwrap();
    }

    let owned = crate::inspect_current_schema(storage.data_dir(), storage.db(), 5_000).unwrap();
    assert_eq!(owned.kv_files, 1);
    assert_eq!(owned.d1_files, 1);
    assert_eq!(owned.kv, crate::KV_SCHEMA_VERSION);
    assert_eq!(owned.d1, crate::D1_DATABASE_SCHEMA_VERSION);
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&config).unwrap();
    let control =
        crate::ControlDb::open_readonly_wal_aware(&data_dir.control_db_path(), 5_000).unwrap();
    let offline = crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap();
    assert_eq!(offline, owned);
    let key = crate::inspect_master_key(&config).unwrap();
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let authority = crate::inspect_control_db(&data_dir.control_db_path(), 5_000)
        .unwrap()
        .1;
    let object_prefix = format!(
        "system/snapshots/v1/{}/{snapshot_id}/objects/",
        authority.platform_id
    );
    let hardening = HardeningConfig::default();
    let request = crate::PreparePlatformSnapshotRequest {
        snapshot_id: &snapshot_id,
        label: "resource-schema-snapshot",
        created_at_ms: 2,
        release: p1_release_identity(),
        master_key_fingerprint: key.fingerprint(),
        s3_authority_fingerprint: &"d".repeat(64),
        r2_prefix_fingerprint: &"e".repeat(64),
        config_policy_sha256: &"f".repeat(64),
        object_prefix: &object_prefix,
        hardening: &hardening,
        sqlite_busy_timeout_ms: 5_000,
    };
    assert!(
        crate::estimate_platform_snapshot_bytes(
            &data_dir,
            &request,
            &authority.platform_id.to_string()
        )
        .unwrap()
            > 0
    );
    let prepared = crate::prepare_platform_snapshot(&data_dir, &request).unwrap();
    assert!(
        prepared
            .manifest
            .files
            .iter()
            .any(|file| file.role == open_compute_core::SnapshotFileRole::KvSqlite)
    );
    assert!(
        prepared
            .manifest
            .files
            .iter()
            .any(|file| file.role == open_compute_core::SnapshotFileRole::D1Sqlite)
    );
    assert_eq!(
        crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap(),
        owned
    );
    let directory = kv_paths.namespace_dir(account, kv.id);
    let relocated = directory.with_extension("held");
    fs::rename(&directory, &relocated).unwrap();
    std::os::unix::fs::symlink(&relocated, &directory).unwrap();
    assert_eq!(
        crate::inspect_current_schema(&data_dir, &control, 5_000)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    fs::remove_file(&directory).unwrap();
    fs::rename(&relocated, &directory).unwrap();
    let file = kv_paths.database_path(account, kv.id);
    fs::remove_file(&file).unwrap();
    assert!(crate::inspect_current_schema(&data_dir, &control, 5_000).is_err());
    assert!(
        !file.exists(),
        "inspection must not recreate missing authority"
    );
}

#[test]
fn p1_disk_admission_modes_and_staging_tree_validation_are_explicit() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let hardening = HardeningConfig::default();
    let admission = crate::DiskAdmission::new(&config, &hardening);
    assert_eq!(
        admission.snapshot(storage.data_dir()).unwrap().mode,
        open_compute_core::PlatformMode::Serving
    );
    assert_eq!(
        admission.reserve(storage.data_dir(), 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    drop(admission.reserve(storage.data_dir(), 4096).unwrap());
    admission.begin_draining();
    assert_eq!(
        admission.snapshot(storage.data_dir()).unwrap().mode,
        open_compute_core::PlatformMode::Draining
    );
    let offline = crate::DiskAdmission::offline(&config, &hardening);
    assert_eq!(
        offline.snapshot(storage.data_dir()).unwrap().mode,
        open_compute_core::PlatformMode::Offline
    );

    let nested = storage.data_dir().backup_staging_dir().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("payload"), b"12345").unwrap();
    assert!(
        admission
            .snapshot(storage.data_dir())
            .unwrap()
            .owned_staging_bytes
            >= 5
    );
    let link = nested.join("escape");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    assert_eq!(
        admission.snapshot(storage.data_dir()).unwrap_err().code(),
        ErrorCode::StoragePressure
    );
}

#[test]
fn durable_object_storage_marker_is_stable_and_inspectable() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let owned = DataDir::acquire(&config).unwrap();
    let path = owned
        .prepare_durable_object_storage("platform-id", "workerd-version")
        .unwrap();
    assert_eq!(
        inspect_durable_object_storage(&root, "platform-id", "workerd-version").unwrap(),
        path
    );
    assert_eq!(
        inspect_durable_object_storage(&root, "other-platform", "workerd-version")
            .unwrap_err()
            .code(),
        ErrorCode::DoStorageUnavailable
    );
    assert_eq!(
        owned
            .prepare_durable_object_storage("platform-id", "other-workerd")
            .unwrap_err()
            .code(),
        ErrorCode::DoStorageUnavailable
    );
}

#[test]
fn lock_released_after_failed_bootstrap() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let err = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::BeforeExecution),
    )
    .expect_err("fault");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    PlatformStorage::bootstrap(&config, &SystemClock).expect("retry after failed bootstrap");
}

#[test]
fn relative_and_symlink_root_rejected() {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut relative = storage_config(tmp.path());
    relative.data_dir = PathBuf::from("relative-data");
    let err = DataDir::acquire(&relative).expect_err("relative");
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut cfg = storage_config(&link);
    cfg.master_key_file = link.join("keys/master.key");
    let err = DataDir::acquire(&cfg).expect_err("symlink root");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}

#[test]
fn child_symlink_and_fifo_rejected() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let owned = DataDir::acquire(&config).expect("acquire");
    drop(owned);

    let outside = _tmp.path().join("outside");
    fs::write(&outside, b"x").unwrap();
    let keys = root.join("keys");
    fs::remove_dir_all(&keys).unwrap();
    std::os::unix::fs::symlink(&outside, &keys).unwrap();
    let err = DataDir::acquire(&config).expect_err("symlink child");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    fs::remove_file(&keys).unwrap();
    fs::create_dir(&keys).unwrap();
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o700)).unwrap();

    let fifo = root.join("runtime").join("fifo");
    let status = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
    assert!(status.success());
    let err = sfs::validate_contained(&root, &fifo).expect_err("fifo");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    let _ = fs::remove_file(&fifo);
}

#[test]
fn world_writable_root_rejected() {
    let (_tmp, root) = unique_root();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    let config = storage_config(&root);
    let err = DataDir::acquire(&config).expect_err("world");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    restore_writable(&root);
}

#[test]
fn filesystem_and_lock_helpers_reject_missing_special_and_escaping_paths() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    let parent_file = root.join("parent-file");
    fs::write(&parent_file, b"x").unwrap();
    assert_eq!(
        sfs::create_dir_secure(&parent_file.join("child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::create_root_first_run(Path::new("/"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::create_root_first_run(&root.join("missing/child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::validate_contained(&root.join("missing"), &root.join("missing/child"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let outside = tmp.path().join("outside");
    fs::write(&outside, b"outside").unwrap();
    assert_eq!(
        sfs::validate_contained(&root, &outside).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        atomic_write(Path::new("/"), b"x").unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        sfs::chmod(&root.join("missing"), 0o600).unwrap_err().code(),
        ErrorCode::PathInvalid
    );

    assert_eq!(
        crate::DataDirLock::classify_path(&root.join("missing")),
        crate::FilesystemDurability::Unclassified
    );
    assert!(
        crate::FilesystemDurability::ApparentlyLocal
            .doctor_warning()
            .is_none()
    );
    assert!(
        crate::FilesystemDurability::NetworkOrRemote
            .doctor_warning()
            .unwrap()
            .contains("network")
    );
    assert!(
        crate::FilesystemDurability::Unclassified
            .doctor_warning()
            .unwrap()
            .contains("could not be classified")
    );
    assert_eq!(
        crate::InspectLock::try_acquire(&root.join("missing.lock"))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn identity_bootstrap_rejects_corrupt_existing_authority_rows() {
    enum Corruption {
        Platform(Vec<u8>),
        Created(Vec<u8>),
        Account(String),
    }
    for corruption in [
        Corruption::Platform(b"bad-platform".to_vec()),
        Corruption::Platform(vec![0xff]),
        Corruption::Created(b"not-a-number".to_vec()),
        Corruption::Account("bad-account".to_owned()),
    ] {
        let (_tmp, root) = unique_root();
        let config = storage_config(&root);
        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let fingerprint = storage.crypto().fingerprint_key_id().to_owned();
        drop(storage);
        let db_path = root.join("control.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        match corruption {
            Corruption::Platform(value) => {
                conn.execute(
                    "UPDATE platform_meta SET value = ?1 WHERE key = 'platform_id'",
                    [value],
                )
                .unwrap();
            }
            Corruption::Created(value) => {
                conn.execute(
                    "UPDATE platform_meta SET value = ?1 WHERE key = 'created_at_ms'",
                    [value],
                )
                .unwrap();
            }
            Corruption::Account(value) => {
                conn.execute(
                    "UPDATE accounts SET id = ?1 WHERE name = 'default'",
                    [value],
                )
                .unwrap();
            }
        }
        drop(conn);
        let db = crate::control_db::ControlDb::open(&db_path, 5_000).unwrap();
        assert!(crate::identity::bootstrap(&db, &SystemClock, &fingerprint).is_err());
    }
}

#[test]
fn exact_layout_and_no_future_files() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    for dir in expected_directories(&root) {
        assert!(dir.is_dir(), "{}", dir.display());
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", dir.display());
    }
    assert!(root.join("platform.lock").is_file());
    assert!(root.join("control.sqlite").is_file());
    for future in future_resource_paths(&root) {
        assert!(!future.exists(), "{}", future.display());
    }
    drop(storage);
}

#[test]
fn deployment_staging_is_private_and_crash_residue_is_cleared_under_lock() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    let staging = storage.data_dir().deployment_staging_dir();
    assert_eq!(
        fs::metadata(&staging).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(storage);

    let stale = staging.join("interrupted.upload");
    fs::write(&stale, b"partial tenant source").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();
    let data_dir = DataDir::acquire(&config).expect("reacquire");
    assert!(!stale.exists());
    drop(data_dir);
}

#[test]
fn atomic_replace_and_temp_cleanup() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("d");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let dest = dir.join("file");
    atomic_write(&dest, b"one").expect("write1");
    atomic_write(&dest, b"two").expect("write2");
    assert_eq!(fs::read(&dest).unwrap(), b"two");
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn pragmas_schema_strict_and_partial_index() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    assert_eq!(
        storage
            .db()
            .pragma_display("journal_mode")
            .unwrap()
            .to_lowercase(),
        "wal"
    );
    let sync = storage.db().pragma_display("synchronous").unwrap();
    assert!(sync == "2" || sync.eq_ignore_ascii_case("full"));
    assert_eq!(storage.db().pragma_display("foreign_keys").unwrap(), "1");
    assert_eq!(storage.db().pragma_display("trusted_schema").unwrap(), "0");
    for table in ["schema_migrations", "platform_meta", "accounts"] {
        let sql = storage.db().table_sql(table).unwrap().expect("sql");
        assert!(sql.to_ascii_uppercase().contains("STRICT"), "{sql}");
    }
    let idx = storage
        .db()
        .index_sql("accounts_live_name")
        .unwrap()
        .unwrap();
    assert!(idx.to_ascii_uppercase().contains("UNIQUE"));
    assert!(idx.contains("deleted_at_ms"));
}

fn raw_user_version(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[test]
fn migration_faults_checksum_future_and_restart() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    for fault in [
        MigrationFault::BeforeExecution,
        MigrationFault::DuringDdl,
        MigrationFault::BeforeMigrationRow,
    ] {
        let (_t, r) = unique_root();
        let c = storage_config(&r);
        let err =
            PlatformStorage::bootstrap_with_fault(&c, &SystemClock, Some(fault)).expect_err("f");
        assert_eq!(err.code(), ErrorCode::MigrationFailed);
        assert_eq!(raw_user_version(&r.join("control.sqlite")), 0);
        PlatformStorage::bootstrap(&c, &SystemClock).expect("recover");
        assert_eq!(
            raw_user_version(&c.data_dir.join("control.sqlite")),
            crate::migrations::current_schema_version()
        );
    }

    let err = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::AfterCommit),
    )
    .expect_err("after commit reports failure");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 1);
    PlatformStorage::bootstrap(&config, &SystemClock).expect("restart sees committed migration");

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute(
        "UPDATE schema_migrations SET checksum_sha256 = ?1",
        [vec![0u8; 32]],
    )
    .unwrap();
    drop(conn);
    let checksum_err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("checksum");
    assert_eq!(checksum_err.code(), ErrorCode::MigrationFailed);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", 99).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("future");
    assert_eq!(err.code(), ErrorCode::SchemaTooNew);
}

#[test]
fn p0_2_migration_ddl_fault_rolls_back_to_schema_one() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::AfterCommit),
    )
    .expect_err("migration one commits, then reports the injected fault");
    assert_eq!(first.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 1);

    let second = PlatformStorage::bootstrap_with_fault(
        &config,
        &SystemClock,
        Some(MigrationFault::DuringDdl),
    )
    .expect_err("migration two must roll back its entire trigger/table batch");
    assert_eq!(second.code(), ErrorCode::MigrationFailed);
    assert_eq!(raw_user_version(&root.join("control.sqlite")), 1);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let workers_exist: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workers')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!workers_exist, "migration two DDL must be atomic");
    drop(conn);

    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    assert_eq!(
        raw_user_version(&root.join("control.sqlite")),
        crate::migrations::current_schema_version()
    );
}

#[test]
fn master_key_modes_and_failures() {
    let (_tmp, root) = unique_root();
    let mut config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("auto");
    let fp = storage.identity().master_key_id.clone();
    let key_bytes = fs::read_to_string(&config.master_key_file).unwrap();
    assert!(key_bytes.starts_with("ocmk1:"));
    drop(storage);

    let env_name = "PLATFORM_STORAGE_TEST_MASTER_KEY";
    master_key::set_test_env(env_name, key_bytes.trim());
    config.master_key_env = Some(env_name.to_string());
    let both = PlatformStorage::bootstrap(&config, &SystemClock).expect("both");
    assert_eq!(both.identity().master_key_id, fp);
    drop(both);

    master_key::set_test_env(
        env_name,
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    );
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("mismatch both");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);

    let (_t2, root2) = unique_root();
    let mut env_only = storage_config(&root2);
    env_only.master_key_env = Some(env_name.to_string());
    fs::create_dir_all(root2.join("keys")).unwrap();
    fs::set_permissions(root2.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
    master_key::set_test_env(env_name, key_bytes.trim());
    let env_boot = PlatformStorage::bootstrap(&env_only, &SystemClock).expect("env only");
    assert!(
        !env_only.master_key_file.exists(),
        "env-only must not persist plaintext"
    );
    drop(env_boot);
    master_key::clear_test_env();

    let mut loose = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&config.master_key_file)
        .unwrap();
    loose.write_all(key_bytes.as_bytes()).unwrap();
    drop(loose);
    fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o644)).unwrap();
    config.master_key_env = None;
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("loose");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&config.master_key_file, b"ocmk1:not-valid!!!").unwrap();
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("corrupt");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
}

#[test]
fn db_fingerprint_mismatch_fails_closed() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap(&config, &SystemClock).expect("first");
    drop(first);
    fs::remove_file(&config.master_key_file).unwrap();
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("new key vs db");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
    assert!(config.master_key_file.exists());
}

#[test]
fn r2_cursor_hmac_is_domain_separated_and_rejects_tampering() {
    let key = SecretBytes::new(vec![7_u8; 32]);
    let fingerprint = master_key::fingerprint_for_test(key.expose());
    let crypto = SecretCrypto::new(&key, &fingerprint).unwrap();
    let payload = br#"{"v":1}"#;
    let signature = crypto.sign_r2_cursor(payload);
    assert!(crypto.verify_r2_cursor(payload, &signature));
    assert!(!crypto.verify_r2_cursor(br#"{"v":2}"#, &signature));
    assert_ne!(signature, crypto.sign_kv_cursor(payload));
}

#[test]
fn no_secrets_in_db_debug_json_or_errors() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    let key_file = fs::read_to_string(&config.master_key_file).unwrap();
    let secret_body = key_file.trim().strip_prefix("ocmk1:").unwrap();
    let debug = format!("{storage:?}");
    let json_lock = fs::read_to_string(root.join("platform.lock")).unwrap();
    let db_bytes = storage.db().dump_bytes().unwrap();
    let db_text = String::from_utf8_lossy(&db_bytes);
    let err = open_compute_core::PlatformError::new(
        ErrorCode::MasterKeyMismatch,
        "master key fingerprint mismatch",
    );
    let err_json = serde_json::to_string(&err).unwrap();
    for hay in [
        debug.as_str(),
        json_lock.as_str(),
        db_text.as_ref(),
        err_json.as_str(),
    ] {
        assert!(!hay.contains(secret_body), "leaked key material");
        assert!(!hay.contains("super-secret-value"));
    }
    let raw = fs::read(root.join("control.sqlite")).unwrap();
    assert!(
        !raw.windows(secret_body.len())
            .any(|w| w == secret_body.as_bytes())
    );
}

#[test]
fn lock_metadata_is_diagnostic_only() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("platform.lock")).unwrap()).unwrap();
    assert!(meta.get("startup_id").is_some());
    assert!(meta.get("pid").is_some());
    assert!(meta.get("release_version").is_some());
    assert!(
        storage
            .data_dir()
            .filesystem_durability()
            .doctor_warning()
            .is_none()
            || storage
                .data_dir()
                .filesystem_durability()
                .doctor_warning()
                .is_some()
    );
    drop(storage);
}

#[test]
fn subprocess_lock_contention() {
    if std::env::var("PLATFORM_STORAGE_HOLD_LOCK").ok().as_deref() == Some("1") {
        let root = PathBuf::from(std::env::var("PLATFORM_STORAGE_HOLD_ROOT").unwrap());
        let config = storage_config(&root);
        let _owned = DataDir::acquire(&config).expect("child lock");
        let ready = PathBuf::from(std::env::var("PLATFORM_STORAGE_HOLD_READY").unwrap());
        File::create(&ready).unwrap();
        loop {
            if PathBuf::from(std::env::var("PLATFORM_STORAGE_HOLD_STOP").unwrap()).exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        return;
    }

    let (_tmp, root) = unique_root();
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let ready = _tmp.path().join("ready");
    let stop = _tmp.path().join("stop");
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&exe)
        .env("PLATFORM_STORAGE_HOLD_LOCK", "1")
        .env("PLATFORM_STORAGE_HOLD_ROOT", &root)
        .env("PLATFORM_STORAGE_HOLD_READY", &ready)
        .env("PLATFORM_STORAGE_HOLD_STOP", &stop)
        .args([
            "--exact",
            "tests::subprocess_lock_contention",
            "--nocapture",
        ])
        .spawn()
        .expect("spawn");
    let start = std::time::Instant::now();
    while !ready.exists() {
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("child did not acquire lock");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let config = storage_config(&root);
    let err = DataDir::acquire(&config).expect_err("contended");
    assert_eq!(err.code(), ErrorCode::DataDirInUse);
    File::create(&stop).unwrap();
    let _ = child.wait();
    DataDir::acquire(&config).expect("after child release");
}

#[test]
fn readonly_root_rejects_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let owned = DataDir::acquire(&config).expect("create");
    drop(owned);
    let keys = root.join("keys");
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o500)).unwrap();
    let dest = keys.join("x");
    let result = atomic_write(&dest, b"nope");
    restore_writable(&keys);
    restore_writable(&root);
    assert!(result.is_err());
}

#[test]
fn durability_does_not_claim_safety_when_unclassified() {
    use crate::FilesystemDurability;
    assert!(
        FilesystemDurability::ApparentlyLocal
            .doctor_warning()
            .is_none()
    );
    assert!(
        FilesystemDurability::NetworkOrRemote
            .doctor_warning()
            .is_some()
    );
    assert!(
        FilesystemDurability::Unclassified
            .doctor_warning()
            .is_some()
    );
}

#[test]
fn partially_created_key_is_rejected() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let _ = DataDir::acquire(&config).unwrap();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config.master_key_file)
        .unwrap();
    let err = master_key::resolve(&config).expect_err("empty key");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
}

#[test]
fn lock_symlink_and_loose_mode_are_rejected_without_side_effects() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("data");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let outside = tmp.path().join("outside.lock");
    fs::write(&outside, b"outside-target").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("platform.lock")).unwrap();
    let config = storage_config(&root);
    let err = DataDir::acquire(&config).expect_err("symlink lock");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&outside).unwrap(), b"outside-target");
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o644
    );

    fs::remove_file(root.join("platform.lock")).unwrap();
    let lock_path = root.join("platform.lock");
    let mut loose = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o622)
        .open(&lock_path)
        .unwrap();
    loose.write_all(b"loose").unwrap();
    drop(loose);
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o622)).unwrap();
    let err = DataDir::acquire(&config).expect_err("loose lock");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&lock_path).unwrap(), b"loose");
    assert_eq!(
        fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
        0o622
    );
}

#[test]
fn sqlite_and_key_symlinks_are_rejected() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let _owned = DataDir::acquire(&config).unwrap();
    drop(_owned);
    let outside = _tmp.path().join("outside.sqlite");
    fs::write(&outside, b"x").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("control.sqlite")).unwrap();
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("db symlink");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(fs::read(&outside).unwrap(), b"x");

    fs::remove_file(root.join("control.sqlite")).unwrap();
    let key_outside = _tmp.path().join("outside.key");
    fs::write(
        &key_outside,
        b"ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap();
    fs::create_dir_all(root.join("keys")).unwrap();
    fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&key_outside, &config.master_key_file).unwrap();
    let err = master_key::resolve(&config).expect_err("key symlink");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert_eq!(
        fs::read(&key_outside).unwrap(),
        b"ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    );
}

#[test]
fn missing_master_key_env_fails_closed() {
    let (_tmp, root) = unique_root();
    let mut config = storage_config(&root);
    config.master_key_env = Some("PLATFORM_STORAGE_MISSING_KEY".to_string());
    master_key::clear_test_env();
    let _owned = DataDir::acquire(&config).unwrap();
    drop(_owned);
    let err = master_key::resolve(&config).expect_err("missing env");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
    assert!(!root.join("control.sqlite").exists());
    assert!(!config.master_key_file.exists());
}

#[test]
fn key_file_rejects_trailing_bytes_and_does_not_overwrite() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let _owned = DataDir::acquire(&config).unwrap();
    drop(_owned);
    let first = master_key::resolve(&config).expect("generate");
    let original = fs::read(&config.master_key_file).unwrap();
    drop(first);

    let extra = {
        let mut v = original.clone();
        v.push(b'\n');
        v
    };
    fs::write(&config.master_key_file, &extra).unwrap();
    fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o600)).unwrap();
    let err = master_key::resolve(&config).expect_err("newline");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
    assert_eq!(fs::read(&config.master_key_file).unwrap(), extra);

    fs::write(&config.master_key_file, &original).unwrap();
    let raced = master_key::resolve(&config).expect("existing wins");
    assert_eq!(fs::read(&config.master_key_file).unwrap(), original);
    drop(raced);

    let leftover = root.join("keys").join(".tmp-master-partial");
    fs::write(&leftover, b"partial").unwrap();
    fs::set_permissions(&leftover, fs::Permissions::from_mode(0o600)).unwrap();
    master_key::resolve(&config).expect("ignores leftover temp");
    assert_eq!(fs::read(&config.master_key_file).unwrap(), original);
    crate::fs::fsync_dir(&root.join("keys")).expect("dir fsync path");
}

#[test]
fn seeded_inconsistent_migrations_fail_closed() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    PlatformStorage::bootstrap(&config, &SystemClock).unwrap();

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute("DELETE FROM schema_migrations", []).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing rows");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let (_t2, root2) = unique_root();
    let c2 = storage_config(&root2);
    PlatformStorage::bootstrap(&c2, &SystemClock).unwrap();
    let conn = Connection::open(root2.join("control.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", 0).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&c2, &SystemClock).expect_err("uv0 with rows");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let (_t3, root3) = unique_root();
    let c3 = storage_config(&root3);
    PlatformStorage::bootstrap(&c3, &SystemClock).unwrap();
    let conn = Connection::open(root3.join("control.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO schema_migrations (version, name, checksum_sha256, applied_at_ms, app_version)
         VALUES (99, 'future', ?1, 1, 'x')",
        [vec![0u8; 32]],
    )
    .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&c3, &SystemClock).expect_err("row too new");
    assert_eq!(err.code(), ErrorCode::SchemaTooNew);
}

#[test]
fn incomplete_identity_fails_without_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let first = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    first.db().quick_check().unwrap();
    let last = first
        .db()
        .query_meta("last_started_version")
        .unwrap()
        .expect("last");
    let fp = first.identity().master_key_id.clone();
    drop(first);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute("DELETE FROM accounts", []).unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing account");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let still: String = conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM platform_meta WHERE key = 'last_started_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, last);
    conn.execute(
        "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms) VALUES ('acct_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'default', 1, NULL)",
        [],
    )
    .ok();
    conn.execute("DELETE FROM platform_meta WHERE key = 'created_at_ms'", [])
        .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("missing created");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);

    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute(
        "INSERT INTO platform_meta (key, value, updated_at_ms) VALUES ('created_at_ms', CAST('1' AS BLOB), 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE platform_meta SET value = CAST('2' AS BLOB) WHERE key = 'artifact_schema_version'",
        [],
    )
    .unwrap();
    drop(conn);
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("artifact");
    assert_eq!(err.code(), ErrorCode::MigrationFailed);
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    let stored_fp: Vec<u8> = conn
        .query_row(
            "SELECT value FROM platform_meta WHERE key = 'master_key_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_fp, fp.as_bytes());
}

#[test]
fn inspect_lock_holds_and_releases_flock() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let held = crate::InspectLock::try_acquire(&config.data_lock_path())
        .unwrap()
        .expect("available");
    assert!(!crate::DataDirLock::probe_available(&config.data_lock_path()).unwrap());
    drop(held);
    assert!(crate::DataDirLock::probe_available(&config.data_lock_path()).unwrap());
}

#[test]
fn operation_receipt_reads_are_bounded_and_never_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let (tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    storage
        .data_dir()
        .write_operation_receipt("last-restore.json", b"bounded")
        .unwrap();
    assert_eq!(
        storage
            .data_dir()
            .read_operation_receipt("last-restore.json", 7)
            .unwrap(),
        b"bounded"
    );
    assert_eq!(
        storage
            .data_dir()
            .read_operation_receipt("../control.sqlite", 64)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    let receipt = root.join("operations/last-restore.json");
    fs::write(&receipt, vec![0_u8; 65]).unwrap();
    assert_eq!(
        storage
            .data_dir()
            .read_operation_receipt("last-restore.json", 64)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );

    fs::remove_file(&receipt).unwrap();
    let outside = tmp.path().join("outside-receipt");
    fs::write(&outside, b"outside").unwrap();
    symlink(&outside, &receipt).unwrap();
    assert!(
        storage
            .data_dir()
            .read_operation_receipt("last-restore.json", 64)
            .is_err()
    );
}

#[test]
fn inspect_control_db_accepts_uri_special_path_chars() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("data?x#y%z");
    let config = storage_config(&root);
    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let inspect = crate::inspect_data_root(&config).unwrap();
    assert!(inspect.lock_available);
    let (version, identity) =
        crate::inspect_control_db(&inspect.root.join("control.sqlite"), 5_000).unwrap();
    assert_eq!(version, crate::migrations::current_schema_version());
    assert!(!identity.master_key_id.is_empty());
}

#[test]
fn snapshot_deployment_artifact_inventory_uses_the_canonical_sharded_key() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "snapshot-key", request, 1, 1_000_000)
        .unwrap();
    repo.insert_staging_deployment(
        &NewDeployment {
            id: DeploymentId::generate(),
            account_id: account,
            worker_id: worker.id,
            content_kind: crate::DeploymentContentKind::Worker,
            artifact_sha256: Some([1; 32]),
            artifact_size: Some(123),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            compatibility_date: "2026-08-22".to_owned(),
            compatibility_flags: Vec::new(),
            limits: serde_json::json!({}),
            worker_code_sha256: [2; 32],
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: 2,
        },
        &crate::NewDeploymentProducts::default(),
        1_000_000,
    )
    .unwrap();
    drop(storage);

    let references = crate::inspect_snapshot_immutable_references(
        &root.join("control.sqlite"),
        5_000,
        "system/",
    )
    .unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].role, "deployment_artifact");
    assert_eq!(
        references[0].object_key,
        format!("system/artifacts/v1/sha256/01/{}", "01".repeat(31))
    );
}

#[test]
fn p0_2_repository_enforces_lifecycle_immutability_and_idempotency() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, route) = repo
        .create_worker(account, "hello-worker", request, 1_000, 1_000_000)
        .unwrap();
    assert_eq!(
        route.path_prefix,
        format!("/__workers/{account}/hello-worker/")
    );
    assert_eq!(
        repo.create_worker(account, "hello-worker", request, 1_001, 1_000_000)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNameConflict
    );

    let deployment = DeploymentId::generate();
    let revision = uuid::Uuid::now_v7().to_string();
    let envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"never-persist-plaintext".to_vec()),
            account,
            worker.id,
            deployment,
            "API_TOKEN",
            &revision,
        )
        .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), br#""production""#.to_vec());
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "API_TOKEN".to_owned(),
        StoredDeploymentSecret {
            name: "API_TOKEN".to_owned(),
            revision_id: revision.clone(),
            envelope,
        },
    );
    let created = repo
        .insert_staging_deployment(
            &NewDeployment {
                id: deployment,
                account_id: account,
                worker_id: worker.id,
                content_kind: crate::DeploymentContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(123),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                compatibility_date: "2026-08-22".to_owned(),
                compatibility_flags: vec!["rpc".to_owned()],
                limits: serde_json::json!({"profile": "default"}),
                worker_code_sha256: [2; 32],
                vars,
                secrets,
                request_id: request,
                now_ms: 2_000,
            },
            &crate::NewDeploymentProducts::default(),
            1_000_000,
        )
        .unwrap();
    assert_eq!(created.version_number, 1);
    assert_eq!(created.state, DeploymentState::Staging);
    repo.begin_validation(deployment).unwrap();
    repo.mark_ready(deployment, 2_100).unwrap();
    let promoted = repo
        .promote(account, worker.id, deployment, None, request, 2_200)
        .unwrap();
    assert_eq!(promoted.active_deployment_id, Some(deployment));
    let resolved = repo
        .resolve_route(None, &format!("{}path", route.path_prefix))
        .unwrap();
    assert_eq!(resolved.deployment.id, deployment);
    let snapshot = repo
        .deployment_snapshot(account, worker.id, deployment, false)
        .unwrap();
    let secret = snapshot.secrets.get("API_TOKEN").unwrap();
    let plaintext = storage
        .crypto()
        .decrypt(
            &secret.envelope,
            account,
            worker.id,
            deployment,
            "API_TOKEN",
            &revision,
        )
        .unwrap();
    assert_eq!(plaintext.expose(), b"never-persist-plaintext");

    let db_path = root.join("control.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    assert!(
        conn.execute(
            "UPDATE worker_deployments SET main_module = 'changed.js' WHERE id = ?1",
            [deployment.to_string()],
        )
        .is_err()
    );
    drop(conn);
    assert_eq!(
        repo.tombstone_deployment(account, worker.id, deployment, request, 3_000)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentActive
    );

    let fingerprint = storage.crypto().fingerprint_request(b"canonical request");
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "worker.create",
            "key-1",
            storage.crypto().fingerprint_key_id(),
            &fingerprint,
            4_000,
            5_000,
        )
        .unwrap(),
        IdempotencyReservation::Reserved
    );
    repo.complete_idempotency(
        account,
        "worker.create",
        "key-1",
        &fingerprint,
        br#"{"ok":true}"#,
    )
    .unwrap();
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "worker.create",
            "key-1",
            storage.crypto().fingerprint_key_id(),
            &fingerprint,
            4_001,
            5_001,
        )
        .unwrap(),
        IdempotencyReservation::Complete(br#"{"ok":true}"#.to_vec())
    );
    let other = storage.crypto().fingerprint_request(b"different request");
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "worker.create",
            "key-1",
            storage.crypto().fingerprint_key_id(),
            &other,
            4_002,
            5_002,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );

    let raw = fs::read(db_path).unwrap();
    assert!(
        !raw.windows(b"never-persist-plaintext".len())
            .any(|window| window == b"never-persist-plaintext")
    );
}

#[test]
fn p0_2_delete_referrer_recovery_and_worker_identity_are_fenced() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, route) = repo
        .create_worker(account, "delete-gate", request, 1, 1_000_000)
        .unwrap();
    let a = insert_ready(&repo, account, worker.id, [9; 32], request, 10);
    repo.promote(account, worker.id, a, None, request, 11)
        .unwrap();
    let b = insert_ready(&repo, account, worker.id, [9; 32], request, 12);

    repo.add_deployment_referrer(b, "control_idempotency", "safe-ref", 13)
        .unwrap();
    assert_eq!(repo.deployment_referrers(b).unwrap().len(), 1);
    assert_eq!(
        repo.begin_deployment_delete(account, worker.id, b)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentReferenced
    );
    repo.remove_deployment_referrer(b, "control_idempotency", "safe-ref")
        .unwrap();
    repo.begin_deployment_delete(account, worker.id, b).unwrap();
    assert_eq!(repo.deleting_deployments().unwrap(), vec![b]);
    drop(storage); // crash boundary: deleting is committed, finalization is retryable.

    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let repo = WorkerRepository::new(storage.db());
    assert_eq!(repo.deleting_deployments().unwrap(), vec![b]);
    assert_eq!(
        repo.recover_deleting_deployments(request, 20, 64).unwrap(),
        1
    );
    assert!(repo.deleting_deployments().unwrap().is_empty());
    assert_eq!(
        repo.get_deployment(account, worker.id, b).unwrap().state,
        DeploymentState::Tombstoned
    );
    let refs = repo.referenced_artifacts().unwrap();
    assert_eq!(refs, vec![([9; 32], 100)]);
    assert_eq!(
        repo.begin_deployment_delete(account, worker.id, a)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentActive
    );

    let old = insert_ready(&repo, account, worker.id, [7; 32], request, 40);
    let newest = insert_ready(&repo, account, worker.id, [8; 32], request, 41);
    let candidates = repo.retention_candidates(1_000, 1, 1, 1, 64).unwrap();
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.deployment_id == old)
    );
    assert!(
        !candidates
            .iter()
            .any(|candidate| candidate.deployment_id == newest)
    );

    let expected = repo
        .list_deployments(account, worker.id)
        .unwrap()
        .into_iter()
        .filter(|deployment| deployment.deleted_at_ms.is_none())
        .map(|deployment| deployment.id)
        .collect::<Vec<_>>();
    repo.delete_worker(account, worker.id, &expected, request, 30)
        .unwrap();
    assert_eq!(
        repo.resolve_route(None, &format!("{}x", route.path_prefix))
            .unwrap_err()
            .code(),
        ErrorCode::RouteNotFound
    );
    let (replacement, replacement_route) = repo
        .create_worker(account, "delete-gate", request, 31, 1_000_000)
        .unwrap();
    assert_ne!(replacement.id, worker.id);
    assert_ne!(replacement.do_storage_id, worker.do_storage_id);
    assert_ne!(replacement_route.id, route.id);
}

#[test]
fn p0_2_concurrent_promotions_have_one_linearization_winner() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (worker, _) = repo
        .create_worker(account, "promotion-race", request, 1, 1_000_000)
        .unwrap();
    let a = insert_ready(&repo, account, worker.id, [1; 32], request, 10);
    let b = insert_ready(&repo, account, worker.id, [2; 32], request, 11);
    let c = insert_ready(&repo, account, worker.id, [3; 32], request, 12);
    let active = repo
        .promote(account, worker.id, a, None, request, 13)
        .unwrap();
    let generation = active.route_generation;

    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| {
            repo.promote_checked(
                account,
                worker.id,
                b,
                Some(a),
                Some(generation),
                request,
                14,
            )
        });
        let right = scope.spawn(|| {
            repo.promote_checked(
                account,
                worker.id,
                c,
                Some(a),
                Some(generation),
                request,
                14,
            )
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    assert_ne!(left.is_ok(), right.is_ok());
    let loser = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(loser.code(), ErrorCode::IdempotencyConflict);
    let current = repo.get_worker(account, worker.id).unwrap();
    assert!(matches!(current.active_deployment_id, Some(id) if id == b || id == c));
    assert_eq!(current.route_generation, generation + 1);
}

#[test]
fn filesystem_helpers_cover_secure_success_and_failure_paths() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    assert!(sfs::require_absolute(&root).is_ok());
    assert!(sfs::require_absolute(Path::new("relative")).is_err());
    assert!(sfs::require_absolute(Path::new("/tmp/../escape")).is_err());
    assert!(sfs::validate_root(&root).is_ok());
    assert!(sfs::validate_root(&root.join("missing")).is_err());

    let owned_dir = root.join("owned");
    sfs::create_dir_secure(&owned_dir).unwrap();
    sfs::create_dir_secure(&owned_dir).unwrap();
    assert!(sfs::validate_owned_dir(&owned_dir).is_ok());
    let file = root.join("authority");
    sfs::ensure_file_secure(&file).unwrap();
    sfs::ensure_file_secure(&file).unwrap();
    assert!(sfs::validate_owned_file(&file, true).is_ok());
    assert!(sfs::validate_owned_dir(&file).is_err());
    assert!(sfs::validate_owned_file(&owned_dir, true).is_err());

    let loose = root.join("loose");
    fs::write(&loose, b"x").unwrap();
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(sfs::validate_owned_file(&loose, false).is_ok());
    assert!(sfs::validate_owned_file(&loose, true).is_err());
    sfs::chmod(&loose, 0o600).unwrap();
    assert!(sfs::validate_owned_file(&loose, true).is_ok());
    assert!(sfs::chmod(&root.join("missing"), 0o600).is_err());

    let link = root.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(sfs::validate_owned_file(&link, true).is_err());
    assert!(sfs::open_nofollow(&link, false, false).is_err());
    assert!(sfs::validate_contained(&root, &link).is_err());

    let opened = sfs::open_nofollow(&file, false, false).unwrap();
    sfs::validate_authority_fd(&opened).unwrap();
    let directory_fd = File::open(&owned_dir).unwrap();
    assert!(sfs::validate_authority_fd(&directory_fd).is_err());
    let loose_fd = File::open(&loose).unwrap();
    sfs::validate_authority_fd(&loose_fd).unwrap();
    assert!(sfs::open_nofollow(Path::new("relative"), false, false).is_err());
    assert!(sfs::open_nofollow(&root.join("does-not-exist"), false, false).is_err());
    let created = root.join("created");
    drop(sfs::open_nofollow(&created, true, true).unwrap());
    assert!(created.is_file());

    assert!(sfs::validate_contained(&root, &file).is_ok());
    assert!(sfs::validate_contained(&root, &root.join("future")).is_ok());
    assert!(sfs::validate_contained(&root, tmp.path()).is_err());
    assert!(sfs::validate_contained(&root, &root.join("missing-parent/future")).is_err());
    assert!(sfs::inspect(&root.join("missing")).is_err());
    sfs::fsync_dir(&root).unwrap();
    assert!(sfs::fsync_dir(&root.join("missing")).is_err());

    let nested = tmp.path().join("new-root");
    sfs::create_root_first_run(&nested).unwrap();
    assert!(sfs::create_root_first_run(&nested).is_err());
    assert!(sfs::create_root_first_run(&tmp.path().join("missing-parent/root")).is_err());
    let root_file = tmp.path().join("root-file");
    fs::write(&root_file, b"x").unwrap();
    assert!(sfs::validate_root(&root_file).is_err());
    let root_link = tmp.path().join("root-link");
    std::os::unix::fs::symlink(&root, &root_link).unwrap();
    assert!(sfs::validate_root(&root_link).is_err());

    let atomic = root.join("atomic");
    atomic_write(&atomic, b"one").unwrap();
    atomic_write(&atomic, b"two").unwrap();
    assert_eq!(fs::read(&atomic).unwrap(), b"two");
    assert!(atomic_write(Path::new("relative"), b"x").is_err());
    assert!(atomic_write(&root.join("missing-parent/value"), b"x").is_err());
}

#[test]
fn control_db_read_write_helpers_and_failures_are_enforced() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let db = storage.db();
    assert!(db.table_exists("accounts").unwrap());
    assert!(!db.table_exists("not_a_table").unwrap());
    assert!(db.table_sql("accounts").unwrap().is_some());
    assert!(db.table_sql("not_a_table").unwrap().is_none());
    assert!(db.index_sql("not_an_index").unwrap().is_none());
    assert!(!db.dump_bytes().unwrap().is_empty());
    assert_eq!(
        db.pragma_display("user_version").unwrap(),
        crate::migrations::current_schema_version().to_string()
    );
    assert!(db.pragma_display("not_a_pragma").is_err());

    db.with_exclusive(|tx| {
        tx.execute(
            "INSERT INTO platform_meta (key, value, updated_at_ms) VALUES ('invalid_utf8', ?1, 1)",
            [vec![0xff]],
        )
        .map_err(|_| open_compute_core::PlatformError::new(ErrorCode::Internal, "insert"))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        db.query_meta("invalid_utf8").unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert!(db.query_meta("absent").unwrap().is_none());
    let expected = open_compute_core::PlatformError::new(ErrorCode::Internal, "callback");
    assert_eq!(
        db.with_immediate::<()>(|_| Err(expected.clone()))
            .unwrap_err()
            .code(),
        ErrorCode::Internal
    );

    let mut raw = Connection::open_in_memory().unwrap();
    raw.pragma_update(None, "foreign_keys", "OFF").unwrap();
    assert!(crate::control_db::verify_foreign_keys_on(&raw).is_err());
    raw.pragma_update(None, "foreign_keys", "ON").unwrap();
    crate::control_db::verify_foreign_keys_on(&raw).unwrap();
    let tx = raw.transaction().unwrap();
    crate::control_db::set_user_version(&tx, 7).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        7
    );

    let db_path = root.join("control.sqlite");
    drop(storage);
    let readonly = crate::ControlDb::open_readonly(&db_path, 100).unwrap();
    assert_eq!(
        readonly.user_version().unwrap(),
        crate::migrations::current_schema_version()
    );
    readonly.quick_check().unwrap();
    assert!(crate::ControlDb::open_readonly(&root.join("missing.sqlite"), 100).is_err());
    assert!(crate::ControlDb::open(&root.join("missing/child.sqlite"), 100).is_err());
    let target = root.join("real.sqlite");
    fs::write(&target, b"").unwrap();
    let link = root.join("linked.sqlite");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(crate::ControlDb::open(&link, 100).is_err());
}

#[test]
fn master_key_inspection_rejects_missing_malformed_and_ambiguous_sources() {
    let (_tmp, root) = unique_root();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let keys = root.join("keys");
    fs::create_dir(&keys).unwrap();
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = storage_config(&root);
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    let key = master_key::resolve(&config).unwrap();
    assert_eq!(key.bytes().expose().len(), 32);
    assert_eq!(key.fingerprint().len(), 64);
    assert!(!format!("{key:?}").contains("ocmk1:"));
    master_key::inspect_existing(&config).unwrap();

    for value in [
        "bad-prefix",
        "ocmk1:not-valid!!!",
        "ocmk1:AA",
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
    ] {
        fs::write(&config.master_key_file, value).unwrap();
        fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            master_key::inspect_existing(&config).unwrap_err().code(),
            ErrorCode::MasterKeyMismatch
        );
    }

    fs::remove_file(&config.master_key_file).unwrap();
    fs::create_dir(&config.master_key_file).unwrap();
    assert!(master_key::inspect_existing(&config).is_err());
    fs::remove_dir(&config.master_key_file).unwrap();
    let outside = root.join("outside");
    fs::write(
        &outside,
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&outside, &config.master_key_file).unwrap();
    assert!(master_key::inspect_existing(&config).is_err());
    fs::remove_file(&config.master_key_file).unwrap();

    let env_name = "OPEN_COMPUTE_STORAGE_EMPTY_MASTER_KEY";
    config.master_key_env = Some(env_name.to_owned());
    master_key::set_test_env(env_name, "");
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    master_key::clear_test_env();

    config.master_key_file = PathBuf::from("relative-key");
    config.master_key_env = None;
    assert_eq!(
        master_key::resolve(&config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn master_key_inspection_covers_env_file_utf8_and_mismatch_paths() {
    let (_tmp, root) = unique_root();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join("keys")).unwrap();
    fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = storage_config(&root);
    let generated = master_key::resolve(&config).unwrap();
    let encoded = fs::read_to_string(&config.master_key_file).unwrap();
    let env_name = "OPEN_COMPUTE_STORAGE_INSPECT_MASTER_KEY";
    config.master_key_env = Some(env_name.to_owned());
    master_key::set_test_env(env_name, &encoded);
    let both = master_key::inspect_existing(&config).unwrap();
    assert_eq!(both.fingerprint(), generated.fingerprint());

    master_key::set_test_env(
        env_name,
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    );
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    fs::remove_file(&config.master_key_file).unwrap();
    let env_only = master_key::inspect_existing(&config).unwrap();
    assert_eq!(env_only.bytes().expose().len(), 32);

    config.master_key_env = None;
    let mut invalid_utf8 = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config.master_key_file)
        .unwrap();
    invalid_utf8.write_all(&[0xff, 0xfe]).unwrap();
    drop(invalid_utf8);
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    master_key::clear_test_env();

    let (_tmp2, root2) = unique_root();
    fs::create_dir(&root2).unwrap();
    fs::set_permissions(&root2, fs::Permissions::from_mode(0o700)).unwrap();
    let missing_parent = storage_config(&root2);
    assert_eq!(
        master_key::resolve(&missing_parent).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn master_key_process_environment_modes_are_covered_in_isolated_processes() {
    const MARKER: &str = "OPEN_COMPUTE_MASTER_KEY_CHILD_MODE";
    const KEY_ENV: &str = "OPEN_COMPUTE_MASTER_KEY_CHILD_VALUE";
    if let Ok(mode) = std::env::var(MARKER) {
        let (_tmp, root) = unique_root();
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("keys")).unwrap();
        fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = storage_config(&root);
        config.master_key_env = Some(KEY_ENV.to_owned());
        let result = master_key::inspect_existing(&config);
        match mode.as_str() {
            "valid" => assert_eq!(result.unwrap().bytes().expose(), &[0_u8; 32]),
            "empty" | "missing" | "invalid-utf8" => {
                assert_eq!(result.unwrap_err().code(), ErrorCode::MasterKeyMismatch);
            }
            _ => panic!("unexpected child mode"),
        }
        return;
    }

    use std::os::unix::ffi::OsStringExt;
    let current = std::env::current_exe().unwrap();
    for mode in ["valid", "empty", "missing", "invalid-utf8"] {
        let mut command = Command::new(&current);
        command.args([
            "--exact",
            "tests::master_key_process_environment_modes_are_covered_in_isolated_processes",
            "--test-threads=1",
        ]);
        command.env(MARKER, mode);
        match mode {
            "valid" => {
                command.env(KEY_ENV, "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
            }
            "empty" => {
                command.env(KEY_ENV, "");
            }
            "missing" => {
                command.env_remove(KEY_ENV);
            }
            "invalid-utf8" => {
                command.env(KEY_ENV, std::ffi::OsString::from_vec(vec![0xff]));
            }
            _ => unreachable!(),
        }
        assert!(
            command.status().unwrap().success(),
            "child mode {mode} failed"
        );
    }
}

#[test]
fn inspection_layout_migration_and_repository_helpers_are_covered() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let data = storage.data_dir();
    assert_eq!(data.root(), root);
    assert_eq!(data.control_db_path(), root.join("control.sqlite"));
    assert_eq!(data.keys_dir(), root.join("keys"));
    assert_eq!(data.runtime_dir(), root.join("runtime"));
    assert_eq!(data.artifact_cache_dir(), root.join("cache/artifacts"));
    assert_eq!(data.lock().path(), config.data_lock_path());
    assert_ne!(data.lock().startup_id().to_string(), "");
    assert_eq!(
        data.filesystem_durability(),
        data.lock().filesystem_durability()
    );

    let busy = crate::inspect_data_root(&config).unwrap();
    assert!(!busy.lock_available);
    assert!(!busy.holds_inspect_lock());
    drop(storage);
    let available = crate::inspect_data_root(&config).unwrap();
    assert!(available.lock_available);
    assert!(available.holds_inspect_lock());
    assert_eq!(available.root, root);
    drop(available);

    let mut relative = config.clone();
    relative.data_dir = PathBuf::from("relative");
    assert_eq!(
        crate::inspect_data_root(&relative).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let (_missing_tmp, missing_root) = unique_root();
    assert!(crate::inspect_data_root(&storage_config(&missing_root)).is_err());
    assert_eq!(
        crate::inspect_control_db(Path::new("relative.sqlite"), 100)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        crate::inspect_control_db(&root.join("missing.sqlite"), 100)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    assert_eq!(crate::migrations::current_schema_version(), 13);
    assert_eq!(crate::migrations::migration_001_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_002_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_003_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_004_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_005_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_006_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_007_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_008_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_009_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_010_checksum().len(), 32);
    assert_eq!(crate::migrations::migration_011_checksum().len(), 32);
    assert!(crate::migrations::expected_checksum(1).is_ok());
    assert!(crate::migrations::expected_checksum(2).is_ok());
    assert!(crate::migrations::expected_checksum(3).is_ok());
    assert!(crate::migrations::expected_checksum(4).is_ok());
    assert!(crate::migrations::expected_checksum(5).is_ok());
    assert!(crate::migrations::expected_checksum(6).is_ok());
    assert!(crate::migrations::expected_checksum(7).is_ok());
    assert!(crate::migrations::expected_checksum(8).is_ok());
    assert_eq!(
        crate::migrations::expected_checksum(crate::migrations::current_schema_version() + 1)
            .unwrap_err()
            .code(),
        ErrorCode::SchemaTooNew
    );
    assert_eq!(
        crate::migrations::expected_checksum(0).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    let uri = crate::control_db::sqlite_readonly_uri(Path::new("/tmp/a b?#%.sqlite"));
    assert_eq!(uri, "file:/tmp/a%20b%3F%23%25.sqlite?mode=ro&immutable=1");
    assert!(crate::ControlDb::open(Path::new("/"), 100).is_err());

    use crate::workers::{
        array32, db_error, deployment_not_found, idempotency_ref_id, invariant, route_not_found,
        validate_exact_route, validate_referrer, validate_worker_name, worker_not_found,
    };
    for state in [
        DeploymentState::Staging,
        DeploymentState::Validating,
        DeploymentState::Ready,
        DeploymentState::Rejected,
        DeploymentState::Deleting,
        DeploymentState::Tombstoned,
    ] {
        assert_eq!(DeploymentState::parse(state.as_str()).unwrap(), state);
    }
    assert!(DeploymentState::parse("bad").is_err());
    assert_eq!(
        crate::RouteKind::parse("platform_path").unwrap(),
        crate::RouteKind::PlatformPath
    );
    assert_eq!(
        crate::RouteKind::parse("exact_host").unwrap(),
        crate::RouteKind::ExactHost
    );
    assert!(crate::RouteKind::parse("bad").is_err());
    for valid in ["a", "worker-1"] {
        validate_worker_name(valid).unwrap();
    }
    for invalid in ["", "-bad", "bad-", "Upper", &"a".repeat(64)] {
        assert!(validate_worker_name(invalid).is_err());
    }
    validate_referrer("route", "host/path:one").unwrap();
    assert!(validate_referrer("", "id").is_err());
    assert!(validate_referrer("kind", "bad value").is_err());
    validate_exact_route("example.com", "/path", Some("handler_1$")).unwrap();
    for (host, path, entrypoint) in [
        ("", "/", None),
        ("UPPER.example", "/", None),
        ("example.com", "relative", None),
        ("example.com", "/bad?query", None),
        ("example.com", "/", Some("bad-name")),
    ] {
        assert!(validate_exact_route(host, path, entrypoint).is_err());
    }
    let account = AccountId::generate();
    assert_eq!(idempotency_ref_id(account, "scope", "key").len(), 64);
    assert!(array32(&[0_u8; 32]).is_ok());
    assert!(array32(&[0_u8; 31]).is_err());
    assert_eq!(worker_not_found().code(), ErrorCode::WorkerNotFound);
    assert_eq!(deployment_not_found().code(), ErrorCode::DeploymentNotFound);
    assert_eq!(route_not_found().code(), ErrorCode::RouteNotFound);
    assert_eq!(invariant().code(), ErrorCode::DeploymentInvariantViolation);
    assert_eq!(db_error().code(), ErrorCode::Internal);
}

fn inspect_identity_after(sql: &str) -> ErrorCode {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    drop(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let conn = Connection::open(root.join("control.sqlite")).unwrap();
    conn.pragma_update(None, "ignore_check_constraints", "ON")
        .unwrap();
    conn.execute_batch(sql).unwrap();
    drop(conn);
    let db = crate::ControlDb::open(&root.join("control.sqlite"), 100).unwrap();
    crate::identity::inspect_stored(&db).unwrap_err().code()
}

#[test]
fn inspect_stored_identity_rejects_every_malformed_authority_field() {
    let cases = [
        (
            "DELETE FROM platform_meta WHERE key = 'platform_id'",
            ErrorCode::MigrationFailed,
        ),
        (
            "UPDATE platform_meta SET value = CAST('bad' AS BLOB) WHERE key = 'platform_id'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "DELETE FROM platform_meta WHERE key = 'created_at_ms'",
            ErrorCode::MigrationFailed,
        ),
        (
            "UPDATE platform_meta SET value = CAST('bad' AS BLOB) WHERE key = 'created_at_ms'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "DELETE FROM platform_meta WHERE key = 'master_key_id'",
            ErrorCode::MigrationFailed,
        ),
        (
            "DELETE FROM platform_meta WHERE key = 'artifact_schema_version'",
            ErrorCode::MigrationFailed,
        ),
        (
            "UPDATE platform_meta SET value = CAST('2' AS BLOB) WHERE key = 'artifact_schema_version'",
            ErrorCode::MigrationFailed,
        ),
        ("DELETE FROM accounts", ErrorCode::MigrationFailed),
        (
            "UPDATE accounts SET id = 'invalid' WHERE name = 'default'",
            ErrorCode::ConfigInvalid,
        ),
        (
            "UPDATE platform_meta SET value = X'FF' WHERE key = 'platform_id'",
            ErrorCode::ConfigInvalid,
        ),
    ];
    for (sql, expected) in cases {
        assert_eq!(inspect_identity_after(sql), expected, "{sql}");
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn insert_ready(
    repo: &WorkerRepository<'_>,
    account: AccountId,
    worker: WorkerId,
    digest: [u8; 32],
    request: open_compute_core::RequestId,
    now: i64,
) -> DeploymentId {
    let id = DeploymentId::generate();
    repo.insert_staging_deployment(
        &NewDeployment {
            id,
            account_id: account,
            worker_id: worker,
            content_kind: crate::DeploymentContentKind::Worker,
            artifact_sha256: Some(digest),
            artifact_size: Some(100),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            compatibility_date: "2026-08-22".to_owned(),
            compatibility_flags: Vec::new(),
            limits: serde_json::json!({"profile":"default"}),
            worker_code_sha256: digest,
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: now,
        },
        &crate::NewDeploymentProducts::default(),
        1_000_000,
    )
    .unwrap();
    repo.begin_validation(id).unwrap();
    repo.mark_ready(id, now + 1).unwrap();
    id
}

#[test]
fn service_declarations_follow_active_targets_and_protect_worker_identity() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let request = open_compute_core::RequestId::generate();
    let workers = WorkerRepository::new(storage.db());
    let (caller, _) = workers
        .create_worker(account, "service-caller", request, 1, 1_000_000)
        .unwrap();
    let (target, _) = workers
        .create_worker(account, "service-target", request, 2, 1_000_000)
        .unwrap();
    let target_v1 = insert_ready(&workers, account, target.id, [1; 32], request, 3);
    workers
        .promote(account, target.id, target_v1, None, request, 5)
        .unwrap();

    let caller_deployment = DeploymentId::generate();
    let descriptor = [7; 32];
    let service = crate::NewDeploymentService {
        binding_name: "CATALOG".to_owned(),
        target_worker_id: target.id,
        entrypoint: Some("CatalogApi".to_owned()),
        descriptor_sha256: descriptor,
    };
    let self_service = crate::NewDeploymentService {
        binding_name: "SELF".to_owned(),
        target_worker_id: caller.id,
        entrypoint: None,
        descriptor_sha256: [6; 32],
    };
    let declarations = [service, self_service];
    workers
        .insert_staging_deployment(
            &NewDeployment {
                id: caller_deployment,
                account_id: account,
                worker_id: caller.id,
                content_kind: crate::DeploymentContentKind::Worker,
                artifact_sha256: Some([2; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                compatibility_date: "2026-08-22".to_owned(),
                compatibility_flags: vec!["rpc".to_owned()],
                limits: serde_json::json!({"profile":"default"}),
                worker_code_sha256: [3; 32],
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: request,
                now_ms: 6,
            },
            &crate::NewDeploymentProducts {
                services: &declarations,
                ..crate::NewDeploymentProducts::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(caller_deployment).unwrap();
    workers.mark_ready(caller_deployment, 7).unwrap();
    workers
        .promote(account, caller.id, caller_deployment, None, request, 8)
        .unwrap();

    let services = crate::ServiceRepository::new(storage.db());
    let first = services
        .resolve(caller_deployment, "CATALOG", &descriptor)
        .unwrap();
    assert_eq!(first.target_deployment_id, target_v1);
    assert_eq!(first.service.entrypoint.as_deref(), Some("CatalogApi"));
    assert_eq!(
        services.inbound_referrers(account, target.id, 10).unwrap(),
        vec![crate::ServiceReferrer {
            caller_worker_id: caller.id,
            caller_deployment_id: caller_deployment,
            binding_name: "CATALOG".to_owned(),
        }]
    );
    assert_eq!(
        workers
            .delete_worker(account, target.id, &[target_v1], request, 9)
            .unwrap_err()
            .code(),
        ErrorCode::ServiceTargetReferenced
    );

    let target_v2 = insert_ready(&workers, account, target.id, [4; 32], request, 10);
    workers
        .promote(account, target.id, target_v2, Some(target_v1), request, 12)
        .unwrap();
    assert_eq!(
        services
            .resolve(caller_deployment, "CATALOG", &descriptor)
            .unwrap()
            .target_deployment_id,
        target_v2
    );
    assert_eq!(
        services
            .resolve(caller_deployment, "CATALOG", &[8; 32])
            .unwrap_err()
            .code(),
        ErrorCode::ServiceBindingDenied
    );
    assert_eq!(
        workers
            .delete_worker(account, caller.id, &[], request, 13)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentReferenced
    );
    workers
        .delete_worker(account, caller.id, &[caller_deployment], request, 14)
        .unwrap();
}

#[test]
fn queue_consumer_unique_index_serializes_concurrent_worker_attachments() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = crate::QueueConfig::default();
    let queues = crate::QueueRepository::new(storage.db());
    queues
        .insert_creating(account, queue_id, "one-consumer", queue_config, 1)
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let workers = WorkerRepository::new(storage.db());
    let request = open_compute_core::RequestId::generate();
    let (first_worker, _) = workers
        .create_worker(account, "consumer-race-a", request, 3, 1_000_000)
        .unwrap();
    let (second_worker, _) = workers
        .create_worker(account, "consumer-race-b", request, 4, 1_000_000)
        .unwrap();
    let create_declaration = |worker_id: WorkerId, now_ms: i64| {
        let deployment_id = DeploymentId::generate();
        let declaration_id = QueueConsumerId::generate();
        workers
            .insert_staging_deployment(
                &NewDeployment {
                    id: deployment_id,
                    account_id: account,
                    worker_id,
                    content_kind: crate::DeploymentContentKind::Worker,
                    artifact_sha256: Some([5; 32]),
                    artifact_size: Some(100),
                    artifact_schema_version: Some(1),
                    main_module: Some("index.js".to_owned()),
                    compatibility_date: "2026-08-22".to_owned(),
                    compatibility_flags: Vec::new(),
                    limits: serde_json::json!({"profile":"default"}),
                    worker_code_sha256: [6; 32],
                    vars: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                    request_id: request,
                    now_ms,
                },
                &crate::NewDeploymentProducts {
                    queue_consumers: &[NewQueueConsumerDeclaration {
                        id: declaration_id,
                        queue_id,
                        queue_lifecycle_generation: 1,
                        entrypoint: None,
                        config: QueueConsumerConfig::default(),
                        dead_letter_queue: None,
                        capability_version: 1,
                        descriptor_sha256: [7; 32],
                    }],
                    ..Default::default()
                },
                10,
            )
            .unwrap();
        workers.begin_validation(deployment_id).unwrap();
        workers.mark_ready(deployment_id, now_ms + 1).unwrap();
        QueueConsumerRepository::new(storage.db())
            .declaration(declaration_id)
            .unwrap()
    };
    let first = create_declaration(first_worker.id, 10);
    let second = create_declaration(second_worker.id, 20);
    let first_db = crate::ControlDb::open(&root.join("control.sqlite"), 5_000).unwrap();
    let second_db = crate::ControlDb::open(&root.join("control.sqlite"), 5_000).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let results = thread::scope(|scope| {
        let first_barrier = barrier.clone();
        let first_handle = scope.spawn(move || {
            first_barrier.wait();
            QueueConsumerRepository::new(&first_db).create_attachment(
                account,
                first_worker.id,
                &first,
                30,
            )
        });
        let second_barrier = barrier.clone();
        let second_handle = scope.spawn(move || {
            second_barrier.wait();
            QueueConsumerRepository::new(&second_db).create_attachment(
                account,
                second_worker.id,
                &second,
                30,
            )
        });
        [first_handle.join().unwrap(), second_handle.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let failure = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert_eq!(failure.code(), ErrorCode::QueueConsumerConflict);
    assert_eq!(
        QueueConsumerRepository::new(storage.db())
            .list_live(10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn worker_repository_rejects_invalid_state_and_ownership_operations() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let repo = WorkerRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let request = open_compute_core::RequestId::generate();

    assert_eq!(
        repo.create_worker(
            AccountId::generate(),
            "missing-account",
            request,
            1,
            1_000_000
        )
        .unwrap_err()
        .code(),
        ErrorCode::AccountNotFound
    );

    let (worker, _) = repo
        .create_worker(account, "state-matrix", request, 2, 1_000_000)
        .unwrap();
    let ready = insert_ready(&repo, account, worker.id, [3; 32], request, 10);
    assert_eq!(
        repo.mark_rejected(ready, DeploymentState::Ready, ErrorCode::BundleInvalid, 12)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentInvariantViolation
    );
    assert_eq!(
        repo.begin_validation(DeploymentId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotReady
    );
    assert_eq!(
        repo.promote(
            account,
            worker.id,
            DeploymentId::generate(),
            None,
            request,
            13,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DeploymentNotFound
    );

    let staging = DeploymentId::generate();
    repo.insert_staging_deployment(
        &NewDeployment {
            id: staging,
            account_id: account,
            worker_id: worker.id,
            content_kind: crate::DeploymentContentKind::Worker,
            artifact_sha256: Some([4; 32]),
            artifact_size: Some(100),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            compatibility_date: "2026-08-22".to_owned(),
            compatibility_flags: Vec::new(),
            limits: serde_json::json!({"profile":"default"}),
            worker_code_sha256: [4; 32],
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            request_id: request,
            now_ms: 14,
        },
        &crate::NewDeploymentProducts::default(),
        1_000_000,
    )
    .unwrap();
    assert_eq!(
        repo.promote(account, worker.id, staging, None, request, 15)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotReady
    );
    let foreign_account = AccountId::generate();
    storage
        .db()
        .with_immediate(|transaction| {
            transaction
                .execute(
                    "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms)
                     VALUES (?1, ?2, 1, NULL)",
                    rusqlite::params![
                        foreign_account.to_string(),
                        format!("foreign-{foreign_account}")
                    ],
                )
                .map_err(|_| {
                    open_compute_core::PlatformError::new(
                        ErrorCode::Internal,
                        "test account insert",
                    )
                })?;
            Ok(())
        })
        .unwrap();
    let (foreign_worker, _) = repo
        .create_worker(foreign_account, "foreign", request, 16, 1_000_000)
        .unwrap();
    let foreign_ready = insert_ready(
        &repo,
        foreign_account,
        foreign_worker.id,
        [5; 32],
        request,
        17,
    );
    assert_eq!(
        repo.promote(account, worker.id, foreign_ready, None, request, 19)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotFound
    );
    assert_eq!(
        repo.promote(
            AccountId::generate(),
            worker.id,
            foreign_ready,
            None,
            request,
            19,
        )
        .unwrap_err()
        .code(),
        ErrorCode::WorkerNotFound
    );
    assert_eq!(
        repo.add_deployment_referrer(staging, "control_idempotency", "ref", 16)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotReady
    );
    assert_eq!(
        repo.add_deployment_referrer(DeploymentId::generate(), "control_idempotency", "ref", 16,)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotReady
    );

    let fingerprint = [9; 32];
    assert_eq!(
        repo.complete_idempotency(account, "scope", "missing", &fingerprint, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repo.complete_idempotency_with_deployment_ref(
            account,
            "scope",
            "missing",
            &fingerprint,
            b"{}",
            ready,
            "wrong-ref",
            17,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DeploymentInvariantViolation
    );
    let expected_ref = crate::workers::idempotency_ref_id(account, "scope", "missing");
    assert_eq!(
        repo.complete_idempotency_with_deployment_ref(
            account,
            "scope",
            "missing",
            &fingerprint,
            b"{}",
            ready,
            &expected_ref,
            17,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repo.fail_idempotency(account, "scope", "missing", &fingerprint, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );

    assert_eq!(
        repo.begin_deployment_delete(account, worker.id, DeploymentId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotFound
    );
    repo.begin_deployment_delete(account, worker.id, ready)
        .unwrap();
    repo.begin_deployment_delete(account, worker.id, ready)
        .unwrap();
    assert_eq!(
        repo.finalize_deployment_delete(account, worker.id, staging, request, 18)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotFound
    );

    assert_eq!(
        repo.deployment_snapshot(account, worker.id, staging, false)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentNotReady
    );
    let promotable = insert_ready(&repo, account, worker.id, [10; 32], request, 19);
    assert_eq!(
        repo.promote_checked(
            account,
            worker.id,
            promotable,
            Some(DeploymentId::generate()),
            None,
            request,
            19,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    repo.promote(account, worker.id, promotable, None, request, 20)
        .unwrap();
    assert_eq!(
        repo.create_exact_route(
            account,
            worker.id,
            "conflict.example",
            "/",
            None,
            Some(DeploymentId::generate()),
            request,
            21,
            1_000_000,
        )
        .unwrap_err()
        .code(),
        ErrorCode::IdempotencyConflict
    );
    let route = repo
        .create_exact_route(
            account,
            worker.id,
            "conflict.example",
            "/",
            None,
            Some(promotable),
            request,
            22,
            1_000_000,
        )
        .unwrap();
    assert_eq!(
        repo.create_exact_route(
            account,
            worker.id,
            "conflict.example",
            "/",
            None,
            Some(promotable),
            request,
            23,
            1_000_000,
        )
        .unwrap_err()
        .code(),
        ErrorCode::RouteConflict
    );
    assert_eq!(
        repo.delete_route(account, worker.id, "missing-route", request, 24)
            .unwrap_err()
            .code(),
        ErrorCode::RouteNotFound
    );
    repo.delete_route(account, worker.id, &route.id, request, 25)
        .unwrap();

    let invalid_state_fingerprint = [11; 32];
    repo.reserve_idempotency(
        account,
        "invalid-state",
        "key",
        "fingerprint-key",
        &invalid_state_fingerprint,
        26,
        100,
    )
    .unwrap();
    storage
        .db()
        .with_read(|conn| {
            conn.execute(
                "UPDATE control_idempotency SET state = 'complete', response_json = NULL
                 WHERE account_id = ?1 AND scope = 'invalid-state' AND idempotency_key = 'key'",
                [account.to_string()],
            )
            .map_err(|_| {
                open_compute_core::PlatformError::new(ErrorCode::Internal, "test update failed")
            })?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.reserve_idempotency(
            account,
            "invalid-state",
            "key",
            "fingerprint-key",
            &invalid_state_fingerprint,
            27,
            100,
        )
        .unwrap_err()
        .code(),
        ErrorCode::Internal
    );

    let referenced = insert_ready(&repo, account, worker.id, [12; 32], request, 30);
    repo.begin_deployment_delete(account, worker.id, referenced)
        .unwrap();
    storage
        .db()
        .with_read(|conn| {
            conn.execute(
                "INSERT INTO deployment_referrers
                 (deployment_id, kind, ref_id, created_at_ms) VALUES (?1, 'test', 'late', 31)",
                [referenced.to_string()],
            )
            .map_err(|_| {
                open_compute_core::PlatformError::new(ErrorCode::Internal, "test insert failed")
            })?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        repo.finalize_deployment_delete(account, worker.id, referenced, request, 32)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentReferenced
    );

    let tombstone = insert_ready(&repo, account, worker.id, [13; 32], request, 33);
    repo.tombstone_deployment(account, worker.id, tombstone, request, 34)
        .unwrap();

    for args in [(0, 1, 1), (1, 0, 1), (1, 1, 0), (1, 1, 10_001)] {
        assert_eq!(
            repo.retention_candidates(40, 0, args.0, args.1, args.2)
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
    }

    let expected = repo
        .list_deployments(account, worker.id)
        .unwrap()
        .into_iter()
        .filter(|deployment| deployment.deleted_at_ms.is_none())
        .map(|deployment| deployment.id)
        .collect::<Vec<_>>();
    repo.delete_worker(account, worker.id, &expected, request, 41)
        .unwrap();
    assert_eq!(
        repo.deployment_snapshot(account, worker.id, ready, false)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerDeleted
    );
    assert_eq!(
        repo.delete_worker(account, worker.id, &[], request, 42)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerDeleted
    );

    for invalid in [0, 10_001] {
        assert_eq!(
            repo.recover_deleting_deployments(request, 19, invalid)
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
        assert_eq!(
            repo.prune_expired_idempotency(19, invalid)
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
    }
}

fn inspect_schema_after_raw_sql(sql: &str) -> ErrorCode {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("control.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(sql).unwrap();
    drop(conn);
    let db = crate::ControlDb::open(&path, 100).unwrap();
    crate::migrations::inspect_schema(&db).unwrap_err().code()
}

#[test]
fn schema_consistency_rejects_missing_malformed_and_duplicate_rows() {
    assert_eq!(
        inspect_schema_after_raw_sql("PRAGMA user_version = 1;"),
        ErrorCode::MigrationFailed
    );

    let checksum_1 = hex::encode(crate::migrations::migration_001_checksum());
    let checksum_2 = hex::encode(crate::migrations::migration_002_checksum());
    let cases = [
        format!(
            "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
             INSERT INTO schema_migrations VALUES(2, X'{checksum_2}');\
             PRAGMA user_version = 1;"
        ),
        "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
         INSERT INTO schema_migrations VALUES(0, X'00');\
         PRAGMA user_version = 2;"
            .to_owned(),
        format!(
            "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
             INSERT INTO schema_migrations VALUES(1, X'{checksum_1}');\
             INSERT INTO schema_migrations VALUES(1, X'{checksum_1}');\
             PRAGMA user_version = 1;"
        ),
        format!(
            "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 BLOB);\
             INSERT INTO schema_migrations VALUES(2, X'{checksum_2}');\
             PRAGMA user_version = 2;"
        ),
        "CREATE TABLE schema_migrations(version INTEGER); PRAGMA user_version = 1;".to_owned(),
        "CREATE TABLE schema_migrations(version INTEGER, checksum_sha256 TEXT);\
         INSERT INTO schema_migrations VALUES(1, 'not-a-blob');\
         PRAGMA user_version = 1;"
            .to_owned(),
    ];
    for sql in cases {
        assert_eq!(
            inspect_schema_after_raw_sql(&sql),
            ErrorCode::MigrationFailed,
            "{sql}"
        );
    }
}

#[test]
fn bootstrap_with_no_fault_matches_normal_bootstrap_and_rejects_nonregular_staging() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap_with_fault(&config, &SystemClock, None).unwrap();
    let staging = storage.data_dir().deployment_staging_dir();
    drop(storage);

    fs::create_dir(staging.join("nested")).unwrap();
    assert_eq!(
        DataDir::acquire(&config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    fs::remove_dir(staging.join("nested")).unwrap();

    let target = _tmp.path().join("outside");
    fs::write(&target, b"outside").unwrap();
    std::os::unix::fs::symlink(&target, staging.join("link")).unwrap();
    assert_eq!(
        DataDir::acquire(&config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn control_db_operations_fail_closed_when_foreign_keys_are_disabled() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("control.sqlite");
    let db = crate::ControlDb::open(&path, 100).unwrap();
    db.migrate(&SystemClock).unwrap();
    db.with_read(|conn| {
        conn.pragma_update(None, "foreign_keys", "OFF")
            .map_err(|_| open_compute_core::PlatformError::new(ErrorCode::Internal, "test"))?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        db.quick_check().unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.user_version().unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.with_read(|_| Ok(())).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.with_immediate(|_| Ok(())).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.with_exclusive(|_| Ok(())).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.table_exists("schema_migrations").unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.migrate(&SystemClock).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        db.migrate_with_fault(&SystemClock, None)
            .unwrap_err()
            .code(),
        ErrorCode::MigrationFailed
    );
}

fn p1_release_identity() -> PlatformReleaseIdentityV1 {
    PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: env!("CARGO_PKG_VERSION").to_owned(),
        git_revision: "test".to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: "workerd test".to_owned(),
        workerd_lock_sha256: "a".repeat(64),
        runtime_assets_sha256: "b".repeat(64),
        facade_capability_version: 1,
        control_schema_version: u32::try_from(crate::migrations::current_schema_version()).unwrap(),
        scheduler_schema_version: u32::try_from(crate::current_scheduler_schema_version()).unwrap(),
        kv_schema_version: crate::KV_SCHEMA_VERSION,
        d1_schema_version: crate::D1_DATABASE_SCHEMA_VERSION,
        snapshot_format_version: 1,
        compatibility_policy_sha256: "c".repeat(64),
    }
}

#[test]
fn p1_offline_snapshot_is_standalone_authenticated_and_rejects_do_symlinks() {
    let (tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let do_root = storage
        .data_dir()
        .prepare_durable_object_storage(&storage.identity().platform_id.to_string(), "workerd test")
        .unwrap();
    let do_file = do_root.join("state.bin");
    fs::write(&do_file, b"opaque-do-state").unwrap();
    fs::set_permissions(&do_file, fs::Permissions::from_mode(0o600)).unwrap();
    let outside = tmp.path().join("outside");
    fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, do_root.join("forbidden-link")).unwrap();
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&config).unwrap();
    let key = crate::inspect_master_key(&config).unwrap();
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let hardening = HardeningConfig::default();
    let mut request = crate::PreparePlatformSnapshotRequest {
        snapshot_id: &snapshot_id,
        label: "p1-test",
        created_at_ms: 1,
        release: p1_release_identity(),
        master_key_fingerprint: key.fingerprint(),
        s3_authority_fingerprint: &"d".repeat(64),
        r2_prefix_fingerprint: &"e".repeat(64),
        config_policy_sha256: &"f".repeat(64),
        object_prefix: &format!(
            "system/snapshots/v1/{}/{snapshot_id}/objects/",
            crate::inspect_control_db(&data_dir.control_db_path(), 5_000)
                .unwrap()
                .1
                .platform_id
        ),
        hardening: &hardening,
        sqlite_busy_timeout_ms: 5_000,
    };
    let wrong_fingerprint = "0".repeat(64);
    let mut wrong_key_request = request.clone();
    wrong_key_request.master_key_fingerprint = &wrong_fingerprint;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &wrong_key_request)
            .unwrap_err()
            .code(),
        ErrorCode::MasterKeyMismatch
    );

    let mut wrong_schema_request = request.clone();
    wrong_schema_request.release.control_schema_version += 1;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &wrong_schema_request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );

    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    let staging = data_dir
        .backup_staging_dir()
        .join(format!("platform-{snapshot_id}"));
    assert!(
        !staging.exists(),
        "staging entries: {:?}",
        fs::read_dir(&staging)
            .map(|entries| entries
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>())
            .ok()
    );
    fs::remove_file(do_root.join("forbidden-link")).unwrap();
    request.release.control_schema_version = 8;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    request.release.control_schema_version =
        u32::try_from(crate::migrations::current_schema_version()).unwrap();
    let mut prepared = crate::prepare_platform_snapshot(&data_dir, &request).unwrap();
    assert!(prepared.manifest.files.iter().any(|file| {
        file.role == open_compute_core::SnapshotFileRole::DurableObjectFile
            && file.restore_path.ends_with("state.bin")
    }));
    crate::sign_snapshot_manifest(&mut prepared.manifest, &key).unwrap();
    crate::verify_snapshot_manifest_mac(&prepared.manifest, &key).unwrap();
    prepared.manifest.label.push('x');
    assert_eq!(
        crate::verify_snapshot_manifest_mac(&prepared.manifest, &key)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    let largest_file = prepared
        .manifest
        .files
        .iter()
        .map(|file| file.size)
        .max()
        .unwrap();
    let total_bytes = prepared.manifest.totals.bytes;
    drop(prepared);

    let file_limited = HardeningConfig {
        max_snapshot_file_bytes: largest_file - 1,
        max_snapshot_total_bytes: total_bytes,
        ..HardeningConfig::default()
    };
    request.hardening = &file_limited;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
    let staging = data_dir
        .backup_staging_dir()
        .join(format!("platform-{snapshot_id}"));
    assert!(
        !staging.exists(),
        "staging entries: {:?}",
        fs::read_dir(&staging)
            .map(|entries| entries
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>())
            .ok()
    );

    let total_limited = HardeningConfig {
        max_snapshot_file_bytes: largest_file,
        max_snapshot_total_bytes: total_bytes - 1,
        ..HardeningConfig::default()
    };
    request.hardening = &total_limited;
    assert_eq!(
        crate::prepare_platform_snapshot(&data_dir, &request)
            .unwrap_err()
            .code(),
        ErrorCode::SnapshotInvalid
    );
}

#[test]
fn p1_admission_lock_restore_target_and_current_schema_fail_closed() {
    let (tmp, root) = unique_root();
    let mut config = storage_config(&root);
    config.free_space_hard_bytes = u64::MAX - 1;
    let hardening = HardeningConfig {
        emergency_reserve_bytes: 1,
        ..HardeningConfig::default()
    };
    let storage =
        PlatformStorage::bootstrap_with_hardening(&config, &hardening, &SystemClock).unwrap();
    assert_eq!(
        storage.reserve_mutation(1).unwrap_err().code(),
        ErrorCode::StoragePressure
    );
    assert_eq!(
        DataDir::acquire_existing_offline(&config)
            .unwrap_err()
            .code(),
        ErrorCode::DataDirInUse
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    drop(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&config).unwrap();
    let control =
        crate::ControlDb::open_readonly_wal_aware(&data_dir.control_db_path(), 5_000).unwrap();
    let current = crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap();
    assert_eq!(
        i64::from(current.control),
        crate::migrations::current_schema_version()
    );
    assert_eq!(
        crate::inspect_current_schema(&data_dir, &control, 5_000).unwrap(),
        current
    );
    drop(control);
    drop(data_dir);

    let target = fs::canonicalize(tmp.path()).unwrap().join("restored");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("occupied"), b"x").unwrap();
    assert_eq!(
        crate::RestoreTarget::acquire(&target).unwrap_err().code(),
        ErrorCode::RestoreInvalid
    );
    fs::remove_file(target.join("occupied")).unwrap();
    let restore = crate::RestoreTarget::acquire(&target).unwrap();
    assert!(restore.staging_root().starts_with(target.parent().unwrap()));
    assert!(restore.destination_for("../escape").is_err());
    assert_eq!(
        crate::RestoreTarget::acquire(&target).unwrap_err().code(),
        ErrorCode::DataDirInUse
    );
    let nested = restore.destination_for("do/workerd/failure.bin").unwrap();
    fs::write(&nested, b"retained restore bytes").unwrap();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(restore.destination_for("do/workerd/failure.bin").is_err());
    let staging_name = restore
        .staging_root()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let staging_id = staging_name.rsplit_once(".restore-").unwrap().1.to_owned();
    drop(restore);
    let cleaned =
        crate::cleanup_restore_staging(&target, &staging_id, 10, 1024, 10 * 1024).unwrap();
    assert_eq!(cleaned.files, 1);
    assert!(!target.parent().unwrap().join(staging_name).exists());

    let real_parent = fs::canonicalize(tmp.path()).unwrap().join("restore-parent");
    fs::create_dir(&real_parent).unwrap();
    let alias_parent = fs::canonicalize(tmp.path())
        .unwrap()
        .join("restore-parent-alias");
    std::os::unix::fs::symlink(&real_parent, &alias_parent).unwrap();
    assert_eq!(
        crate::RestoreTarget::acquire(&alias_parent.join("target"))
            .unwrap_err()
            .code(),
        ErrorCode::RestoreInvalid
    );
}

#[test]
fn p2_2_queue_catalog_projection_and_config_fences_are_exact() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap();
    let account_id = storage.identity().default_account_id;
    let queue_id = open_compute_core::QueueId::generate();
    let repository = crate::QueueRepository::new(storage.db());
    let queue = repository
        .insert_creating(
            account_id,
            queue_id,
            "events",
            crate::QueueConfig::default(),
            10,
        )
        .unwrap();
    assert_eq!(queue.state, crate::QueueState::Creating);
    assert_eq!(queue.availability, crate::QueueAvailability::Degraded);
    let projection = crate::QueueProjection {
        queue_id,
        account_id,
        lifecycle_generation: queue.lifecycle_generation,
        config_generation: queue.config_generation,
        config: queue.config,
        created_at_ms: queue.created_at_ms,
        updated_at_ms: queue.updated_at_ms,
    };
    scheduler.create_queue_projection(&projection).unwrap();
    scheduler.verify_queue_projection(&projection).unwrap();
    let ready = repository.mark_ready(account_id, queue_id, 11).unwrap();
    assert_eq!(ready.state, crate::QueueState::Ready);
    assert_eq!(ready.availability, crate::QueueAvailability::Healthy);
    assert_eq!(
        repository
            .insert_creating(
                account_id,
                open_compute_core::QueueId::generate(),
                "events",
                crate::QueueConfig::default(),
                12,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueNameConflict
    );
    let raw = Connection::open(storage.data_dir().control_db_path()).unwrap();
    assert!(
        raw.execute(
            "UPDATE queues SET delivery_delay_seconds = 4 WHERE id = ?1",
            [queue_id.to_string()],
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "UPDATE queues SET config_generation = config_generation + 1 WHERE id = ?1",
            [queue_id.to_string()],
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "UPDATE queues SET name = 'combined', delivery_delay_seconds = 4,
                    config_generation = config_generation + 1,
                    availability = 'degraded', availability_code = 'QUEUE_CONFIG_PENDING'
             WHERE id = ?1",
            [queue_id.to_string()],
        )
        .is_err()
    );
    drop(raw);

    scheduler.begin_queue_config(queue_id, 1, 1, 20).unwrap();
    assert_eq!(
        scheduler
            .enqueue_queue(
                &crate::QueueEnqueueRequest {
                    queue_id,
                    lifecycle_generation: 1,
                    config_generation: 1,
                    batch_delay_seconds: None,
                    messages: vec![crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Text,
                        body: b"blocked".to_vec(),
                        delay_seconds: None,
                    }],
                },
                20,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    let mut next_config = ready.config;
    next_config.delivery_delay_seconds = 9;
    next_config.max_backlog_bytes = 4096;
    let pending = repository
        .write_config_pending(account_id, queue_id, 1, next_config, 21)
        .unwrap();
    let next_projection = crate::QueueProjection {
        queue_id,
        account_id,
        lifecycle_generation: 1,
        config_generation: 2,
        config: next_config,
        created_at_ms: pending.created_at_ms,
        updated_at_ms: pending.updated_at_ms,
    };
    scheduler.project_queue_config(&next_projection).unwrap();
    let healthy = repository
        .mark_config_healthy(
            account_id,
            queue_id,
            2,
            open_compute_core::RequestId::generate(),
            22,
        )
        .unwrap();
    scheduler.finish_queue_config(queue_id, 1, 2, 23).unwrap();
    scheduler.verify_queue_projection(&next_projection).unwrap();
    assert_eq!(healthy.config_generation, 2);
    assert_eq!(healthy.config.delivery_delay_seconds, 9);
}

#[test]
fn p2_2_queue_enqueue_delay_quota_retention_and_counters_are_transactional() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap();
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = crate::QueueConfig {
        delivery_delay_seconds: 7,
        retention_seconds: 60,
        max_backlog_bytes: 8,
        ..crate::QueueConfig::default()
    };
    scheduler
        .create_queue_projection(&crate::QueueProjection {
            queue_id,
            account_id: storage.identity().default_account_id,
            lifecycle_generation: 1,
            config_generation: 1,
            config: queue_config,
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        })
        .unwrap();
    let result = scheduler
        .enqueue_queue(
            &crate::QueueEnqueueRequest {
                queue_id,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: Some(3),
                messages: vec![
                    crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Json,
                        body: b"{}".to_vec(),
                        delay_seconds: None,
                    },
                    crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Bytes,
                        body: vec![1, 2, 3],
                        delay_seconds: Some(0),
                    },
                ],
            },
            1_000,
        )
        .unwrap();
    assert_eq!(result.message_ids.len(), 2);
    assert_eq!(result.metrics.backlog_count, 2);
    assert_eq!(result.metrics.backlog_bytes, 5);
    let reader = Connection::open(&scheduler_path).unwrap();
    let rows = reader
        .prepare(
            "SELECT available_at_ms, expires_at_ms, content_type, body
             FROM queue_messages WHERE queue_id = ?1 ORDER BY seq",
        )
        .unwrap()
        .query_map([queue_id.to_string()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows[0], (4_000, 61_000, "json".to_owned(), b"{}".to_vec()));
    assert_eq!(rows[1], (1_000, 61_000, "bytes".to_owned(), vec![1, 2, 3]));
    assert_eq!(
        scheduler
            .enqueue_queue(
                &crate::QueueEnqueueRequest {
                    queue_id,
                    lifecycle_generation: 1,
                    config_generation: 1,
                    batch_delay_seconds: None,
                    messages: vec![crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Text,
                        body: b"more".to_vec(),
                        delay_seconds: None,
                    }],
                },
                2_000,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueBacklogLimitExceeded
    );
    assert_eq!(
        scheduler
            .queue_metrics(queue_id, 1, 1)
            .unwrap()
            .backlog_count,
        2
    );
    let swept = scheduler.sweep_queue_retention(61_000, 100, 1024).unwrap();
    assert_eq!(swept.messages, 2);
    assert_eq!(swept.bytes, 5);
    assert_eq!(
        scheduler
            .queue_metrics(queue_id, 1, 1)
            .unwrap()
            .backlog_count,
        0
    );
    assert!(scheduler.queue_counter_mismatches().unwrap().is_empty());
    drop(reader);
    drop(scheduler);
    let inspection = crate::inspect_scheduler_db(&scheduler_path, 5_000, 61_000).unwrap();
    assert_eq!(
        inspection.schema_version,
        crate::current_scheduler_schema_version()
    );
    assert_eq!(inspection.queue.queues, 1);
    assert_eq!(inspection.queue.backlog_messages, 0);
    assert_eq!(inspection.queue.counter_mismatches, 0);
}

#[test]
fn p2_2_concurrent_queue_enqueues_never_exceed_backlog_quota() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(crate::SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let queue_id = open_compute_core::QueueId::generate();
    scheduler
        .create_queue_projection(&crate::QueueProjection {
            queue_id,
            account_id: storage.identity().default_account_id,
            lifecycle_generation: 1,
            config_generation: 1,
            config: crate::QueueConfig {
                max_backlog_bytes: 10,
                ..crate::QueueConfig::default()
            },
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for _ in 0..8 {
        let scheduler = scheduler.clone();
        let barrier = barrier.clone();
        threads.push(thread::spawn(move || {
            barrier.wait();
            scheduler.enqueue_queue(
                &crate::QueueEnqueueRequest {
                    queue_id,
                    lifecycle_generation: 1,
                    config_generation: 1,
                    batch_delay_seconds: None,
                    messages: vec![crate::QueueMessageInput {
                        content_type: crate::QueueContentType::Bytes,
                        body: vec![1, 2, 3],
                        delay_seconds: Some(0),
                    }],
                },
                2,
            )
        }));
    }
    barrier.wait();
    let mut accepted = 0_u64;
    for thread in threads {
        match thread.join().unwrap() {
            Ok(_) => accepted += 1,
            Err(error) => assert_eq!(error.code(), ErrorCode::QueueBacklogLimitExceeded),
        }
    }
    assert_eq!(accepted, 3);
    let metrics = scheduler.queue_metrics(queue_id, 1, 1).unwrap();
    assert_eq!(metrics.backlog_count, 3);
    assert_eq!(metrics.backlog_bytes, 9);
    assert!(scheduler.queue_counter_mismatches().unwrap().is_empty());
}

#[test]
fn p2_2_queue_catalog_idempotency_and_failure_boundaries_are_complete() {
    let (_tmp, root) = unique_root();
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repository = crate::QueueRepository::new(storage.db());
    let workers = WorkerRepository::new(storage.db());
    let fingerprint = [7_u8; 32];
    let other_fingerprint = [8_u8; 32];

    assert_eq!(crate::QueueState::Creating.as_str(), "creating");
    assert_eq!(crate::QueueState::Ready.as_str(), "ready");
    assert_eq!(crate::QueueState::Deleting.as_str(), "deleting");
    assert_eq!(crate::QueueState::Tombstoned.as_str(), "tombstoned");
    assert_eq!(
        "creating".parse::<crate::QueueState>().unwrap(),
        crate::QueueState::Creating
    );
    assert_eq!(
        "tombstoned".parse::<crate::QueueState>().unwrap(),
        crate::QueueState::Tombstoned
    );
    assert_eq!(
        "invalid".parse::<crate::QueueState>().unwrap_err().code(),
        ErrorCode::QueueInvariantViolation
    );
    assert_eq!(crate::QueueAvailability::Healthy.as_str(), "healthy");
    assert_eq!(crate::QueueAvailability::Degraded.as_str(), "degraded");
    assert_eq!(
        crate::QueueAvailability::Unavailable.as_str(),
        "unavailable"
    );
    assert_eq!(
        "degraded".parse::<crate::QueueAvailability>().unwrap(),
        crate::QueueAvailability::Degraded
    );
    assert_eq!(
        "invalid"
            .parse::<crate::QueueAvailability>()
            .unwrap_err()
            .code(),
        ErrorCode::QueueInvariantViolation
    );

    for invalid in [
        crate::QueueConfig {
            delivery_delay_seconds: crate::QUEUE_MAX_DELAY_SECONDS + 1,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            retention_seconds: crate::QUEUE_MIN_RETENTION_SECONDS - 1,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_message_bytes: 0,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_batch_messages: 0,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_batch_bytes: 0,
            ..crate::QueueConfig::default()
        },
        crate::QueueConfig {
            max_backlog_bytes: 0,
            ..crate::QueueConfig::default()
        },
    ] {
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            ErrorCode::LimitInvalid
        );
    }
    for name in ["", "bad\nname"] {
        assert_eq!(
            repository
                .insert_creating(
                    account,
                    open_compute_core::QueueId::generate(),
                    name,
                    crate::QueueConfig::default(),
                    1,
                )
                .unwrap_err()
                .code(),
            ErrorCode::ConfigInvalid
        );
    }
    assert_eq!(
        repository.list(account, None, 0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repository.list_reconcile(1001).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repository.list_running_mutations(0).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        repository
            .insert_creating(
                AccountId::generate(),
                open_compute_core::QueueId::generate(),
                "orphan",
                crate::QueueConfig::default(),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::AccountNotFound
    );

    let running_id = open_compute_core::QueueId::generate();
    let running = repository
        .reserve_create(
            account,
            running_id,
            "running",
            crate::QueueConfig::default(),
            "create-running",
            "key",
            &fingerprint,
            10,
            100,
            10,
        )
        .unwrap();
    assert!(matches!(
        running,
        crate::QueueCreateReservation::Reserved(_)
    ));
    assert_eq!(
        repository
            .reserve_create(
                account,
                running_id,
                "running",
                crate::QueueConfig::default(),
                "create-running",
                "key",
                &fingerprint,
                10,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Running
    );
    assert_eq!(
        repository
            .reserve_create(
                account,
                running_id,
                "running",
                crate::QueueConfig::default(),
                "create-running",
                "key",
                &other_fingerprint,
                10,
                100,
                10,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );

    let complete_id = open_compute_core::QueueId::generate();
    let complete = match repository
        .reserve_create(
            account,
            complete_id,
            "complete",
            crate::QueueConfig::default(),
            "create-complete",
            "key",
            &fingerprint,
            11,
            100,
            10,
        )
        .unwrap()
    {
        crate::QueueCreateReservation::Reserved(queue) => queue,
        other => panic!("unexpected reservation: {other:?}"),
    };
    repository
        .complete_reconciled_create(&complete, b"{\"complete\":true}")
        .unwrap();
    assert_eq!(
        repository
            .reserve_create(
                account,
                complete_id,
                "complete",
                crate::QueueConfig::default(),
                "create-complete",
                "key",
                &fingerprint,
                11,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Complete(b"{\"complete\":true}".to_vec())
    );

    let failed_id = open_compute_core::QueueId::generate();
    assert!(matches!(
        repository
            .reserve_create(
                account,
                failed_id,
                "failed",
                crate::QueueConfig::default(),
                "create-failed",
                "key",
                &fingerprint,
                12,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Reserved(_)
    ));
    workers
        .fail_idempotency(
            account,
            "queue.create",
            "create-failed",
            &fingerprint,
            b"{\"failed\":true}",
        )
        .unwrap();
    assert_eq!(
        repository
            .reserve_create(
                account,
                failed_id,
                "failed",
                crate::QueueConfig::default(),
                "create-failed",
                "key",
                &fingerprint,
                12,
                100,
                10,
            )
            .unwrap(),
        crate::QueueCreateReservation::Failed(b"{\"failed\":true}".to_vec())
    );
    assert_eq!(
        repository
            .reserve_create(
                account,
                open_compute_core::QueueId::generate(),
                "quota",
                crate::QueueConfig::default(),
                "create-quota",
                "key",
                &fingerprint,
                13,
                100,
                0,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QuotaExceeded
    );

    let lifecycle_id = open_compute_core::QueueId::generate();
    let lifecycle = repository
        .insert_creating(
            account,
            lifecycle_id,
            "lifecycle",
            crate::QueueConfig::default(),
            20,
        )
        .unwrap();
    assert_eq!(
        repository
            .get(AccountId::generate(), lifecycle_id)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotFound
    );
    assert_eq!(
        repository
            .rename(
                account,
                lifecycle_id,
                "too-early",
                open_compute_core::RequestId::generate(),
                21
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    let ready = repository.mark_ready(account, lifecycle_id, 22).unwrap();
    assert_eq!(ready.state, crate::QueueState::Ready);
    assert_eq!(
        repository
            .mark_ready(account, lifecycle_id, 23)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    assert_eq!(
        repository
            .write_config_pending(account, lifecycle_id, 9, ready.config, 24)
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    assert_eq!(
        repository
            .mark_config_healthy(
                account,
                lifecycle_id,
                1,
                open_compute_core::RequestId::generate(),
                25,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueConfigPending
    );
    assert_eq!(
        repository
            .begin_delete(account, lifecycle_id, 9, 26)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    repository
        .begin_delete(account, lifecycle_id, 1, 27)
        .unwrap();
    assert_eq!(
        repository
            .begin_delete(account, lifecycle_id, 1, 28)
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );
    repository
        .mark_tombstoned(
            account,
            lifecycle_id,
            open_compute_core::RequestId::generate(),
            29,
        )
        .unwrap();
    assert_eq!(
        repository
            .mark_tombstoned(
                account,
                lifecycle_id,
                open_compute_core::RequestId::generate(),
                30,
            )
            .unwrap_err()
            .code(),
        ErrorCode::QueueNotReady
    );

    let mutation_id = lifecycle.id;
    let mutation = crate::RunningQueueMutation {
        account_id: account,
        scope: format!("queue.patch:{mutation_id}"),
        idempotency_key: "mutation".to_owned(),
        request_fingerprint: fingerprint,
        queue_id: mutation_id,
        intent_json: b"{\"version\":1}".to_vec(),
    };
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap(),
        IdempotencyReservation::Reserved
    );
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap(),
        IdempotencyReservation::Running
    );
    assert_eq!(
        repository.list_running_mutations(10).unwrap(),
        vec![mutation.clone()]
    );
    repository
        .replace_mutation_intent(&mutation, b"{\"version\":1,\"changed\":true}")
        .unwrap();
    let mut wrong = mutation.clone();
    wrong.request_fingerprint = other_fingerprint;
    assert_eq!(
        repository
            .replace_mutation_intent(&wrong, b"{}")
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &other_fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    workers
        .complete_idempotency_with_queue_ref(
            account,
            &mutation.scope,
            &mutation.idempotency_key,
            &fingerprint,
            b"{\"done\":true}",
            mutation_id,
        )
        .unwrap();
    assert_eq!(
        repository
            .reserve_mutation(
                account,
                &mutation.scope,
                &mutation.idempotency_key,
                "key",
                &fingerprint,
                mutation_id,
                &mutation.intent_json,
                40,
                100,
            )
            .unwrap(),
        IdempotencyReservation::Complete(b"{\"done\":true}".to_vec())
    );
}

#[test]
fn p1_restore_cleanup_rejects_ambiguous_bounds_links_receipts_and_lock_owners() {
    let (tmp, _) = unique_root();
    let parent = fs::canonicalize(tmp.path()).unwrap();
    let target = parent.join("cleanup-target");
    let make_staging = |id: &str| {
        let staging = parent.join(format!(".cleanup-target.restore-{id}"));
        fs::create_dir(&staging).unwrap();
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
        staging
    };

    let valid_id = uuid::Uuid::now_v7().hyphenated().to_string();
    for result in [
        crate::cleanup_restore_staging(&target, "not-a-uuid", 1, 1, 1),
        crate::cleanup_restore_staging(&target, &valid_id, 0, 1, 1),
        crate::cleanup_restore_staging(&target, &valid_id, 1, 0, 1),
        crate::cleanup_restore_staging(&target, &valid_id, 1, 2, 1),
        crate::cleanup_restore_staging(Path::new("relative"), &valid_id, 1, 1, 1),
    ] {
        assert!(result.is_err());
    }

    let empty_id = uuid::Uuid::now_v7().hyphenated().to_string();
    make_staging(&empty_id);
    let empty = crate::cleanup_restore_staging(&target, &empty_id, 1, 1, 1).unwrap();
    assert_eq!(empty.files, 0);
    assert_eq!(empty.bytes, 0);

    let size_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let size_staging = make_staging(&size_id);
    fs::write(size_staging.join("control.sqlite"), b"large").unwrap();
    fs::set_permissions(
        size_staging.join("control.sqlite"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(crate::cleanup_restore_staging(&target, &size_id, 1, 4, 4).is_err());

    let count_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let count_staging = make_staging(&count_id);
    for name in ["control.sqlite", "scheduler.sqlite"] {
        fs::write(count_staging.join(name), b"x").unwrap();
        fs::set_permissions(count_staging.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(crate::cleanup_restore_staging(&target, &count_id, 1, 1, 2).is_err());

    let total_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let total_staging = make_staging(&total_id);
    for name in ["control.sqlite", "scheduler.sqlite"] {
        fs::write(total_staging.join(name), b"abc").unwrap();
        fs::set_permissions(total_staging.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(crate::cleanup_restore_staging(&target, &total_id, 2, 4, 5).is_err());

    let hardlink_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let hardlink_staging = make_staging(&hardlink_id);
    let first = hardlink_staging.join("control.sqlite");
    fs::write(&first, b"x").unwrap();
    fs::set_permissions(&first, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&first, hardlink_staging.join("scheduler.sqlite")).unwrap();
    assert!(crate::cleanup_restore_staging(&target, &hardlink_id, 2, 1, 2).is_err());

    let receipt_id = uuid::Uuid::now_v7().hyphenated().to_string();
    make_staging(&receipt_id);
    fs::create_dir(parent.join(format!(".cleanup-target.restore-failure-{receipt_id}.json")))
        .unwrap();
    assert!(crate::cleanup_restore_staging(&target, &receipt_id, 1, 1, 1).is_err());

    let held = crate::RestoreTarget::acquire(&target).unwrap();
    let held_name = held.staging_root().file_name().unwrap().to_str().unwrap();
    let held_id = held_name.rsplit_once(".restore-").unwrap().1;
    assert_eq!(
        crate::cleanup_restore_staging(&target, held_id, 1, 1, 1)
            .unwrap_err()
            .code(),
        ErrorCode::DataDirInUse
    );
}

#[test]
fn p1_concurrent_resource_creates_never_exceed_the_account_kind_limit() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for index in 0..8 {
        let storage = storage.clone();
        let barrier = barrier.clone();
        threads.push(thread::spawn(move || {
            let name = format!("p1-concurrent-{index}");
            let idempotency_key = format!("p1-concurrent-key-{index}");
            let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
            barrier.wait();
            ResourceRepository::new(storage.db()).reserve_create(
                &ReserveResourceCreate {
                    account_id: account,
                    kind: BindingKind::KvNamespace,
                    name: &name,
                    idempotency_key: &idempotency_key,
                    fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                    request_fingerprint: &fingerprint,
                    resource_id: ResourceId::generate(),
                    driver_schema_version: 1,
                    request_id: open_compute_core::RequestId::generate(),
                    now_ms: 1,
                    expires_at_ms: 1_001,
                },
                3,
            )
        }));
    }
    barrier.wait();
    let mut accepted = 0;
    let mut rejected = 0;
    for thread in threads {
        match thread.join().unwrap() {
            Ok(ResourceCreateReservation::Reserved(_)) => accepted += 1,
            Err(error) if error.code() == ErrorCode::QuotaExceeded => rejected += 1,
            other => panic!("unexpected concurrent resource result: {other:?}"),
        }
    }
    assert_eq!(accepted, 3);
    assert_eq!(rejected, 5);
    assert_eq!(
        ResourceRepository::new(storage.db())
            .list(account, Some(BindingKind::KvNamespace))
            .unwrap()
            .len(),
        3
    );
}
