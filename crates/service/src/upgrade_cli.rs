//! Offline forward-only release upgrade commands.

use crate::backup_cli::{assert_runtime_quiescent, verified_snapshot};
use crate::capabilities::{platform_config_policy_sha256, platform_release_metadata};
use crate::config_load::LoadedConfig;
use open_compute_core::{ErrorCode, PlatformError, PlatformSnapshotManifestV1};
use open_compute_storage::{
    DataDir, OfflineSchemaState, apply_offline_upgrade, inspect_control_db, inspect_master_key,
    inspect_offline_schema,
};
use serde::Serialize;

/// Versioned upgrade preflight or application result.
#[derive(Clone, Debug, Serialize)]
pub struct UpgradeResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Operation result token.
    pub result: String,
    /// Verified rollback snapshot.
    pub from_snapshot: String,
    /// Stable platform identity.
    pub platform_id: String,
    /// Schema state before the operation.
    pub before: OfflineSchemaState,
    /// Target or applied schema state.
    pub target: OfflineSchemaState,
    /// Completion timestamp.
    pub completed_at_ms: i64,
}

/// Verify that the stopped platform and rollback snapshot can be upgraded by this release.
pub async fn upgrade_check(
    loaded: &LoadedConfig,
    snapshot_id: &str,
) -> Result<UpgradeResult, PlatformError> {
    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage)?;
    assert_runtime_quiescent(loaded, &data_dir)?;
    let manifest = verified_snapshot(loaded, snapshot_id).await?;
    let now_ms = unix_ms();
    let before = inspect_offline_schema(
        &data_dir,
        loaded.config.storage.sqlite_busy_timeout_ms,
        now_ms,
    )?;
    let (_, identity) = inspect_control_db(
        &data_dir.control_db_path(),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let key = inspect_master_key(&loaded.config.storage)?;
    validate_upgrade_source(
        loaded,
        &manifest,
        &before,
        &identity.platform_id.to_string(),
        key.fingerprint(),
    )?;
    ensure_upgrade_headroom(loaded, manifest.totals.bytes)?;
    let metadata = platform_release_metadata(loaded)?;
    let target = target_state(&before, &metadata.release);
    Ok(UpgradeResult {
        schema_version: 1,
        result: "upgrade_ready".to_owned(),
        from_snapshot: snapshot_id.to_owned(),
        platform_id: identity.platform_id.to_string(),
        before,
        target,
        completed_at_ms: now_ms,
    })
}

/// Verify the rollback anchor, then idempotently apply all pending migrations offline.
pub async fn upgrade_apply(
    loaded: &LoadedConfig,
    snapshot_id: &str,
) -> Result<UpgradeResult, PlatformError> {
    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage)?;
    assert_runtime_quiescent(loaded, &data_dir)?;
    let manifest = verified_snapshot(loaded, snapshot_id).await?;
    let now_ms = unix_ms();
    let before = inspect_offline_schema(
        &data_dir,
        loaded.config.storage.sqlite_busy_timeout_ms,
        now_ms,
    )?;
    let (_, identity) = inspect_control_db(
        &data_dir.control_db_path(),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let key = inspect_master_key(&loaded.config.storage)?;
    validate_upgrade_source(
        loaded,
        &manifest,
        &before,
        &identity.platform_id.to_string(),
        key.fingerprint(),
    )?;
    ensure_upgrade_headroom(loaded, manifest.totals.bytes)?;
    let target = apply_offline_upgrade(
        &data_dir,
        loaded.config.storage.sqlite_busy_timeout_ms,
        now_ms,
    )?;
    let completed_at_ms = unix_ms();
    let receipt = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "result": "upgrade_applied",
        "from_snapshot": snapshot_id,
        "platform_id": identity.platform_id,
        "before": before,
        "target": target,
        "completed_at_ms": completed_at_ms,
    }))
    .map_err(|_| upgrade_invalid())?;
    data_dir.write_operation_receipt("last-upgrade.json", &receipt)?;
    Ok(UpgradeResult {
        schema_version: 1,
        result: "upgrade_applied".to_owned(),
        from_snapshot: snapshot_id.to_owned(),
        platform_id: identity.platform_id.to_string(),
        before,
        target,
        completed_at_ms,
    })
}

