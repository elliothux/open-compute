//! Owner-only cleanup of the shared OCD cache, never instance data.

use super::{
    daemon_control::DaemonApi, daemon_lifecycle::DaemonPlan,
    daemon_lifecycle::OfflineInstanceOwner, gateway::GatewayOwner,
};
use open_compute_artifacts::CacheCleanReport;
use open_compute_core::InstanceId;
use open_compute_core::{ErrorCode, PlatformError};
use rustix::fs::{AtFlags, FileType, statat, unlinkat};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::atomic::Ordering;
use tokio::sync::watch;

pub(super) fn clean_global_online(
    plan: &DaemonPlan,
    active: &HashMap<InstanceId, watch::Sender<bool>>,
    api: &DaemonApi,
    gateway: Option<&GatewayOwner>,
    dry_run: bool,
) -> Result<CacheCleanReport, PlatformError> {
    plan.registry.require_unchanged_online(
        plan.scope,
        &plan.records,
        plan.manifest_digest.as_deref(),
    )?;
    if gateway.is_some_and(|owner| owner.pids().0.load(Ordering::Acquire) <= 1) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeUnavailable,
            "shared Gateway orphan recovery is incomplete",
        ));
    }
    let views = api.list()?;
    let mut stopped = Vec::new();
    for record in &plan.records {
        let id = record.instance_id()?;
        if active.contains_key(&id) {
            if !views
                .iter()
                .any(|view| view.instance_id == id.as_str() && view.state == "running")
            {
                return Err(PlatformError::new(
                    ErrorCode::RuntimeUnavailable,
                    "instance recovery or shutdown is incomplete",
                ));
            }
        } else {
            stopped.push(OfflineInstanceOwner::acquire(&plan.root, record, dry_run)?);
        }
    }
    let report = clean_global_cache(&plan.root, dry_run, true);
    drop(stopped);
    report
}

pub(crate) fn clean_global_cache(
    root: &Path,
    dry_run: bool,
    online: bool,
) -> Result<CacheCleanReport, PlatformError> {
    let cache_root = root.join("cache");
    let packages = open_compute_runtime::clean_embedded_runtime_cache(&cache_root, dry_run)?;
    let mut report = CacheCleanReport {
        bytes: packages.bytes,
        entries: packages.entries,
        skipped: packages.skipped,
        failed: packages.failed,
        failure_reason: (packages.failed > 0).then(|| "runtime package removal failed".into()),
    };
    match std::fs::symlink_metadata(&cache_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(_) => return Err(invalid("shared cache directory is inaccessible")),
        Ok(_) => {}
    }
    let directory = open_compute_runtime::open_host_directory_nofollow(&cache_root)?;
    let name = OsStr::new("update-check.json");
    let entry = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(entry) => entry,
        Err(rustix::io::Errno::NOENT) => return Ok(report),
        Err(_) => return Err(invalid("failed to inspect update-check cache")),
    };
    if FileType::from_raw_mode(entry.st_mode) != FileType::RegularFile || online {
        report.skipped += 1;
        return Ok(report);
    }
    if !dry_run && unlinkat(&directory, name, AtFlags::empty()).is_err() {
        report.failed += 1;
        if report.failure_reason.is_none() {
            report.failure_reason = Some("update-check cache removal failed".into());
        }
        return Ok(report);
    }
    report.entries += 1;
    report.bytes = report
        .bytes
        .saturating_add(u64::try_from(entry.st_size).unwrap_or(0));
    Ok(report)
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PathInvalid, message)
}

#[cfg(test)]
#[path = "cache_clean_tests.rs"]
mod tests;
