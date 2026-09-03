//! Single serialized D1 execution and completed-snapshot authority.

use crate::metrics::MetricsRegistry;
use md5::Md5;
use open_compute_core::{
    AccountId, D1Config, ErrorCode, PlatformError, ResourceAvailability, ResourceId,
};
use open_compute_storage::{
    D1DatabaseRecord, D1DatabaseRepository, D1Engine, D1Paths, D1QueryLimits, D1SnapshotRecord,
    D1SnapshotRepository, D1TransferState, PlatformStorage, ResourceRepository,
};
use open_compute_workers::{ResourcePin, ResourcePins};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const D1_COMPLETED_HISTORY_POINTS: u32 = 8;
const D1_TRANSFER_FILES: u32 = 8;

/// Context for one operation while its resource pin and database lane are held.
pub(crate) struct D1OperationContext<'a> {
    /// Verified live database engine.
    pub(crate) engine: &'a D1Engine,
    /// Platform storage for admission and session signing.
    pub(crate) storage: &'a PlatformStorage,
    /// Validated D1 limits.
    pub(crate) config: &'a D1Config,
    paths: &'a D1Paths,
    catalog: &'a D1DatabaseRecord,
    mutation_started: &'a AtomicBool,
    completed_history_required: &'a AtomicBool,
}

impl D1OperationContext<'_> {
    /// Mark that the operation is about to attempt a database mutation.
    pub(crate) fn mark_mutation(&self) {
        self.mutation_started.store(true, Ordering::Release);
    }

    /// Persist the current database state as an explicit management checkpoint.
    pub(crate) fn checkpoint_completed_history(&self) -> Result<D1SnapshotRecord, PlatformError> {
        self.checkpoint_completed_history_preserving(None)
    }

    /// Persist the current state while retaining one selected restore source.
    pub(crate) fn checkpoint_completed_history_preserving(
        &self,
        protected_session_version: Option<u64>,
    ) -> Result<D1SnapshotRecord, PlatformError> {
        prune_expired_transfer_history(self.storage, self.paths, self.catalog)?;
        let version = self.engine.session_version()?;
        let repository = D1SnapshotRepository::new(self.storage.db());
        let snapshot = match repository
            .latest_snapshot(self.catalog.resource.account_id, self.catalog.resource.id)?
        {
            Some(latest) if latest.session_version == version => {
                validate_snapshot(self.paths, self.catalog, &latest)?
            }
            Some(latest) if latest.session_version > version => return Err(invariant()),
            _ => {
                repository.ensure_completed_snapshot_capacity(
                    self.catalog.resource.account_id,
                    self.catalog.resource.id,
                    D1_COMPLETED_HISTORY_POINTS,
                    [protected_session_version, None],
                )?;
                crate::d1_backend::ensure_d1_storage_headroom(self.storage)?;
                self.engine.checkpoint(true)?;
                persist_snapshot(self.storage, self.paths, self.catalog, self.engine, version)?
            }
        };
        prune_snapshot_history(
            self.storage,
            self.paths,
            self.catalog,
            [protected_session_version, None],
        )?;
        Ok(snapshot)
    }

    /// Verify that one mutation result checkpoint can fit before mutating.
    pub(crate) fn ensure_completed_history_capacity(
        &self,
        protected_session_versions: [Option<u64>; 2],
    ) -> Result<(), PlatformError> {
        prune_expired_transfer_history(self.storage, self.paths, self.catalog)?;
        D1SnapshotRepository::new(self.storage.db()).ensure_completed_snapshot_capacity(
            self.catalog.resource.account_id,
            self.catalog.resource.id,
            D1_COMPLETED_HISTORY_POINTS,
            protected_session_versions,
        )
    }

    /// Expire an abandoned non-ingesting transfer and collect expired terminal evidence.
    pub(crate) fn reclaim_expired_transfers(&self) -> Result<(), PlatformError> {
        prune_expired_transfer_history(self.storage, self.paths, self.catalog)
    }

    /// Bound unexpired terminal transfer files before producing one more body.
    pub(crate) fn ensure_transfer_file_capacity(&self) -> Result<(), PlatformError> {
        prune_expired_transfer_history(self.storage, self.paths, self.catalog)?;
        D1SnapshotRepository::new(self.storage.db()).ensure_transfer_file_capacity(
            self.catalog.resource.account_id,
            self.catalog.resource.id,
            D1_TRANSFER_FILES,
            checked_wall_now_ms()?,
        )
    }

    /// Require a completed management checkpoint after this operation mutates the database.
    pub(crate) fn require_completed_history(&self) {
        self.completed_history_required
            .store(true, Ordering::Release);
    }
}