fn validate_upgrade_source(
    loaded: &LoadedConfig,
    manifest: &PlatformSnapshotManifestV1,
    before: &OfflineSchemaState,
    platform_id: &str,
    master_key_fingerprint: &str,
) -> Result<(), PlatformError> {
    let metadata = platform_release_metadata(loaded)?;
    let source = &manifest.source_release;
    if manifest.platform_id != platform_id
        || manifest.master_key_fingerprint != master_key_fingerprint
        || manifest.config_policy_sha256 != platform_config_policy_sha256(loaded)?
        || manifest.source_schemas.get("control").copied() != Some(source.control_schema_version)
        || manifest.source_schemas.get("scheduler").copied()
            != Some(source.scheduler_schema_version)
        || manifest.source_schemas.get("kv").copied() != Some(source.kv_schema_version_max)
        || manifest.source_schemas.get("d1").copied() != Some(source.d1_schema_version_max)
        || before.control < metadata.upgrade_from_control_schema_min
        || before.control < source.control_schema_version
        || before.control > metadata.release.control_schema_version
        || before.scheduler < source.scheduler_schema_version
        || before.scheduler > metadata.release.scheduler_schema_version
        || before.kv_min < source.kv_schema_version_min
        || before.kv_max > metadata.release.kv_schema_version_max
        || before.d1_min < source.d1_schema_version_min
        || before.d1_max > metadata.release.d1_schema_version_max
        || !metadata
            .upgrade_from_platform_versions
            .contains(&source.platform_version)
        || source.workerd_version != metadata.release.workerd_version
        || source.workerd_lock_sha256 != metadata.release.workerd_lock_sha256
        || source.runtime_assets_sha256 != metadata.release.runtime_assets_sha256
        || source.facade_capability_version != metadata.release.facade_capability_version
        || source.kv_schema_version_min != metadata.release.kv_schema_version_min
        || source.kv_schema_version_max != metadata.release.kv_schema_version_max
        || source.d1_schema_version_min != metadata.release.d1_schema_version_min
        || source.d1_schema_version_max != metadata.release.d1_schema_version_max
        || source.snapshot_format_version != metadata.release.snapshot_format_version
        || source.compatibility_policy_sha256 != metadata.release.compatibility_policy_sha256
    {
        return Err(upgrade_invalid());
    }
    Ok(())
}

fn target_state(
    before: &OfflineSchemaState,
    release: &open_compute_core::PlatformReleaseIdentityV1,
) -> OfflineSchemaState {
    OfflineSchemaState {
        control: release.control_schema_version,
        scheduler: release.scheduler_schema_version,
        kv_min: release.kv_schema_version_min,
        kv_max: release.kv_schema_version_max,
        d1_min: release.d1_schema_version_min,
        d1_max: release.d1_schema_version_max,
        kv_files: before.kv_files,
        d1_files: before.d1_files,
    }
}

fn ensure_upgrade_headroom(
    loaded: &LoadedConfig,
    rollback_bytes: u64,
) -> Result<(), PlatformError> {
    let stat = rustix::fs::statvfs(&loaded.config.storage.data_dir).map_err(|_| {
        PlatformError::new(
            ErrorCode::StoragePressure,
            "upgrade free space could not be measured",
        )
    })?;
    let required = loaded
        .config
        .storage
        .free_space_hard_bytes
        .saturating_add(loaded.config.hardening.snapshot_staging_margin_bytes)
        .saturating_add(rollback_bytes);
    if stat.f_bavail.saturating_mul(stat.f_frsize) < required {
        return Err(PlatformError::new(
            ErrorCode::StoragePressure,
            "upgrade staging would violate the host storage reserve",
        ));
    }
    Ok(())
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn upgrade_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ReleaseUnsupported,
        "release does not support this offline upgrade source",
    )
}
