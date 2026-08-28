//! Authenticated post-restore smoke receipt transition.

use crate::backup_cli::{assert_runtime_quiescent, verified_snapshot};
use crate::capabilities::platform_capabilities;
use crate::config_load::LoadedConfig;
use open_compute_core::{ErrorCode, PlatformError, PlatformReleaseIdentityV1};
use open_compute_storage::{DataDir, inspect_control_db};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of recording an operator's completed post-restore smoke rehearsal.
#[derive(Clone, Debug, Serialize)]
pub struct BackupRestoreSmokeResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Restored snapshot identity.
    pub snapshot_id: String,
    /// Stable restored platform identity.
    pub platform_id: String,
    /// Time at which the operator attested the successful smoke.
    pub attested_at_ms: i64,
    /// Receipt state after the atomic update.
    pub smoke_verified: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RestoreReceiptV1 {
    schema_version: u32,
    snapshot_id: String,
    platform_id: String,
    source_release: PlatformReleaseIdentityV1,
    manifest_mac: String,
    bytes: u64,
    restored_at_ms: i64,
    duration_ms: u64,
    smoke_verified: bool,
    verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    smoke_attested_at_ms: Option<i64>,
}

/// Atomically attest a successful external post-restore smoke after the daemon is stopped again.
pub async fn backup_attest_restore_smoke(
    loaded: &LoadedConfig,
    snapshot_id: &str,
    passed: bool,
) -> Result<BackupRestoreSmokeResult, PlatformError> {
    if !passed {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "restore smoke attestation requires an explicit passed result",
        ));
    }
    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage)?;
    assert_runtime_quiescent(&data_dir)?;
    let manifest = verified_snapshot(loaded, snapshot_id).await?;
    let current_release = platform_capabilities(&loaded.config)?.release;
    let (_, identity) = inspect_control_db(
        &data_dir.control_db_path(),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let bytes = data_dir.read_operation_receipt("last-restore.json", 64 * 1024)?;
    let mut receipt: RestoreReceiptV1 =
        serde_json::from_slice(&bytes).map_err(|_| receipt_invalid())?;
    if receipt.schema_version != 1
        || !receipt.verified
        || receipt.snapshot_id != snapshot_id
        || receipt.snapshot_id != manifest.snapshot_id
        || receipt.platform_id != identity.platform_id.to_string()
        || receipt.platform_id != manifest.platform_id
        || receipt.source_release != manifest.source_release
        || receipt.source_release != current_release
        || receipt.manifest_mac != manifest.manifest_mac
        || receipt.bytes != manifest.totals.bytes
    {
        return Err(receipt_invalid());
    }
    let attested_at_ms = receipt.smoke_attested_at_ms.unwrap_or_else(unix_ms);
    receipt.smoke_verified = true;
    receipt.smoke_attested_at_ms = Some(attested_at_ms);
    let encoded = serde_json::to_vec(&receipt).map_err(|_| receipt_invalid())?;
    data_dir.write_operation_receipt("last-restore.json", &encoded)?;
    Ok(BackupRestoreSmokeResult {
        schema_version: 1,
        snapshot_id: receipt.snapshot_id,
        platform_id: receipt.platform_id,
        attested_at_ms,
        smoke_verified: true,
    })
}

fn receipt_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RestoreInvalid,
        "restore smoke receipt failed authentication or state validation",
    )
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}