/// One owner for per-database lanes, execution, and completed history.
#[derive(Clone)]
pub(crate) struct D1Coordinator {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    config: D1Config,
    handles: D1HandleManager,
    metrics: Arc<Mutex<Option<Arc<MetricsRegistry>>>>,
}

impl D1Coordinator {
    pub(crate) fn new(storage: Arc<PlatformStorage>, pins: ResourcePins, config: D1Config) -> Self {
        Self {
            handles: D1HandleManager::new(
                config.max_open_databases,
                config.max_queued_operations_per_database,
                Duration::from_millis(config.idle_handle_ttl_ms),
            ),
            storage,
            pins,
            config,
            metrics: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn set_metrics(&self, metrics: Arc<MetricsRegistry>) {
        self.handles.set_metrics(metrics.clone());
        *self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(metrics);
    }

    pub(crate) async fn execute<T, F>(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        timeout: Duration,
        mutation_possible: bool,
        operation: F,
    ) -> Result<T, PlatformError>
    where
        T: Send + 'static,
        F: FnOnce(D1OperationContext<'_>) -> Result<T, PlatformError> + Send + 'static,
    {
        let pin = self.pins.try_pin(resource_id)?;
        let lane = self.handles.acquire(resource_id, timeout).await?;
        let storage = self.storage.clone();
        let config = self.config.clone();
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mutation_started = Arc::new(AtomicBool::new(mutation_possible));
        let mutation_for_task = mutation_started.clone();
        let task = tokio::task::spawn_blocking(move || {
            execute_blocking(
                storage,
                config,
                pin,
                lane,
                metrics,
                account_id,
                resource_id,
                mutation_for_task,
                operation,
            )
        });
        match tokio::time::timeout(timeout.saturating_add(Duration::from_secs(1)), task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(protocol_error()),
            Err(_) if mutation_started.load(Ordering::Acquire) => Err(result_unknown()),
            Err(_) => Err(PlatformError::new(
                ErrorCode::D1Timeout,
                "D1 operation exceeded its wall deadline",
            )),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_blocking<T, F>(
    storage: Arc<PlatformStorage>,
    config: D1Config,
    pin: ResourcePin,
    lane: D1LaneLease,
    metrics: Option<Arc<MetricsRegistry>>,
    account_id: AccountId,
    resource_id: ResourceId,
    mutation_started: Arc<AtomicBool>,
    operation: F,
) -> Result<T, PlatformError>
where
    F: FnOnce(D1OperationContext<'_>) -> Result<T, PlatformError>,
{
    let observed_storage = storage.clone();
    let result = execute_blocking_inner(
        storage,
        config,
        pin,
        lane,
        metrics,
        account_id,
        resource_id,
        mutation_started,
        operation,
    );
    persist_corruption(&observed_storage, account_id, resource_id, &result);
    result
}

#[allow(clippy::too_many_arguments)]
fn execute_blocking_inner<T, F>(
    storage: Arc<PlatformStorage>,
    config: D1Config,
    pin: ResourcePin,
    lane: D1LaneLease,
    metrics: Option<Arc<MetricsRegistry>>,
    account_id: AccountId,
    resource_id: ResourceId,
    mutation_started: Arc<AtomicBool>,
    operation: F,
) -> Result<T, PlatformError>
where
    F: FnOnce(D1OperationContext<'_>) -> Result<T, PlatformError>,
{
    let _pin = pin;
    let _lane = lane;
    let catalog = D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)?;
    if catalog.resource.availability != ResourceAvailability::Healthy {
        return Err(PlatformError::new(
            ErrorCode::ResourceUnavailable,
            "D1 database is quarantined",
        ));
    }
    let paths = D1Paths::open(storage.data_dir().root())?;
    let path = paths.resolve_storage_key(&catalog.storage_key, account_id, resource_id)?;
    let engine = D1Engine::from_record(path, &catalog)?;
    reconcile_restore(&storage, &paths, &catalog, &engine)?;
    reconcile_ingest(&storage, &paths, &catalog, &engine, &config)?;
    complete_ingest(&storage, &paths, &catalog, &engine)?;
    let before = engine.session_version()?;
    let completed_history_required = AtomicBool::new(false);
    let result = operation(D1OperationContext {
        engine: &engine,
        storage: &storage,
        config: &config,
        paths: &paths,
        catalog: &catalog,
        mutation_started: &mutation_started,
        completed_history_required: &completed_history_required,
    });
    let after = engine.session_version()?;
    if after != before {
        mutation_started.store(true, Ordering::Release);
        if after != before.checked_add(1).ok_or_else(invariant)? {
            return Err(result_unknown());
        }
        if completed_history_required.load(Ordering::Acquire) {
            engine.checkpoint(true).map_err(|_| result_unknown())?;
            persist_snapshot(&storage, &paths, &catalog, &engine, after)
                .map_err(|_| result_unknown())?;
            if let Some(intent) =
                D1SnapshotRepository::new(storage.db()).pending_restore(account_id, resource_id)?
            {
                if intent.result_session_version != after {
                    return Err(result_unknown());
                }
                D1SnapshotRepository::new(storage.db())
                    .complete_restore(account_id, resource_id, &intent.id)
                    .map_err(|_| result_unknown())?;
                prune_snapshot_history(
                    &storage,
                    &paths,
                    &catalog,
                    [Some(intent.previous_session_version), None],
                )
                .map_err(|_| result_unknown())?;
            }
            complete_ingest(&storage, &paths, &catalog, &engine).map_err(|_| result_unknown())?;
        }
    }
    if let Some(metrics) = &metrics
        && let Ok(bytes) = engine.wal_bytes()
    {
        metrics.observe_d1_wal_bytes(bytes);
    }
    result
}

fn reconcile_restore(
    storage: &PlatformStorage,
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
    engine: &D1Engine,
) -> Result<(), PlatformError> {
    let account = catalog.resource.account_id;
    let resource = catalog.resource.id;
    let repository = D1SnapshotRepository::new(storage.db());
    let Some(intent) = repository.pending_restore(account, resource)? else {
        return Ok(());
    };
    let current = engine.session_version()?;
    if current == intent.previous_session_version {
        let source = repository.snapshot(account, resource, intent.source_session_version)?;
        let source_path = paths.resolve_snapshot_key(
            &source.snapshot_key,
            account,
            resource,
            source.session_version,
        )?;
        validate_snapshot(paths, catalog, &source)?;
        engine.restore_in_place(
            &source_path,
            catalog,
            source.session_version,
            intent.result_session_version,
        )?;
    } else if current != intent.result_session_version {
        return Err(invariant());
    }
    let latest = repository
        .latest_snapshot(account, resource)?
        .ok_or_else(invariant)?;
    if latest.session_version == intent.previous_session_version {
        persist_snapshot(
            storage,
            paths,
            catalog,
            engine,
            intent.result_session_version,
        )?;
    } else if latest.session_version == intent.result_session_version {
        validate_snapshot(paths, catalog, &latest)?;
    } else {
        return Err(invariant());
    }
    repository.complete_restore(account, resource, &intent.id)?;
    prune_snapshot_history(
        storage,
        paths,
        catalog,
        [Some(intent.previous_session_version), None],
    )
}

fn reconcile_ingest(
    storage: &PlatformStorage,
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
    engine: &D1Engine,
    config: &D1Config,
) -> Result<(), PlatformError> {
    let account = catalog.resource.account_id;
    let resource = catalog.resource.id;
    let repository = D1SnapshotRepository::new(storage.db());
    let Some(transfer) = repository.active_transfer(account, resource)? else {
        return Ok(());
    };
    if transfer.state != D1TransferState::Ingesting {
        return Ok(());
    }
    let current = engine.session_version()?;
    if current
        == transfer
            .at_session_version
            .checked_add(1)
            .ok_or_else(invariant)?
    {
        return Ok(());
    }
    if current != transfer.at_session_version {
        return Err(invariant());
    }
    let bytes = paths.read_transfer(
        transfer.file_key.as_deref().ok_or_else(invariant)?,
        account,
        resource,
        &transfer.id,
        &transfer.filename,
    )?;
    verify_transfer_bytes(&transfer, &bytes)?;
    let sql = std::str::from_utf8(&bytes).map_err(|_| invariant())?;
    engine.import_sql(sql, D1QueryLimits::batch(config)?, |result| {
        repository
            .begin_ingest(
                account,
                &transfer.id,
                result.num_queries,
                result.duration_ms,
                result.rows_read,
                result.rows_written,
                result.size_after,
                checked_wall_now_ms()?,
            )
            .map(|_| ())
    })?;
    Ok(())
}

fn complete_ingest(
    storage: &PlatformStorage,
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
    engine: &D1Engine,
) -> Result<(), PlatformError> {
    let account = catalog.resource.account_id;
    let resource = catalog.resource.id;
    let repository = D1SnapshotRepository::new(storage.db());
    let Some(transfer) = repository.active_transfer(account, resource)? else {
        return Ok(());
    };
    if transfer.state != D1TransferState::Ingesting {
        return Ok(());
    }
    let version = engine.session_version()?;
    if version
        != transfer
            .at_session_version
            .checked_add(1)
            .ok_or_else(invariant)?
    {
        return Err(invariant());
    }
    match repository.latest_snapshot(account, resource)? {
        Some(latest) if latest.session_version == version => {
            validate_snapshot(paths, catalog, &latest)?;
        }
        Some(latest) if latest.session_version < version => {
            engine.checkpoint(true)?;
            persist_snapshot(storage, paths, catalog, engine, version)?;
        }
        None => {
            engine.checkpoint(true)?;
            persist_snapshot(storage, paths, catalog, engine, version)?;
        }
        Some(_) => return Err(invariant()),
    }
    repository.complete_import(
        account,
        &transfer.id,
        version,
        transfer.num_queries.ok_or_else(invariant)?,
        checked_wall_now_ms()?,
    )?;
    prune_snapshot_history(storage, paths, catalog, [None, None])
}

fn verify_transfer_bytes(
    transfer: &open_compute_storage::D1TransferRecord,
    bytes: &[u8],
) -> Result<(), PlatformError> {
    let size = u64::try_from(bytes.len()).map_err(|_| invariant())?;
    let sha256: [u8; 32] = Sha256::digest(bytes).into();
    let md5: [u8; 16] = Md5::digest(bytes).into();
    if transfer.size_bytes != Some(size)
        || transfer.sha256 != Some(sha256)
        || transfer.etag_md5 != Some(md5)
    {
        return Err(invariant());
    }
    Ok(())
}

fn persist_snapshot(
    storage: &PlatformStorage,
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
    engine: &D1Engine,
    version: u64,
) -> Result<D1SnapshotRecord, PlatformError> {
    let account = catalog.resource.account_id;
    let resource = catalog.resource.id;
    let key = D1Paths::snapshot_key(account, resource, version);
    let destination = paths.resolve_snapshot_key(&key, account, resource, version)?;
    if !destination.exists() {
        let staging = paths.snapshot_staging_path(account, resource, version)?;
        let result: Result<(), PlatformError> = (|| {
            engine.online_backup(&staging)?;
            D1Engine::verify_completed_snapshot(&staging, catalog, version)?;
            paths.publish_snapshot(&staging, account, resource, version)?;
            Ok(())
        })();
        if result.is_err() && staging.exists() {
            let _ = std::fs::remove_file(&staging);
        }
        result?;
    }
    let snapshot = snapshot_evidence(&destination, catalog, version)?;
    D1SnapshotRepository::new(storage.db()).record_completed_snapshot(
        account,
        resource,
        version,
        &key,
        &snapshot.0,
        snapshot.1,
        checked_wall_now_ms()?,
    )
}

fn prune_snapshot_history(
    storage: &PlatformStorage,
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
    protected_session_versions: [Option<u64>; 2],
) -> Result<(), PlatformError> {
    let account = catalog.resource.account_id;
    let resource = catalog.resource.id;
    let removed = D1SnapshotRepository::new(storage.db()).prune_completed_snapshots(
        account,
        resource,
        D1_COMPLETED_HISTORY_POINTS,
        protected_session_versions,
    )?;
    for snapshot in removed {
        let _ = paths.remove_pruned_snapshot(
            &snapshot.snapshot_key,
            account,
            resource,
            snapshot.session_version,
        );
    }
    Ok(())
}

fn prune_expired_transfer_history(
    storage: &PlatformStorage,
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
) -> Result<(), PlatformError> {
    let account = catalog.resource.account_id;
    let resource = catalog.resource.id;
    let now_ms = checked_wall_now_ms()?;
    let repository = D1SnapshotRepository::new(storage.db());
    if let Some(active) = repository.active_transfer(account, resource)?
        && active.state != D1TransferState::Ingesting
        && active.token_expires_at_ms <= now_ms
    {
        repository.expire_transfer(account, &active.id, now_ms)?;
    }
    let removed = repository.prune_expired_terminal_transfers(account, resource, now_ms)?;
    for transfer in removed {
        if let Some(key) = transfer.file_key {
            let _ = paths.remove_pruned_transfer(
                &key,
                account,
                resource,
                &transfer.id,
                &transfer.filename,
            );
        }
    }
    Ok(())
}

fn validate_snapshot(
    paths: &D1Paths,
    catalog: &D1DatabaseRecord,
    record: &D1SnapshotRecord,
) -> Result<D1SnapshotRecord, PlatformError> {
    let path = paths.resolve_snapshot_key(
        &record.snapshot_key,
        catalog.resource.account_id,
        catalog.resource.id,
        record.session_version,
    )?;
    let evidence = snapshot_evidence(&path, catalog, record.session_version)?;
    if evidence.0 != record.sha256 || evidence.1 != record.size_bytes {
        return Err(invariant());
    }
    Ok(record.clone())
}

fn snapshot_evidence(
    path: &std::path::Path,
    catalog: &D1DatabaseRecord,
    version: u64,
) -> Result<([u8; 32], u64), PlatformError> {
    if !path.is_file() {
        return Err(invariant());
    }
    D1Engine::verify_completed_snapshot(path, catalog, version)?;
    let mut file = std::fs::File::open(path).map_err(|_| invariant())?;
    let size = file.metadata().map_err(|_| invariant())?.len();
    if size == 0 {
        return Err(invariant());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| invariant())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((digest.finalize().into(), size))
}

fn checked_wall_now_ms() -> Result<i64, PlatformError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| invariant())?;
    i64::try_from(duration.as_millis()).map_err(|_| invariant())
}

fn persist_corruption<T>(
    storage: &PlatformStorage,
    account_id: AccountId,
    resource_id: ResourceId,
    result: &Result<T, PlatformError>,
) {
    let Err(error) = result else { return };
    let code = match error.code() {
        ErrorCode::D1DatabaseCorrupt => "D1_DATABASE_CORRUPT",
        ErrorCode::D1IdentityMismatch => "D1_IDENTITY_MISMATCH",
        _ => return,
    };
    let Ok(now_ms) = checked_wall_now_ms() else {
        return;
    };
    let _ = ResourceRepository::new(storage.db()).set_availability(
        account_id,
        resource_id,
        ResourceAvailability::Unavailable,
        Some(code),
        now_ms,
    );
}

fn result_unknown() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1ResultUnknown,
        "D1 mutation committed but completed snapshot durability is unknown",
    )
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "D1 completed history does not match the live database",
    )
}

fn protocol_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1InternalProtocolError,
        "D1 execution task failed",
    )
}

