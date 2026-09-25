use super::*;

#[allow(
    clippy::too_many_arguments,
    reason = "transport boundary inputs mirror the wire contract"
)]
pub(super) async fn run_worker_maintenance(
    storage: &Arc<PlatformStorage>,
    store: &ArtifactStore,
    cache: &Arc<ArtifactCache>,
    response_cache: &Arc<CacheManager>,
    pins: &VersionPins,
    config: &open_compute_core::WorkersConfig,
    snapshot_pins: &SnapshotPins,
    metrics: &Arc<MetricsRegistry>,
) {
    let now = open_compute_core::wall_time_ms();
    let storage_for_db = storage.clone();
    let batch = config.delete_recovery_batch;
    let policy = config.clone();
    let pass = tokio::task::spawn_blocking(move || {
        let repo = WorkerRepository::new(storage_for_db.db());
        let _ = repo.prune_expired_idempotency(now, batch)?;
        let candidates = repo.retention_candidates(
            now,
            policy.version_min_retention_ms,
            policy.retain_ready_versions,
            policy.retain_rejected_versions,
            batch,
        )?;
        Ok::<_, PlatformError>(candidates)
    })
    .await;
    let candidates = match pass {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            tracing::warn!(
                code = error.code().as_str(),
                "Worker maintenance DB pass failed"
            );
            return;
        }
        Err(_) => {
            tracing::warn!("Worker maintenance DB task failed");
            return;
        }
    };
    for candidate in candidates {
        let storage_for_begin = storage.clone();
        let begin = tokio::task::spawn_blocking(move || {
            WorkerRepository::new(storage_for_begin.db()).begin_version_delete(
                candidate.instance_id,
                candidate.worker_id,
                candidate.version_id,
            )
        })
        .await;
        if !matches!(begin, Ok(Ok(()))) {
            pins.unfence(candidate.version_id);
            continue;
        }
        if pins
            .fence_and_wait(
                candidate.version_id,
                Duration::from_millis(config.delete_drain_timeout_ms),
            )
            .await
            .is_err()
        {
            // Keep both the SQLite deleting state and memory fence. A future
            // process restart has no surviving in-flight pins and recovers it.
            continue;
        }
        let storage_for_finish = storage.clone();
        let finish = tokio::task::spawn_blocking(move || {
            WorkerRepository::new(storage_for_finish.db()).finalize_version_delete(
                candidate.instance_id,
                candidate.worker_id,
                candidate.version_id,
                RequestId::generate(),
                now,
            )
        })
        .await;
        if matches!(finish, Ok(Ok(()))) {
            pins.retire_fence(candidate.version_id);
        } else {
            tracing::warn!("Worker retention finalization failed");
        }
    }
    if let Err(error) = gc_worker_artifacts(
        storage,
        store,
        config,
        snapshot_pins,
        Some(response_cache.clone()),
    )
    .await
    {
        tracing::warn!(
            code = error.code().as_str(),
            "Worker artifact GC pass failed"
        );
    }
    if let Err(error) = cache.evict_if_needed().await {
        tracing::warn!(
            code = error.code().as_str(),
            "Worker cache eviction pass failed"
        );
    }
    match response_cache.stats(open_compute_core::wall_time_ms()) {
        Ok(stats) => metrics.set_response_cache_stats(stats),
        Err(error) => tracing::warn!(
            code = error.code().as_str(),
            "Response cache metrics inspection failed"
        ),
    }
}

