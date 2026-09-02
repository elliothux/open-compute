use super::*;
use crate::{
    NewDeployment, NewDeploymentBinding, ReserveResourceCreate, ResourceCreateReservation,
    WorkerRepository,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, RequestId, SystemClock,
};
use std::collections::BTreeMap;

fn storage() -> (tempfile::TempDir, PlatformStorage) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    };
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    (temp, storage)
}

fn public_id(namespace: ResourceId, fill: u8) -> DurableObjectId {
    let mut bytes = [fill; 32];
    bytes[..8].copy_from_slice(&durable_object_namespace_prefix(namespace));
    DurableObjectId::for_namespace(bytes, namespace).unwrap()
}

fn reserve_resource(
    storage: &PlatformStorage,
    account: AccountId,
    kind: BindingKind,
    name: &str,
    now_ms: i64,
) -> ResourceRecord {
    match ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind,
                name,
                idempotency_key: name,
                fingerprint_key_id: "key",
                request_fingerprint: &[7; 32],
                resource_id: ResourceId::generate(),
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms,
                expires_at_ms: now_ms + 1000,
            },
            1_000_000,
        )
        .unwrap()
    {
        ResourceCreateReservation::Reserved(value) => value,
        other => panic!("unexpected {other:?}"),
    }
}

struct Fixture {
    account: AccountId,
    worker: WorkerId,
    deployment: DeploymentId,
    namespace: ResourceId,
    binding: BindingId,
    descriptor: [u8; 32],
    route_generation: u64,
}

fn ready_fixture(storage: &PlatformStorage) -> Fixture {
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "durable", RequestId::generate(), 10, 1_000_000)
        .unwrap();
    let resources = ResourceRepository::new(storage.db());
    let namespace = ResourceId::generate();
    let fingerprint = [3; 32];
    let resource = match resources
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::DoNamespace,
                name: "COUNTERS",
                idempotency_key: "create-do",
                fingerprint_key_id: "key",
                request_fingerprint: &fingerprint,
                resource_id: namespace,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 11,
                expires_at_ms: 1000,
            },
            1_000_000,
        )
        .unwrap()
    {
        ResourceCreateReservation::Reserved(value) => value,
        other => panic!("unexpected {other:?}"),
    };
    DurableObjectRepository::new(storage)
        .ensure_namespace(&resource, worker.id, "Counter")
        .unwrap();
    resources.mark_ready(namespace, 12).unwrap();

    let deployment = DeploymentId::generate();
    let binding = BindingId::generate();
    let descriptor_value = open_compute_workers_forbidden_descriptor(binding, namespace);
    let descriptor = descriptor_value.0;
    workers
        .insert_staging_deployment(
            &NewDeployment {
                id: deployment,
                account_id: account,
                worker_id: worker.id,
                content_kind: crate::DeploymentContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [9; 32],
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 13,
            },
            &crate::NewDeploymentProducts {
                bindings: &[NewDeploymentBinding {
                    id: binding,
                    name: "COUNTERS".to_owned(),
                    kind: BindingKind::DoNamespace,
                    resource_id: namespace,
                    resource_spec_generation: 1,
                    capability_version: 1,
                    permissions_json: serde_json::to_vec(&CanonicalPermissions::default()).unwrap(),
                    config_json: serde_json::to_vec(&CanonicalBindingConfig::default()).unwrap(),
                    descriptor_sha256: descriptor,
                }],
                ..Default::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 14).unwrap();
    let promoted = workers
        .promote(
            account,
            worker.id,
            deployment,
            None,
            RequestId::generate(),
            15,
        )
        .unwrap();
    Fixture {
        account,
        worker: worker.id,
        deployment,
        namespace,
        binding,
        descriptor,
        route_generation: promoted.route_generation,
    }
}

