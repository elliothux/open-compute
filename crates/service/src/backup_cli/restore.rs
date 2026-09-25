//! Fresh-host S3 snapshot restore into one explicit instance data root.

use super::*;

/// Restore one exact-release snapshot into a nonexistent or empty fresh-host data directory.
/// `other_instance_ids` are existing registered authorities in the selected OCD scope.
pub async fn backup_restore(
    loaded: &LoadedConfig,
    snapshot_id: &str,
    other_instance_ids: &[InstanceId],
) -> Result<BackupRestoreResult, PlatformError> {
    let started = Instant::now();
    let target = &loaded.config.data.path;
    if matches!(loaded.config.object_storage, ObjectStorageConfig::Local(_)) {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "Local fresh-host recovery requires an independently backed-up complete instance directory; snapshot restore requires S3 object authority",
        ));
    }
    if loaded.config.data.master_key_env.is_none()
        && loaded.config.data.master_key_file.starts_with(target)
    {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "fresh-host restore requires a recovery master key outside data_dir or via env",
        ));
    }
    let key = inspect_master_key(&loaded.config.data)?;
    let (backend, instance_id) = discover_snapshot_backend(&loaded.config, snapshot_id).await?;
    let objects = SnapshotObjectStore::new(backend, instance_id);
    let manifest = load_manifest(loaded, &objects, snapshot_id, &key).await?;
    let restored_id: InstanceId = manifest
        .instance_id
        .parse()
        .map_err(|_| snapshot_invalid())?;
    if other_instance_ids.contains(&restored_id) {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "restore identity belongs to another registered instance",
        ));
    }
    let current_release = platform_capabilities(&loaded.config)?.release;
    if manifest.source_release != current_release {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "restore requires the exact source release identity",
        ));
    }
    if manifest.config_policy_sha256 != platform_config_policy_sha256(loaded)? {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "restore requires the snapshot source storage and product policy",
        ));
    }
    verify_snapshot_objects(&objects, &manifest, true).await?;
    ensure_restore_headroom(loaded, manifest.totals.bytes)?;
    let restore = open_compute_storage::RestoreTarget::acquire(target)
        .map_err(|error| restore_stage(&error, "restore target acquisition failed"))?;
    for file in &manifest.files {
        let destination = restore
            .destination_for(&file.restore_path)
            .map_err(|error| restore_stage(&error, "restore destination validation failed"))?;
        objects
            .download_file(&file.object_key, &destination, &file.sha256, file.size)
            .await?;
    }
    let restored_at_ms = open_compute_core::wall_time_ms();
    let duration_ms = elapsed_ms(started);
    let receipt = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "snapshot_id": manifest.snapshot_id,
        "instance_id": manifest.instance_id,
        "source_release": manifest.source_release,
        "manifest_mac": manifest.manifest_mac,
        "bytes": manifest.totals.bytes,
        "restored_at_ms": restored_at_ms,
        "duration_ms": duration_ms,
        "smoke_verified": false,
        "verified": true,
    }))
    .map_err(|_| snapshot_invalid())?;
    let installed = restore.validate_and_publish(
        &manifest,
        key.fingerprint(),
        loaded.config.data.sqlite_busy_timeout_ms,
        &receipt,
    )?;
    Ok(BackupRestoreResult {
        schema_version: 1,
        snapshot_id: manifest.snapshot_id,
        instance_id: manifest.instance_id,
        data_dir: installed.to_string_lossy().into_owned(),
        bytes: manifest.totals.bytes,
        restored_at_ms,
        duration_ms,
    })
}
