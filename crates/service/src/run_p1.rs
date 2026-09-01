//! P1 startup schema fence, offline receipts, and fixed-series metric refresh.

use crate::config_load::LoadedConfig;
use crate::health::HealthCoordinator;
use crate::metrics::MetricsRegistry;
use open_compute_core::{ComponentName, ComponentState, ErrorCode, PlatformError, ReadinessReason};
use open_compute_storage::{DataDir, PlatformStorage};
use std::time::{Duration, SystemTime};

pub(crate) fn require_current_serving_schema(loaded: &LoadedConfig) -> Result<(), PlatformError> {
    let path = loaded.config.storage.data_dir.join("control.sqlite");
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "control database metadata is not accessible",
            ));
        }
    };
    if metadata.len() == 0 {
        return Ok(());
    }
    let db = open_compute_storage::ControlDb::open_readonly_wal_aware(
        &path,
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    // inspect_schema already refuses too-new schemas and checksum mismatches.
    // A prefix of this binary's lineage (including user_version 0) is an
    // unfinished first start; bootstrap apply() continues those migrations.
    open_compute_storage::migrations::inspect_schema(&db)?;
    Ok(())
}

pub(crate) fn load_offline_metrics_receipts(data_dir: &DataDir, metrics: &MetricsRegistry) {
    if let Some(receipt) = load_operation_receipt(data_dir, "last-snapshot.json") {
        let bytes = receipt.get("bytes").and_then(serde_json::Value::as_u64);
        let created_at_ms = receipt
            .get("created_at_ms")
            .and_then(serde_json::Value::as_i64);
        let duration_ms = receipt
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64);
        match (bytes, created_at_ms, duration_ms) {
            (Some(bytes), Some(created_at_ms), Some(duration_ms)) => metrics
                .record_snapshot_receipt_at(
                    bytes,
                    Duration::from_millis(duration_ms),
                    created_at_ms,
                ),
            _ => metrics.inc_snapshot_inspect_failure(),
        }
    }
    if let Some(receipt) = load_operation_receipt(data_dir, "last-restore.json") {
        let restored_at_ms = receipt
            .get("restored_at_ms")
            .and_then(serde_json::Value::as_i64);
        let duration_ms = receipt
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64);
        let smoke_verified = receipt
            .get("smoke_verified")
            .and_then(serde_json::Value::as_bool);
        if let (Some(restored_at_ms), Some(duration_ms), Some(smoke_verified)) =
            (restored_at_ms, duration_ms, smoke_verified)
        {
            metrics.record_restore_receipt(
                restored_at_ms,
                Duration::from_millis(duration_ms),
                smoke_verified,
            );
        }
    }
}

pub(crate) fn update_operations_health(
    data_dir: &DataDir,
    stale_after_ms: u64,
    health: &HealthCoordinator,
) -> Result<(), PlatformError> {
    let now = unix_ms();
    let fresh = load_operation_receipt(data_dir, "last-snapshot.json").is_some_and(|receipt| {
        receipt.get("verified").and_then(serde_json::Value::as_bool) == Some(true)
            && receipt
                .get("created_at_ms")
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|created_at_ms| {
                    created_at_ms > 0
                        && created_at_ms <= now
                        && u64::try_from(now.saturating_sub(created_at_ms))
                            .is_ok_and(|age| age <= stale_after_ms)
                })
    });
    health.set_component(
        ComponentName::Operations,
        if fresh {
            ComponentState::Healthy
        } else {
            ComponentState::Degraded
        },
        Some(if fresh {
            ReadinessReason::Ready
        } else {
            ReadinessReason::SnapshotStale
        }),
    )
}

pub(crate) fn refresh_metrics(
    storage: &PlatformStorage,
    metrics: &MetricsRegistry,
    emergency_reserve_bytes: u64,
) -> Result<(), PlatformError> {
    metrics.set_disk_admission(&storage.admission_snapshot()?, emergency_reserve_bytes);
    let inventory = open_compute_storage::inspect_control_inventory(storage.db())?;
    metrics.set_resource_counts([
        inventory.accounts,
        inventory.workers,
        inventory.deployments,
        inventory.routes,
        inventory.kv_namespaces,
        inventory.r2_buckets,
        inventory.d1_databases,
        inventory.do_namespaces,
    ]);
    Ok(())
}

fn load_operation_receipt(data_dir: &DataDir, name: &str) -> Option<serde_json::Value> {
    data_dir
        .read_operation_receipt(name, 64 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}
