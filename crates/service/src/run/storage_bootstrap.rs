//! Exclusive local storage bootstrap and current resource recovery before serving.

use open_compute_core::{PlatformConfig, PlatformError, RequestId, SystemClock};
use open_compute_storage::PlatformStorage;
use open_compute_storage::public_gateway::PublicGatewayRepository;
use open_compute_storage::scheduler::SchedulerStore;
use open_compute_storage::worker_repository::WorkerRepository;
use open_compute_workers::{
    AiSearchInstanceResourceDriver, AiSearchNamespaceResourceDriver, D1ResourceDriver,
    KvResourceDriver, ResourceController, ResourcePins, VectorizeResourceDriver,
};
use std::sync::Arc;

pub(super) fn bootstrap(
    config: &PlatformConfig,
) -> Result<(Arc<PlatformStorage>, Arc<SchedulerStore>), PlatformError> {
    let storage = Arc::new(PlatformStorage::bootstrap_with_hardening(
        &config.data,
        &config.hardening,
        &SystemClock,
    )?);
    let (_, total_bytes) = storage.filesystem_space()?;
    if config.durable_objects.disk_degraded_free_bytes > total_bytes {
        return Err(PlatformError::new(
            open_compute_core::ErrorCode::LimitInvalid,
            "[durable_objects] disk_degraded_free_bytes exceeds the data filesystem capacity in bytes",
        ));
    }
    let now = open_compute_core::wall_time_ms();
    let scheduler = Arc::new(SchedulerStore::open(
        &storage.data_dir().ensure_scheduler_db()?,
        config.data.sqlite_busy_timeout_ms,
        now,
        storage.identity().instance_id,
    )?);
    open_compute_storage::vectorize::VectorizePaths::open(storage.data_dir().root())?;
    open_compute_storage::ai_search::AiSearchPaths::open(storage.data_dir().root())?;
    open_compute_storage::schema_inspection::inspect_current_schema(
        storage.data_dir(),
        storage.db(),
        config.data.sqlite_busy_timeout_ms,
    )?;
    let gateway = PublicGatewayRepository::new(storage.db());
    if let Some(config) = &config.public_gateway {
        gateway.provision(&config.base_domain, now)?;
    } else {
        gateway.disable(now)?;
    }

    open_compute_storage::kv::KvPaths::open(storage.data_dir().root())?.cleanup_write_staging()?;
    open_compute_storage::r2_staging::R2Staging::open(storage.data_dir().root())?.cleanup()?;
    let workers = WorkerRepository::new(storage.db());
    workers.prune_expired_idempotency(now, config.workers.delete_recovery_batch)?;
    workers.recover_deleting_versions(
        RequestId::generate(),
        now,
        config.workers.delete_recovery_batch,
    )?;
    // No request can hold a resource pin while the exclusive startup owner recovers it.
    let pins = ResourcePins::new();
    ResourceController::new(
        &storage,
        pins.clone(),
        KvResourceDriver::new(&storage, config.kv.namespace_quota_bytes),
    )
    .reconcile_pending(RequestId::generate(), now)?;
    ResourceController::new(
        &storage,
        pins.clone(),
        D1ResourceDriver::new(&storage, config.d1.database_quota_bytes),
    )
    .reconcile_pending(RequestId::generate(), now)?;
    ResourceController::new(
        &storage,
        pins.clone(),
        VectorizeResourceDriver::recovery(&storage, config.data.sqlite_busy_timeout_ms),
    )
    .reconcile_pending(RequestId::generate(), now)?;
    ResourceController::new(
        &storage,
        pins.clone(),
        AiSearchNamespaceResourceDriver::new(&storage),
    )
    .reconcile_pending(RequestId::generate(), now)?;
    ResourceController::new(
        &storage,
        pins.clone(),
        AiSearchInstanceResourceDriver::recovery(&storage, config.data.sqlite_busy_timeout_ms),
    )
    .reconcile_pending(RequestId::generate(), now)?;
    crate::vectorize_coordinator::VectorizeCoordinator::new(storage.clone(), pins).drain_once()?;
    Ok((storage, scheduler))
}

#[cfg(test)]
mod tests;
