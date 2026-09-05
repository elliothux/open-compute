use super::*;
use crate::ResourceDriver;
use open_compute_core::config::DataConfig;
use open_compute_core::{RequestId, ResourceId, SystemClock, durable_object_namespace_prefix};
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, PlatformStorage, ReserveResourceCreate, ResourceCreateReservation,
    ResourceRepository, WorkerRepository,
};

fn fixture() -> (tempfile::TempDir, PlatformStorage, ResourceRecord, WorkerId) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &DataConfig {
            path: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 1,
        },
        &SystemClock,
    )
    .unwrap();
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "driver", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0
        .id;
    let resource_id = ResourceId::generate();
    let resource = match ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::DoNamespace,
                name: "COUNTER",
                idempotency_key: "create-counter",
                fingerprint_key_id: "key",
                request_fingerprint: &[1; 32],
                resource_id,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 2,
                expires_at_ms: 10_000,
            },
            1_000_000,
        )
        .unwrap()
    {
        ResourceCreateReservation::Reserved(value) => value,
        other => panic!("unexpected {other:?}"),
    };
    (temp, storage, resource, worker)
}

#[test]
fn driver_covers_reconcile_health_and_live_delete_fence() {
    let (_temp, storage, creating, worker) = fixture();
    let driver = DurableObjectResourceDriver::new(&storage, worker, "Counter");
    assert_eq!(driver.kind(), BindingKind::DoNamespace);
    assert_eq!(
        driver.create_fingerprint_material().len(),
        16 + "Counter".len()
    );
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Absent
    );
    driver.create(&creating).unwrap();
    assert_eq!(
        driver.reconcile(&creating).unwrap(),
        ReconcileOutcome::Ready
    );
    ResourceRepository::new(storage.db())
        .mark_ready(creating.id, 3)
        .unwrap();
    let ready = ResourceRepository::new(storage.db())
        .get(creating.account_id, creating.id)
        .unwrap();
    assert_eq!(driver.reconcile(&ready).unwrap(), ReconcileOutcome::Ready);
    assert_eq!(
        driver.health(&ready).unwrap().availability,
        ResourceAvailability::Healthy
    );
    assert_eq!(
        DurableObjectResourceDriver::new(&storage, worker, "Other")
            .health(&ready)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );

    let mut deleting = ready.clone();
    deleting.state = ResourceState::Deleting;
    assert_eq!(
        driver.reconcile(&deleting).unwrap(),
        ReconcileOutcome::Deleted
    );
    let mut tombstoned = ready.clone();
    tombstoned.state = ResourceState::Tombstoned;
    assert_eq!(
        driver.reconcile(&tombstoned).unwrap(),
        ReconcileOutcome::Deleted
    );

    let mut object = [9; 32];
    object[..8].copy_from_slice(&durable_object_namespace_prefix(ready.id));
    let connection =
        rusqlite::Connection::open(storage.data_dir().root().join("control.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO do_objects(namespace_resource_id, object_id, generation, state, \
             created_at_ms, updated_at_ms, deleted_at_ms) VALUES (?1, ?2, 1, 'ready', 4, 4, NULL)",
            rusqlite::params![ready.id.to_string(), hex::encode(object)],
        )
        .unwrap();
    assert_eq!(
        driver.begin_delete(&ready).unwrap_err().code(),
        ErrorCode::DoNamespaceNotEmpty
    );
    assert_eq!(
        driver.finalize_delete(&ready).unwrap_err().code(),
        ErrorCode::DoNamespaceNotEmpty
    );
}
