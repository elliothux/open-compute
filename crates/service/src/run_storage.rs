//! Exclusive local storage bootstrap and current resource recovery before serving.

use open_compute_core::{PlatformConfig, PlatformError, RequestId, SystemClock};
use open_compute_storage::{PlatformStorage, SchedulerStore, WorkerRepository};
use open_compute_workers::{D1ResourceDriver, KvResourceDriver, ResourceController, ResourcePins};
use std::sync::Arc;

pub(super) fn bootstrap(
    config: &PlatformConfig,
) -> Result<(Arc<PlatformStorage>, Arc<SchedulerStore>), PlatformError> {
    let storage = Arc::new(PlatformStorage::bootstrap_with_hardening(
        &config.storage,
        &config.hardening,
        &SystemClock,
    )?);
    let now = super::unix_ms();
    let scheduler = Arc::new(SchedulerStore::open(
        &storage.data_dir().ensure_scheduler_db()?,
        config.storage.sqlite_busy_timeout_ms,
        now,
    )?);
    open_compute_storage::inspect_current_schema(
        storage.data_dir(),
        storage.db(),
        config.storage.sqlite_busy_timeout_ms,
    )?;

    open_compute_storage::KvPaths::open(storage.data_dir().root())?.cleanup_write_staging()?;
    open_compute_storage::R2Staging::open(storage.data_dir().root())?.cleanup()?;
    let workers = WorkerRepository::new(storage.db());
    workers.prune_expired_idempotency(now, config.workers.delete_recovery_batch)?;
    workers.recover_deleting_deployments(
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
        pins,
        D1ResourceDriver::new(&storage, config.d1.database_quota_bytes),
    )
    .reconcile_pending(RequestId::generate(), now)?;
    Ok((storage, scheduler))
}

#[cfg(test)]
#[path = "run_storage_tests.rs"]
mod tests;
