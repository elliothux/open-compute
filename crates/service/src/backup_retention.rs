//! Authenticated, non-mutating platform snapshot retention planning.

use crate::backup_cli::{connect_snapshot_client, load_manifest};
use crate::config_load::LoadedConfig;
use open_compute_artifacts::SnapshotObjectStore;
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_storage::{inspect_control_db, inspect_master_key};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// One authenticated committed snapshot in a retention dry-run plan.
#[derive(Clone, Debug, Serialize)]
pub struct BackupRetentionEntry {
    /// Snapshot identity.
    pub snapshot_id: String,
    /// Bounded operator label.
    pub label: String,
    /// Snapshot commit audit timestamp.
    pub created_at_ms: i64,
    /// Local file count.
    pub files: u32,
    /// Local byte total.
    pub bytes: u64,
}

/// Authenticated, non-mutating retention plan.
#[derive(Clone, Debug, Serialize)]
pub struct BackupRetentionPlan {
    /// Output schema version.
    pub schema_version: u32,
    /// Number of newest snapshots retained unconditionally.
    pub keep_last: u32,
    /// Optional maximum age in seconds before a snapshot becomes eligible.
    pub max_age_seconds: Option<u64>,
    /// Exact operator labels retained unconditionally.
    pub keep_labels: Vec<String>,
    /// Snapshots the policy retains.
    pub keep: Vec<BackupRetentionEntry>,
    /// Snapshots eligible for individual authenticated deletion.
    pub delete: Vec<BackupRetentionEntry>,
    /// Aggregate local bytes eligible for deletion.
    pub delete_bytes: u64,
}

/// Generate an authenticated dry-run plan without deleting any snapshot object.
pub async fn backup_retention_plan(
    loaded: &LoadedConfig,
    keep_last: u32,
    max_age_seconds: Option<u64>,
    mut keep_labels: Vec<String>,
) -> Result<BackupRetentionPlan, PlatformError> {
    if keep_last > 10_000 || max_age_seconds == Some(0) || keep_labels.len() > 128 {
        return Err(snapshot_invalid());
    }
    keep_labels.sort();
    keep_labels.dedup();
    for label in &keep_labels {
        validate_label(label)?;
    }
    let (_, identity) = inspect_control_db(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let key = inspect_master_key(&loaded.config.storage)?;
    let objects = SnapshotObjectStore::new(connect_snapshot_client(loaded)?, identity.platform_id);
    let now_ms = unix_ms();
    let max_age_ms = max_age_seconds.and_then(|seconds| seconds.checked_mul(1_000));
    if max_age_seconds.is_some() && max_age_ms.is_none() {
        return Err(snapshot_invalid());
    }
    let mut manifests = Vec::new();
    for snapshot in objects.list_committed().await? {
        let manifest = load_manifest(loaded, &objects, &snapshot.snapshot_id, &key).await?;
        if manifest.created_at_ms > now_ms {
            return Err(snapshot_invalid());
        }
        manifests.push(manifest);
    }
    manifests.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
    });
    let mut keep = Vec::new();
    let mut delete = Vec::new();
    for (index, manifest) in manifests.into_iter().enumerate() {
        let within_keep_last = index < keep_last as usize;
        let label_kept = keep_labels.binary_search(&manifest.label).is_ok();
        let old_enough = max_age_ms.is_none_or(|age| {
            now_ms.saturating_sub(manifest.created_at_ms) >= i64::try_from(age).unwrap_or(i64::MAX)
        });
        let entry = BackupRetentionEntry {
            snapshot_id: manifest.snapshot_id,
            label: manifest.label,
            created_at_ms: manifest.created_at_ms,
            files: manifest.totals.files,
            bytes: manifest.totals.bytes,
        };
        if within_keep_last || label_kept || !old_enough {
            keep.push(entry);
        } else {
            delete.push(entry);
        }
    }
    let delete_bytes = delete
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.bytes));
    Ok(BackupRetentionPlan {
        schema_version: 1,
        keep_last,
        max_age_seconds,
        keep_labels,
        keep,
        delete,
        delete_bytes,
    })
}

fn validate_label(label: &str) -> Result<(), PlatformError> {
    if label.is_empty() || label.len() > 128 || label.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(snapshot_invalid());
    }
    Ok(())
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn snapshot_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SnapshotInvalid,
        "platform snapshot operation failed validation",
    )
}