pub(crate) async fn gc_worker_artifacts(
    storage: &Arc<PlatformStorage>,
    store: &ArtifactStore,
    config: &open_compute_core::WorkersConfig,
    snapshot_pins: &SnapshotPins,
    response_cache: Option<Arc<CacheManager>>,
) -> Result<u64, PlatformError> {
    let gc_fence = store.fence_version_gc().await;
    let storage_for_refs = storage.clone();
    let references = match tokio::task::spawn_blocking(move || {
        WorkerRepository::new(storage_for_refs.db()).referenced_artifacts()
    })
    .await
    {
        Ok(Ok(references)) => references,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::ArtifactUnavailable,
                "artifact reference inspection failed",
            ));
        }
    };
    let mut retained = HashSet::new();
    for (digest, size) in references {
        if let Ok(reference) = ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size) {
            retained.insert(reference);
        }
    }
    if let Some(response_cache) = response_cache {
        let cache_references =
            match tokio::task::spawn_blocking(move || response_cache.referenced_bodies()).await {
                Ok(Ok(references)) => references,
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(PlatformError::new(
                        ErrorCode::CacheUnavailable,
                        "cache reference inspection failed",
                    ));
                }
            };
        for body in cache_references {
            match ArtifactRef::new(ARTIFACT_KEY_VERSION, &body.sha256, body.size) {
                Ok(reference) => {
                    retained.insert(reference);
                }
                Err(error) => return Err(error),
            }
        }
    }
    snapshot_pins.extend_artifacts(&mut retained)?;
    let grace = SystemTime::now()
        .checked_sub(Duration::from_millis(config.artifact_gc_grace_ms))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    store.gc_unreferenced(&gc_fence, &retained, grace).await
}

pub(crate) async fn run_kv_maintenance(
    storage: &Arc<PlatformStorage>,
    pins: &ResourcePins,
    config: &open_compute_core::KvConfig,
    metrics: &Arc<MetricsRegistry>,
) {
    let storage = storage.clone();
    let pins = pins.clone();
    let metrics = metrics.clone();
    let batch = usize::try_from(config.max_connections.min(64)).unwrap_or(64);
    let pass = tokio::task::spawn_blocking(move || {
        let instance_id = storage.identity().instance_id;
        let catalog = open_compute_storage::KvNamespaceRepository::new(storage.db());
        let resources = open_compute_storage::ResourceRepository::new(storage.db());
        let paths = open_compute_storage::KvPaths::open(storage.data_dir().root())?;
        let now = open_compute_core::wall_time_ms();
        for record in catalog.list(instance_id)?.into_iter().take(batch) {
            if record.resource.state != open_compute_core::ResourceState::Ready
                || pins.count(record.resource.id) != 0
            {
                continue;
            }
            let path = paths.resolve_storage_key(
                &record.storage_key,
                record.resource.instance_id,
                record.resource.id,
            )?;
            let engine = match open_compute_storage::KvEngine::from_record(path, &record) {
                Ok(engine) => engine,
                Err(error) => {
                    metrics.inc_kv_corruption(2);
                    let code = if error.code() == ErrorCode::KvCorrupt {
                        "KV_CORRUPT"
                    } else {
                        "KV_UNAVAILABLE"
                    };
                    let _ = resources.set_availability(
                        record.resource.instance_id,
                        record.resource.id,
                        open_compute_core::ResourceAvailability::Unavailable,
                        Some(code),
                        now,
                    );
                    continue;
                }
            };
            if let Ok(wal_bytes) = engine.wal_bytes() {
                metrics.observe_kv_wal_bytes(wal_bytes);
            }
            metrics.inc_kv_maintenance(KvMaintenance::Gc, engine.gc_expired(now, 256).is_ok());
            if record
                .last_quick_check_ms
                .is_none_or(|last| now.saturating_sub(last) >= 60 * 60 * 1000)
            {
                match engine.quick_check() {
                    Ok(()) => {
                        let _ = catalog.record_quick_check(record.resource.id, now);
                    }
                    Err(error) => {
                        metrics.inc_sqlite_check_failure();
                        metrics.inc_kv_corruption(2);
                        let code = if error.code() == ErrorCode::KvCorrupt {
                            "KV_CORRUPT"
                        } else {
                            "KV_UNAVAILABLE"
                        };
                        let _ = resources.set_availability(
                            record.resource.instance_id,
                            record.resource.id,
                            open_compute_core::ResourceAvailability::Unavailable,
                            Some(code),
                            now,
                        );
                    }
                }
            }
            metrics.inc_kv_maintenance(KvMaintenance::Checkpoint, engine.checkpoint(false).is_ok());
        }
        Ok::<_, PlatformError>(())
    })
    .await;
    match pass {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(code = error.code().as_str(), "KV maintenance pass failed");
        }
        Err(_) => tracing::warn!("KV maintenance task failed"),
    }
}
