use super::*;
use open_compute_core::{
    BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DurableObjectId,
    RequestId, ResourceId, VersionId, durable_object_namespace_prefix,
};
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, NewVersion, NewVersionBinding, NewVersionProducts,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository, VersionContentKind,
    WorkerRepository,
};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct DeleteTransport {
    calls: AtomicUsize,
}

impl DurableObjectDeleteTransport for DeleteTransport {
    fn delete<'a>(
        &'a self,
        _authority: &'a AuthorizedDurableObjectDelete,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(()) })
    }
}

fn object_id(namespace: ResourceId) -> DurableObjectId {
    let mut bytes = [7; 32];
    bytes[..8].copy_from_slice(&durable_object_namespace_prefix(namespace));
    DurableObjectId::for_namespace(bytes, namespace).unwrap()
}

#[tokio::test]
async fn reconciliation_finishes_creating_and_deleting_object_generations() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(account, "lifecycle-worker", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let namespace = ResourceId::generate();
    let ResourceCreateReservation::Reserved(resource) = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id: account,
                kind: BindingKind::DoNamespace,
                name: "LIFECYCLE",
                idempotency_key: "lifecycle",
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &[3; 32],
                resource_id: namespace,
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: 2,
                expires_at_ms: i64::MAX,
            },
            100,
        )
        .unwrap()
    else {
        panic!("resource reservation");
    };
    let durable = DurableObjectRepository::new(&storage);
    durable
        .ensure_namespace(&resource, worker.id, "Counter")
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(namespace, 3)
        .unwrap();
    let version = VersionId::generate();
    let binding = BindingId::generate();
    let descriptor_bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,
        "bindingId":binding,
        "name":"COUNTERS",
        "kind":"do_namespace",
        "resourceId":namespace,
        "resourceSpecGeneration":1,
        "capabilityVersion":1,
        "permissions":{"read":true,"write":true},
        "config":{}
    }))
    .unwrap();
    let descriptor: [u8; 32] = Sha256::digest(descriptor_bytes).into();
    workers
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".to_owned(),
                compatibility_flags: Vec::new(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 4,
            },
            &NewVersionProducts {
                bindings: &[NewVersionBinding {
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
            100,
        )
        .unwrap();
    workers.begin_validation(version).unwrap();
    workers.mark_ready(version, 5).unwrap();
    let active = workers
        .promote(account, worker.id, version, None, RequestId::generate(), 6)
        .unwrap();
    let object = object_id(namespace);
    let dispatch = durable
        .authorize_dispatch(
            binding,
            version,
            &descriptor,
            active.route_generation,
            object,
            7,
            true,
        )
        .unwrap();
    let transport = Arc::new(DeleteTransport::default());
    let service = DurableObjectLifecycleService {
        storage: storage.clone(),
        transport: transport.clone(),
        config: DurableObjectsConfig {
            reconcile_batch: 100,
            ..DurableObjectsConfig::default()
        },
        metrics: Some(state.metrics().clone()),
        scheduler: None,
    };
    assert!(format!("{service:?}").contains("DurableObjectLifecycleService"));
    assert_eq!(service.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        durable
            .list_objects(account, namespace)
            .unwrap()
            .last()
            .unwrap()
            .state,
        DurableObjectState::Ready
    );
    durable
        .begin_object_delete(account, namespace, object, 8)
        .unwrap();
    assert_eq!(service.reconcile_pending().await.unwrap(), 1);
    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        durable
            .list_objects(account, namespace)
            .unwrap()
            .last()
            .unwrap()
            .state,
        DurableObjectState::Tombstoned
    );
    assert_eq!(dispatch.object_generation, 1);
}