// Keep this storage-crate test independent from the workers crate dependency boundary.
fn open_compute_workers_forbidden_descriptor(
    binding: BindingId,
    namespace: ResourceId,
) -> ([u8; 32], Vec<u8>) {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": 1,
        "bindingId": binding,
        "name": "COUNTERS",
        "kind": "do_namespace",
        "resourceId": namespace,
        "resourceSpecGeneration": 1,
        "capabilityVersion": 1,
        "permissions": {"read": true, "write": true},
        "config": {},
    }))
    .unwrap();
    (Sha256::digest(&bytes).into(), bytes)
}

#[test]
fn namespace_identity_dispatch_and_generation_fence_are_durable() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let namespace = repo
        .get_namespace(fixture.account, fixture.namespace)
        .unwrap();
    assert_eq!(namespace.owner_worker_id, fixture.worker);
    assert_eq!(namespace.class_name, "Counter");
    assert_eq!(namespace.namespace_storage_key.len(), 64);
    let (prefix, name_key) = repo.facade_identity(fixture.namespace).unwrap();
    assert_eq!(prefix, durable_object_namespace_prefix(fixture.namespace));
    assert_ne!(name_key, [0; 32]);

    let object = public_id(fixture.namespace, 7);
    let first = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            20,
            true,
        )
        .unwrap();
    assert_eq!(first.object_generation, 1);
    assert_eq!(
        repo.list_objects(fixture.account, fixture.namespace)
            .unwrap()[0]
            .state,
        DurableObjectState::Creating
    );
    assert_eq!(first.class_name, "Counter");
    assert_eq!(first.worker_id, fixture.worker);
    assert_eq!(first.worker_code_sha256, "09".repeat(32));
    assert!(!first.host_key.contains(&object.to_string()));
    let repeated = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            21,
            true,
        )
        .unwrap();
    assert_eq!(repeated.host_key, first.host_key);
    let existing_at_stop_watermark = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            21,
            false,
        )
        .unwrap();
    assert_eq!(existing_at_stop_watermark.object_generation, 1);
    let blocked_new = public_id(fixture.namespace, 9);
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            blocked_new,
            21,
            false,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoStorageLimit
    );
    repo.finish_object_create(fixture.namespace, object, first.object_generation, 21)
        .unwrap();

    let deleting = repo
        .begin_object_delete(fixture.account, fixture.namespace, object, 22)
        .unwrap();
    assert_eq!(deleting.state, DurableObjectState::Deleting);
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            23,
            true,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoObjectDeleting
    );
    repo.finish_object_delete(fixture.namespace, object, 1, 24)
        .unwrap();
    let recreated = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            25,
            true,
        )
        .unwrap();
    assert_eq!(recreated.object_generation, 2);
    assert_ne!(recreated.host_key, first.host_key);
}

#[test]
fn authority_rejects_cross_namespace_stale_generation_and_live_namespace_delete() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let wrong = public_id(ResourceId::generate(), 4);
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            wrong,
            20,
            true,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoIdInvalid
    );
    let object = public_id(fixture.namespace, 5);
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation - 1,
            object,
            21,
            true,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoDeploymentStale
    );
    repo.authorize_dispatch(
        fixture.binding,
        fixture.deployment,
        &fixture.descriptor,
        fixture.route_generation,
        object,
        22,
        true,
    )
    .unwrap();
    assert!(repo.has_live_objects(fixture.namespace).unwrap());
    assert_eq!(
        repo.list_objects(fixture.account, fixture.namespace)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn fenced_delete_authority_survives_worker_tombstone() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let object = public_id(fixture.namespace, 8);
    let dispatch = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            20,
            true,
        )
        .unwrap();
    repo.finish_object_create(fixture.namespace, object, dispatch.object_generation, 21)
        .unwrap();
    let fenced = repo
        .begin_object_delete(fixture.account, fixture.namespace, object, 22)
        .unwrap();
    WorkerRepository::new(storage.db())
        .delete_worker(
            fixture.account,
            fixture.worker,
            &[fixture.deployment],
            RequestId::generate(),
            23,
        )
        .unwrap();

    let authority = repo
        .deletion_authority(
            fixture.account,
            fixture.namespace,
            object,
            fenced.generation,
        )
        .unwrap();
    assert_eq!(authority.object_id, object);
    assert_eq!(authority.object_generation, fenced.generation);
    assert!(!authority.host_key.contains(&object.to_string()));
    assert_eq!(
        repo.authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            24,
            true,
        )
        .unwrap_err()
        .code(),
        ErrorCode::BindingTypeMismatch
    );
}

