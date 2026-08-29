use super::*;
use open_compute_core::{BindingKind, ErrorCode, ResourceId, ResourceState};
use open_compute_storage::{
    D1DatabaseRepository, D1Engine, D1Paths, KvEngine, KvNamespaceRepository, KvPaths,
    KvPutOptions, ReserveResourceCreate, ResourceCreateReservation, ResourceRecord,
    ResourceRepository, inspect_current_schema,
};
use open_compute_workers::ResourceDriver;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const QUOTA: u64 = 256 * 1024 * 1024;
const KINDS: [BindingKind; 2] = [BindingKind::KvNamespace, BindingKind::D1Database];

fn config() -> (tempfile::TempDir, PlatformConfig) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = PlatformConfig::default();
    config.storage.data_dir = temp.path().join("data");
    config.storage.master_key_file = config.storage.data_dir.join("keys/master.key");
    config.kv.namespace_quota_bytes = QUOTA;
    config.d1.database_quota_bytes = QUOTA;
    (temp, config)
}

fn reserve(
    storage: &PlatformStorage,
    kind: BindingKind,
    name: &str,
    version: u32,
) -> ResourceRecord {
    let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: storage.identity().default_account_id,
                kind,
                name,
                idempotency_key: name,
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id: ResourceId::generate(),
                driver_schema_version: version,
                request_id: RequestId::generate(),
                now_ms: 1,
                expires_at_ms: i64::MAX,
            },
            100,
        )
        .unwrap()
    else {
        panic!("expected a new resource");
    };
    resource
}

fn driver(storage: &PlatformStorage, kind: BindingKind) -> Box<dyn ResourceDriver + '_> {
    match kind {
        BindingKind::KvNamespace => Box::new(KvResourceDriver::new(storage, QUOTA)),
        BindingKind::D1Database => Box::new(D1ResourceDriver::new(storage, QUOTA)),
        _ => unreachable!(),
    }
}

fn live_path(storage: &PlatformStorage, resource: &ResourceRecord) -> PathBuf {
    let root = storage.data_dir().root();
    match resource.kind {
        BindingKind::KvNamespace => KvPaths::open(root)
            .unwrap()
            .database_path(resource.account_id, resource.id),
        BindingKind::D1Database => D1Paths::open(root)
            .unwrap()
            .database_path(resource.account_id, resource.id),
        _ => unreachable!(),
    }
}

fn prepare_creation(storage: &PlatformStorage, resource: &ResourceRecord, phase: usize) {
    if phase == 0 {
        return;
    }
    if phase == 3 {
        driver(storage, resource.kind).create(resource).unwrap();
    } else {
        match resource.kind {
            BindingKind::KvNamespace => {
                let key = KvPaths::storage_key(resource.account_id, resource.id);
                KvNamespaceRepository::new(storage.db())
                    .ensure_namespace(resource, &key, 1, QUOTA)
                    .unwrap();
                if phase == 2 {
                    let staging = KvPaths::open(storage.data_dir().root())
                        .unwrap()
                        .create_namespace_staging(resource.id)
                        .unwrap();
                    let path = staging.join("data.sqlite");
                    drop(
                        KvEngine::create(&path, resource.account_id, resource.id, 1, QUOTA)
                            .unwrap(),
                    );
                    write_marker(storage, resource, &path);
                }
            }
            BindingKind::D1Database => {
                let key = D1Paths::storage_key(resource.account_id, resource.id);
                D1DatabaseRepository::new(storage.db())
                    .ensure_database(resource, &key, 1, QUOTA)
                    .unwrap();
                if phase == 2 {
                    let staging = D1Paths::open(storage.data_dir().root())
                        .unwrap()
                        .create_database_staging(resource.id)
                        .unwrap();
                    let path = staging.join("data.sqlite");
                    drop(
                        D1Engine::create(&path, resource.account_id, resource.id, 1, QUOTA)
                            .unwrap(),
                    );
                    write_marker(storage, resource, &path);
                }
            }
            _ => unreachable!(),
        }
    }
    if phase == 3 {
        write_marker(storage, resource, &live_path(storage, resource));
    }
}