#[derive(Clone)]
pub(crate) struct D1HandleManager {
    max_open: usize,
    queue_limit: usize,
    idle_ttl: Duration,
    lanes: Arc<Mutex<HashMap<ResourceId, Arc<D1Lane>>>>,
    metrics: Arc<Mutex<Option<Arc<MetricsRegistry>>>>,
}

impl D1HandleManager {
    pub(crate) fn new(global: u32, queue_limit: u32, idle_ttl: Duration) -> Self {
        Self {
            max_open: global.max(1) as usize,
            queue_limit: queue_limit.max(1) as usize,
            idle_ttl,
            lanes: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Mutex::new(None)),
        }
    }

    fn set_metrics(&self, metrics: Arc<MetricsRegistry>) {
        *self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(metrics);
    }

    pub(crate) async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<D1LaneLease, PlatformError> {
        let (lane, open_databases) = {
            let mut lanes = self
                .lanes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(lane) = lanes.get(&resource) {
                (lane.clone(), lanes.len())
            } else {
                let now = Instant::now();
                lanes.retain(|_, lane| {
                    lane.queued.load(Ordering::Acquire) > 0
                        || lane.semaphore.available_permits() == 0
                        || now.duration_since(lane.last_used()) < self.idle_ttl
                });
                if lanes.len() >= self.max_open {
                    let candidate = lanes
                        .iter()
                        .filter(|(_, lane)| {
                            lane.queued.load(Ordering::Acquire) == 0
                                && lane.semaphore.available_permits() == 1
                        })
                        .min_by_key(|(_, lane)| lane.last_used())
                        .map(|(id, _)| *id);
                    let Some(candidate) = candidate else {
                        return Err(overloaded());
                    };
                    lanes.remove(&candidate);
                }
                let lane = Arc::new(D1Lane {
                    semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
                    queued: AtomicUsize::new(0),
                    last_used: Mutex::new(now),
                });
                lanes.insert(resource, lane.clone());
                (lane, lanes.len())
            }
        };
        let prior = lane.queued.fetch_add(1, Ordering::AcqRel);
        if let Some(metrics) = self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            metrics.set_d1_open_databases(open_databases as u64);
            metrics.observe_d1_queue_depth(prior.saturating_add(1) as u64);
        }
        if prior >= self.queue_limit {
            lane.queued.fetch_sub(1, Ordering::AcqRel);
            return Err(overloaded());
        }
        let permit = tokio::time::timeout(timeout, lane.semaphore.clone().acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded());
        lane.queued.fetch_sub(1, Ordering::AcqRel);
        Ok(D1LaneLease {
            _resource: permit?,
            lane,
        })
    }
}

struct D1Lane {
    semaphore: Arc<tokio::sync::Semaphore>,
    queued: AtomicUsize,
    last_used: Mutex<Instant>,
}

impl D1Lane {
    fn last_used(&self) -> Instant {
        *self
            .last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(crate) struct D1LaneLease {
    _resource: tokio::sync::OwnedSemaphorePermit,
    lane: Arc<D1Lane>,
}

impl Drop for D1LaneLease {
    fn drop(&mut self) {
        *self
            .lane
            .last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }
}

fn overloaded() -> PlatformError {
    PlatformError::new(ErrorCode::D1Overloaded, "D1 operation queue is saturated")
}

#[cfg(test)]
#[path = "d1_coordinator_tests.rs"]
mod tests;