#[test]
fn namespace_and_object_failure_boundaries_are_idempotent() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);

    for invalid in ["", "1Counter", "Counter-Bad", &"C".repeat(129)] {
        assert_eq!(
            repo.ensure_namespace(
                &ResourceRepository::new(storage.db())
                    .get(fixture.account, fixture.namespace)
                    .unwrap(),
                fixture.worker,
                invalid,
            )
            .unwrap_err()
            .code(),
            ErrorCode::DoClassNotFound
        );
    }
    let ready_resource = ResourceRepository::new(storage.db())
        .get(fixture.account, fixture.namespace)
        .unwrap();
    assert_eq!(
        repo.ensure_namespace(&ready_resource, fixture.worker, "Counter")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        repo.get_namespace_by_resource(ResourceId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::DoNamespaceNotFound
    );

    let missing = public_id(fixture.namespace, 11);
    assert_eq!(
        repo.begin_object_delete(fixture.account, fixture.namespace, missing, 20)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
    assert_eq!(
        repo.begin_object_delete(
            fixture.account,
            fixture.namespace,
            public_id(ResourceId::generate(), 12),
            20,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoIdInvalid
    );

    let object = public_id(fixture.namespace, 13);
    let dispatch = repo
        .authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            21,
            true,
        )
        .unwrap();
    repo.finish_object_create(fixture.namespace, object, dispatch.object_generation, 22)
        .unwrap();
    repo.finish_object_create(fixture.namespace, object, dispatch.object_generation, 23)
        .unwrap();
    assert_eq!(
        repo.deletion_authority(
            fixture.account,
            fixture.namespace,
            object,
            dispatch.object_generation,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoObjectDeleting
    );
    assert_eq!(
        repo.finish_object_delete(fixture.namespace, object, dispatch.object_generation, 24)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let deleting = repo
        .begin_object_delete(fixture.account, fixture.namespace, object, 25)
        .unwrap();
    assert_eq!(
        repo.begin_object_delete(fixture.account, fixture.namespace, object, 26)
            .unwrap(),
        deleting
    );
    assert_eq!(
        repo.finish_object_create(fixture.namespace, object, dispatch.object_generation, 27)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let tombstone = repo
        .finish_object_delete(fixture.namespace, object, dispatch.object_generation, 28)
        .unwrap();
    assert_eq!(
        repo.finish_object_delete(fixture.namespace, object, dispatch.object_generation, 29)
            .unwrap(),
        tombstone
    );
}

#[test]
fn namespace_owner_kind_and_existing_product_fail_closed() {
    let (_temp, storage) = storage();
    let account = storage.identity().default_account_id;
    let repo = DurableObjectRepository::new(&storage);
    let missing_owner = reserve_resource(
        &storage,
        account,
        BindingKind::DoNamespace,
        "MISSING_OWNER",
        40,
    );
    assert_eq!(
        repo.ensure_namespace(&missing_owner, WorkerId::generate(), "Counter")
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNotFound
    );

    let workers = WorkerRepository::new(storage.db());
    let deleted_worker = workers
        .create_worker(
            account,
            "deleted-owner",
            RequestId::generate(),
            41,
            1_000_000,
        )
        .unwrap()
        .0;
    workers
        .delete_worker(account, deleted_worker.id, &[], RequestId::generate(), 42)
        .unwrap();
    let unavailable_owner = reserve_resource(
        &storage,
        account,
        BindingKind::DoNamespace,
        "UNAVAILABLE_OWNER",
        43,
    );
    assert_eq!(
        repo.ensure_namespace(&unavailable_owner, deleted_worker.id, "Counter")
            .unwrap_err()
            .code(),
        ErrorCode::WorkerNotFound
    );

    let worker = workers
        .create_worker(account, "live-owner", RequestId::generate(), 44, 1_000_000)
        .unwrap()
        .0;
    let existing = reserve_resource(&storage, account, BindingKind::DoNamespace, "EXISTING", 45);
    repo.ensure_namespace(&existing, worker.id, "Counter")
        .unwrap();
    assert_eq!(
        repo.ensure_namespace(&existing, worker.id, "DifferentCounter")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );

    let wrong_kind = reserve_resource(
        &storage,
        account,
        BindingKind::KvNamespace,
        "WRONG_KIND",
        46,
    );
    assert_eq!(
        repo.get_namespace(account, wrong_kind.id)
            .unwrap_err()
            .code(),
        ErrorCode::DoNamespaceNotFound
    );

    ResourceRepository::new(storage.db())
        .mark_ready(wrong_kind.id, 47)
        .unwrap();
    let deployment = DeploymentId::generate();
    let binding = BindingId::generate();
    let descriptor = [8; 32];
    workers
        .insert_staging_deployment(
            &NewDeployment {
                id: deployment,
                account_id: account,
                worker_id: worker.id,
                content_kind: crate::DeploymentContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [9; 32],
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 48,
            },
            &crate::NewDeploymentProducts {
                bindings: &[NewDeploymentBinding {
                    id: binding,
                    name: "WRONG_KIND".to_owned(),
                    kind: BindingKind::KvNamespace,
                    resource_id: wrong_kind.id,
                    resource_spec_generation: 1,
                    capability_version: 1,
                    permissions_json: serde_json::to_vec(&CanonicalPermissions::default()).unwrap(),
                    config_json: serde_json::to_vec(&CanonicalBindingConfig::default()).unwrap(),
                    descriptor_sha256: descriptor,
                }],
                ..Default::default()
            },
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 49).unwrap();
    let promoted = workers
        .promote(
            account,
            worker.id,
            deployment,
            None,
            RequestId::generate(),
            50,
        )
        .unwrap();
    assert_eq!(
        repo.authorize_dispatch(
            binding,
            deployment,
            &descriptor,
            promoted.route_generation,
            public_id(wrong_kind.id, 9),
            51,
            true,
        )
        .unwrap_err()
        .code(),
        ErrorCode::DoNamespaceNotFound
    );
}

#[test]
fn object_list_page_is_bounded_and_cursor_is_stable() {
    let (_temp, storage) = storage();
    let fixture = ready_fixture(&storage);
    let repo = DurableObjectRepository::new(&storage);
    let ids = [7u8, 8, 9, 10];
    for fill in ids {
        let object = public_id(fixture.namespace, fill);
        repo.authorize_dispatch(
            fixture.binding,
            fixture.deployment,
            &fixture.descriptor,
            fixture.route_generation,
            object,
            i64::from(fill),
            true,
        )
        .unwrap();
        repo.finish_object_create(fixture.namespace, object, 1, i64::from(fill) + 100)
            .unwrap();
    }

    let first = repo
        .list_objects_page(fixture.account, fixture.namespace, None, 2)
        .unwrap();
    assert_eq!(first.objects.len(), 2);
    assert!(first.next_cursor.is_some());
    let cursor = decode_object_list_cursor(first.next_cursor.as_ref().unwrap()).unwrap();
    assert_eq!(
        cursor,
        (first.objects[1].object_id, first.objects[1].generation)
    );

    let second = repo
        .list_objects_page(fixture.account, fixture.namespace, Some(cursor), 2)
        .unwrap();
    assert_eq!(second.objects.len(), 2);
    assert!(second.next_cursor.is_none());

    let latest = repo
        .get_latest_object(
            fixture.account,
            fixture.namespace,
            public_id(fixture.namespace, 10),
        )
        .unwrap();
    assert_eq!(latest.state, DurableObjectState::Ready);

    assert_eq!(
        decode_object_list_cursor("not-a-cursor")
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        decode_object_list_cursor(&format!("{}:0", public_id(fixture.namespace, 7)))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
}
