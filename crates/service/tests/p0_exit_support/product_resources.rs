//! Direct product setup through the owning lifecycle authorities.

use open_compute_artifacts::R2ObjectStore;
use open_compute_core::{AccountId, BindingKind, RequestId, ResourceId, ResourceState};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, KV_SCHEMA_VERSION, PlatformStorage, R2_SCHEMA_VERSION,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, CreateResourceResult, D1ResourceDriver,
    KvResourceDriver, R2ResourceDriver, ResourceController, ResourceDriver, ResourcePins,
};

use super::{d1_config, kv_config, r2_config};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_product_resource(
    storage: &PlatformStorage,
    objects: &R2ObjectStore,
    pins: &ResourcePins,
    account_id: AccountId,
    kind: BindingKind,
    name: &str,
    idempotency_key: &str,
    now_ms: i64,
) -> ResourceId {
    match kind {
        BindingKind::KvNamespace => create_local_resource(
            storage,
            pins,
            KvResourceDriver::new(storage, kv_config().namespace_quota_bytes),
            account_id,
            kind,
            name,
            idempotency_key,
            KV_SCHEMA_VERSION,
            now_ms,
        ),
        BindingKind::D1Database => create_local_resource(
            storage,
            pins,
            D1ResourceDriver::new(storage, d1_config().database_quota_bytes),
            account_id,
            kind,
            name,
            idempotency_key,
            D1_DATABASE_SCHEMA_VERSION,
            now_ms,
        ),
        BindingKind::R2Bucket => {
            create_r2_resource(storage, objects, account_id, name, idempotency_key, now_ms).await
        }
        _ => panic!("unsupported product resource kind: {kind:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn create_local_resource<D: ResourceDriver>(
    storage: &PlatformStorage,
    pins: &ResourcePins,
    driver: D,
    account_id: AccountId,
    kind: BindingKind,
    name: &str,
    idempotency_key: &str,
    driver_schema_version: u32,
    now_ms: i64,
) -> ResourceId {
    match ResourceController::new(storage, pins.clone(), driver)
        .create(&CreateResourceRequest {
            account_id,
            kind,
            name: name.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
            driver_schema_version,
            request_id: RequestId::generate(),
            now_ms,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(result) => result.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("first product resource create replayed"),
    }
}

async fn create_r2_resource(
    storage: &PlatformStorage,
    objects: &R2ObjectStore,
    account_id: AccountId,
    name: &str,
    idempotency_key: &str,
    now_ms: i64,
) -> ResourceId {
    let fingerprint = storage.crypto().fingerprint_request(
        format!("r2:{name}:{}", hex::encode(objects.authority_sha256())).as_bytes(),
    );
    let resource_id = ResourceId::generate();
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id,
                kind: BindingKind::R2Bucket,
                name,
                idempotency_key,
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: R2_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms,
                expires_at_ms: now_ms.saturating_add(24 * 60 * 60 * 1_000),
            },
            storage.hardening().max_resources_per_kind_per_account,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        panic!("first product resource create must reserve a new identity")
    };
    R2ResourceDriver::new(storage, objects.clone(), r2_config())
        .create(&resource)
        .await
        .unwrap();
    ResourceRepository::new(storage.db())
        .mark_ready(resource.id, now_ms.saturating_add(1))
        .unwrap();
    let response = serde_json::to_vec(&CreateResourceResult {
        resource_id: resource.id,
        state: ResourceState::Ready,
    })
    .unwrap();
    ResourceRepository::new(storage.db())
        .complete_create(
            account_id,
            idempotency_key,
            &fingerprint,
            resource.id,
            &response,
        )
        .unwrap();
    resource.id
}