fn write_marker(storage: &PlatformStorage, resource: &ResourceRecord, path: &Path) {
    match resource.kind {
        BindingKind::KvNamespace => {
            let record = KvNamespaceRepository::new(storage.db())
                .get(resource.account_id, resource.id)
                .unwrap();
            let engine = KvEngine::from_record(path.to_path_buf(), &record).unwrap();
            engine
                .put("survivor", b"retained", &KvPutOptions::default(), 2)
                .unwrap();
            engine.checkpoint(true).unwrap();
        }
        BindingKind::D1Database => {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.execute_batch(
                "CREATE TABLE user_marker(value TEXT); INSERT INTO user_marker VALUES('retained');",
            )
            .unwrap();
        }
        _ => unreachable!(),
    }
}

#[test]
fn bootstrap_recovers_current_creation_and_deletion_boundaries() {
    let (_temp, config) = config();
    let (storage, scheduler) = bootstrap(&config).unwrap();
    let mut resources = Vec::new();
    let mut cancelled = Vec::new();
    for (kind_index, kind) in KINDS.into_iter().enumerate() {
        for phase in 0..4 {
            let resource = reserve(&storage, kind, &format!("create-{kind_index}-{phase}"), 1);
            prepare_creation(&storage, &resource, phase);
            resources.push((resource, phase));
        }
        let resource = reserve(&storage, kind, &format!("cancel-{kind_index}"), 1);
        ResourceRepository::new(storage.db())
            .begin_delete(resource.account_id, resource.id, 2)
            .unwrap();
        cancelled.push(resource);
    }
    drop(scheduler);
    drop(storage);

    let (storage, scheduler) = bootstrap(&config).unwrap();
    let schemas = inspect_current_schema(storage.data_dir(), storage.db(), 5_000).unwrap();
    assert_eq!((schemas.kv_files, schemas.d1_files), (4, 4));
    for resource in cancelled {
        assert_eq!(
            ResourceRepository::new(storage.db())
                .get(resource.account_id, resource.id)
                .unwrap()
                .state,
            ResourceState::Tombstoned
        );
        assert!(!live_path(&storage, &resource).exists());
    }
    for (resource, phase) in &resources {
        assert_eq!(
            ResourceRepository::new(storage.db())
                .get(resource.account_id, resource.id)
                .unwrap()
                .state,
            ResourceState::Ready
        );
        if *phase >= 2 {
            let path = live_path(&storage, resource);
            let value: String = match resource.kind {
                BindingKind::KvNamespace => {
                    let record = KvNamespaceRepository::new(storage.db())
                        .get(resource.account_id, resource.id)
                        .unwrap();
                    String::from_utf8(
                        KvEngine::from_record(path, &record)
                            .unwrap()
                            .get("survivor", 3)
                            .unwrap()
                            .unwrap()
                            .value,
                    )
                    .unwrap()
                }
                BindingKind::D1Database => rusqlite::Connection::open(path)
                    .unwrap()
                    .query_row("SELECT value FROM user_marker", [], |row| row.get(0))
                    .unwrap(),
                _ => unreachable!(),
            };
            assert_eq!(value, "retained");
        }
        ResourceRepository::new(storage.db())
            .begin_delete(resource.account_id, resource.id, 4)
            .unwrap();
        let deleting = ResourceRepository::new(storage.db())
            .get(resource.account_id, resource.id)
            .unwrap();
        if phase % 3 >= 1 {
            driver(&storage, resource.kind)
                .begin_delete(&deleting)
                .unwrap();
        }
        if phase % 3 == 2 {
            driver(&storage, resource.kind)
                .finalize_delete(&deleting)
                .unwrap();
        }
    }
    drop(scheduler);
    drop(storage);

    let (storage, _scheduler) = bootstrap(&config).unwrap();
    for (resource, _) in resources {
        assert_eq!(
            ResourceRepository::new(storage.db())
                .get(resource.account_id, resource.id)
                .unwrap()
                .state,
            ResourceState::Tombstoned
        );
        assert!(!live_path(&storage, &resource).exists());
    }
    assert!(
        ResourceRepository::new(storage.db())
            .reconcile_candidates()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn bootstrap_keeps_incomplete_restores_pending_without_creating_empty_databases() {
    let (_temp, config) = config();
    let (storage, scheduler) = bootstrap(&config).unwrap();
    let backup = uuid::Uuid::now_v7().to_string();
    let mut resources = Vec::new();
    for (index, kind) in KINDS.into_iter().enumerate() {
        let source = reserve(&storage, kind, &format!("source-{index}"), 1);
        driver(&storage, kind).create(&source).unwrap();
        ResourceRepository::new(storage.db())
            .mark_ready(source.id, 2)
            .unwrap();
        let bytes = std::fs::read(live_path(&storage, &source)).unwrap();
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let fingerprint = storage.crypto().fingerprint_request(backup.as_bytes());
        let resource = reserve(&storage, kind, &format!("restore-{index}"), 1);
        match kind {
            BindingKind::KvNamespace => {
                let catalog = KvNamespaceRepository::new(storage.db());
                catalog
                    .create_backup(source.id, &backup, 1, "backup", &fingerprint, 3)
                    .unwrap();
                catalog
                    .complete_backup(
                        &backup,
                        &format!("system/backups/kv/{}/{backup}/data.sqlite", source.id),
                        &digest,
                        bytes.len() as u64,
                        4,
                    )
                    .unwrap();
                KvNamespaceRepository::new(storage.db())
                    .ensure_restoring_namespace(
                        &resource,
                        &KvPaths::storage_key(resource.account_id, resource.id),
                        1,
                        QUOTA,
                        &backup,
                    )
                    .unwrap();
            }
            BindingKind::D1Database => {
                let catalog = D1DatabaseRepository::new(storage.db());
                catalog
                    .create_backup(source.id, &backup, 1, 0, "backup", &fingerprint, 3)
                    .unwrap();
                catalog
                    .complete_backup(
                        &backup,
                        &format!("system/backups/d1/{}/{backup}/data.sqlite", source.id),
                        &digest,
                        bytes.len() as u64,
                        4,
                    )
                    .unwrap();
                D1DatabaseRepository::new(storage.db())
                    .ensure_restoring_database(
                        &resource,
                        &D1Paths::storage_key(resource.account_id, resource.id),
                        1,
                        QUOTA,
                        &backup,
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        resources.push(resource);
    }
    drop(scheduler);
    drop(storage);
    let (storage, _scheduler) = bootstrap(&config).unwrap();
    for resource in resources {
        assert_eq!(
            ResourceRepository::new(storage.db())
                .get(resource.account_id, resource.id)
                .unwrap()
                .state,
            ResourceState::Creating
        );
        assert!(!live_path(&storage, &resource).exists());
        let retained_backup = match resource.kind {
            BindingKind::KvNamespace => {
                KvNamespaceRepository::new(storage.db())
                    .get(resource.account_id, resource.id)
                    .unwrap()
                    .restore_backup_id
            }
            BindingKind::D1Database => {
                D1DatabaseRepository::new(storage.db())
                    .get(resource.account_id, resource.id)
                    .unwrap()
                    .restore_backup_id
            }
            _ => unreachable!(),
        };
        assert_eq!(retained_backup.as_deref(), Some(backup.as_str()));
    }
}

#[test]
fn bootstrap_rejects_unknown_resource_versions_and_corrupt_scheduler_without_repair() {
    let (_temp, config) = config();
    let (storage, scheduler) = bootstrap(&config).unwrap();
    reserve(&storage, BindingKind::KvNamespace, "unknown-driver", 2);
    drop(scheduler);
    drop(storage);
    assert_eq!(
        bootstrap(&config).unwrap_err().code(),
        ErrorCode::SchemaUnsupported
    );

    let (_other, other_config) = self::config();
    let (storage, scheduler) = bootstrap(&other_config).unwrap();
    let path = storage.data_dir().scheduler_db_path();
    drop(scheduler);
    drop(storage);
    std::fs::write(&path, b"not a SQLite database").unwrap();
    assert!(bootstrap(&other_config).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"not a SQLite database");
}
