//! Doctor: default is strictly read-only; `--full` authorizes canary and a temporary runtime.

use crate::capabilities::platform_release_metadata;
use crate::config_load::LoadedConfig;
use crate::metrics::MetricsRegistry;
#[path = "doctor_runtime.rs"]
mod runtime;
#[path = "doctor_workflow.rs"]
mod workflow;
use open_compute_artifacts::{
    ArtifactCache, S3ArtifactClient, resolve_s3_credentials, sample_cache_integrity,
};
use open_compute_core::{ErrorCode, PlatformError, ResourceAvailability};
use open_compute_storage::{
    inspect_control_db, inspect_data_root, inspect_durable_object_storage, inspect_master_key,
    inspect_p23_cross_database, inspect_resources, inspect_scheduler_db, read_operation_receipt,
};
use serde::Serialize;
use std::io::Write;

use std::time::{SystemTime, UNIX_EPOCH};

/// Doctor intensity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoctorMode {
    /// No mutation, no serving child.
    Basic,
    /// S3 canary and temporary workerd compile/start/stop.
    Full,
}

/// Check status token.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Passed.
    Ok,
    /// Non-fatal warning.
    Warning,
    /// Failed.
    Failed,
    /// Not run because a prerequisite failed.
    Skipped,
}

/// One doctor check.
#[derive(Clone, Debug, Serialize)]
pub struct DoctorCheck {
    /// Fixed check name.
    pub name: &'static str,
    /// Status.
    pub status: CheckStatus,
    /// Stable error/readiness code when failed.
    pub code: Option<&'static str>,
    /// Static secret-safe message.
    pub message: &'static str,
    /// Optional bounded non-secret value.
    pub value: Option<String>,
}

/// Versioned doctor report.
#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    /// JSON schema version.
    pub schema_version: u32,
    /// Command name.
    pub command: &'static str,
    /// Aggregate result.
    pub result: &'static str,
    /// Ordered checks.
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// True if any check failed.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Failed)
    }

    /// Write human or JSON output.
    pub fn write(&self, out: &mut impl Write, json: bool) -> Result<(), PlatformError> {
        if json {
            let body = serde_json::to_string(self).map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
            })?;
            writeln!(out, "{body}").map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
            })?;
        } else {
            writeln!(out, "DOCTOR {}", self.result.to_ascii_uppercase()).map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
            })?;
            for check in &self.checks {
                let status = match check.status {
                    CheckStatus::Ok => "ok",
                    CheckStatus::Warning => "warning",
                    CheckStatus::Failed => "failed",
                    CheckStatus::Skipped => "skipped",
                };
                let code = check.code.unwrap_or("-");
                let value = check.value.as_deref().unwrap_or("-");
                writeln!(
                    out,
                    "{} {status} {code} {} {value}",
                    check.name, check.message
                )
                .map_err(|_| {
                    PlatformError::new(ErrorCode::ConfigInvalid, "failed to write command output")
                })?;
            }
        }
        Ok(())
    }
}

/// Run doctor against a loaded config.
pub async fn doctor_report(loaded: &LoadedConfig, mode: DoctorMode) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(ok(
        "config",
        "static configuration parsed",
        Some("ok".into()),
    ));
    match platform_release_metadata(loaded) {
        Ok(metadata) => checks.push(ok(
            "release_identity",
            "release identity and migration registry are internally consistent",
            Some(metadata.release.platform_version),
        )),
        Err(error) => checks.push(failed(
            "release_identity",
            error.code(),
            error.message(),
            None,
        )),
    }
    if MetricsRegistry::validate_limits(&loaded.config.metrics).is_err() {
        checks[0] = failed(
            "config",
            ErrorCode::LimitInvalid,
            "metrics.max_series cannot contain the required fixed series set",
            None,
        );
    }

    let inspect = match inspect_data_root(&loaded.config.storage) {
        Ok(v) => {
            checks.push(ok(
                "data_dir",
                "data directory exists",
                Some("present".into()),
            ));
            if let Some(msg) = v.durability.doctor_warning() {
                checks.push(warning("filesystem", msg, None));
            } else {
                checks.push(ok(
                    "filesystem",
                    "filesystem durability appears local",
                    None,
                ));
            }
            match v.free_bytes {
                Some(bytes) if bytes < loaded.config.storage.free_space_hard_bytes => {
                    checks.push(failed(
                        "free_space",
                        ErrorCode::DiskHardLimit,
                        "data directory free space is below the hard limit",
                        Some(bytes.to_string()),
                    ));
                }
                Some(bytes) if bytes < loaded.config.storage.free_space_soft_bytes => {
                    checks.push(warning(
                        "free_space",
                        "data directory free space is below the soft limit",
                        Some(bytes.to_string()),
                    ));
                }
                Some(bytes) => checks.push(ok(
                    "free_space",
                    "data directory free space is sufficient",
                    Some(bytes.to_string()),
                )),
                None => checks.push(warning(
                    "free_space",
                    "data directory free space could not be measured",
                    None,
                )),
            }
            if v.lock_available {
                checks.push(ok("lock", "data directory lock is available", None));
            } else {
                checks.push(failed(
                    "lock",
                    ErrorCode::DataDirInUse,
                    "data directory exclusive lock is held by another instance",
                    None,
                ));
            }
            Some(v)
        }
        Err(err) => {
            checks.push(failed("data_dir", err.code(), err.message(), None));
            checks.push(skipped("filesystem", "data directory is missing"));
            checks.push(skipped("free_space", "data directory is missing"));
            checks.push(skipped("lock", "data directory is missing"));
            None
        }
    };

    let inspected_key = inspect_master_key(&loaded.config.storage);

    let db_ok = match inspect.as_ref() {
        Some(root) if !root.lock_available => {
            checks.push(skipped(
                "sqlite",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "schema",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "identity",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "resource_catalog",
                "data directory exclusive lock is held by another instance",
            ));
            None
        }
        Some(root) => {
            let db_path = root.root.join("control.sqlite");
            match inspect_control_db(&db_path, loaded.config.storage.sqlite_busy_timeout_ms) {
                Ok((version, identity)) => {
                    checks.push(ok(
                        "sqlite",
                        "control database quick_check passed",
                        Some(version.to_string()),
                    ));
                    checks.push(ok(
                        "schema",
                        "applied migration checksums match this binary",
                        Some(version.to_string()),
                    ));
                    if version != open_compute_storage::migrations::current_schema_version() {
                        let index = checks.len() - 1;
                        checks[index] = failed(
                            "schema",
                            ErrorCode::UpgradeRequired,
                            "control schema requires an offline upgrade before serving",
                            Some(version.to_string()),
                        );
                    }
                    let id = identity.platform_id.to_string();
                    let bounded = bound_value(&id, 36);
                    checks.push(ok(
                        "identity",
                        "stored platform identity is present",
                        Some(bounded),
                    ));
                    match inspect_resources(
                        &db_path,
                        loaded.config.storage.sqlite_busy_timeout_ms,
                        1_000,
                    ) {
                        Ok(resources) if resources.is_empty() => checks.push(ok(
                            "resource_catalog",
                            "resource health catalog is empty",
                            Some("0".to_owned()),
                        )),
                        Ok(resources) => {
                            for resource in resources {
                                let code = resource.availability_code.as_deref().unwrap_or("-");
                                let value = format!(
                                    "{} {} {} {}",
                                    resource.id,
                                    resource.kind,
                                    resource.availability.as_str(),
                                    code
                                );
                                if resource.availability == ResourceAvailability::Healthy {
                                    checks.push(ok(
                                        "resource_catalog",
                                        "resource health probe is healthy",
                                        Some(bound_value(&value, 256)),
                                    ));
                                } else {
                                    checks.push(warning(
                                        "resource_catalog",
                                        "resource health probe requires attention",
                                        Some(bound_value(&value, 256)),
                                    ));
                                }
                            }
                        }
                        Err(err) => {
                            checks.push(failed(
                                "resource_catalog",
                                err.code(),
                                err.message(),
                                None,
                            ));
                        }
                    }
                    Some(identity)
                }
                Err(err) => {
                    checks.push(failed("sqlite", err.code(), err.message(), None));
                    checks.push(skipped("schema", "control database is not inspectable"));
                    checks.push(skipped("identity", "control database is not inspectable"));
                    checks.push(skipped(
                        "resource_catalog",
                        "control database is not inspectable",
                    ));
                    None
                }
            }
        }
        None => {
            checks.push(skipped("sqlite", "data directory is missing"));
            checks.push(skipped("schema", "data directory is missing"));
            checks.push(skipped("identity", "data directory is missing"));
            checks.push(skipped("resource_catalog", "data directory is missing"));
            None
        }
    };

    match inspect.as_ref() {
        Some(root) if root.lock_available => {
            let path = root.root.join("scheduler.sqlite");
            match inspect_scheduler_db(
                &path,
                loaded.config.storage.sqlite_busy_timeout_ms,
                unix_ms(),
            ) {
                Ok(scheduler) => {
                    checks.push(workflow::inspect(loaded, &root.root));
                    let mode_ok = scheduler.journal_mode.eq_ignore_ascii_case("wal")
                        && scheduler.synchronous == 2;
                    checks.push(if mode_ok {
                        ok(
                            "scheduler_sqlite",
                            "scheduler database integrity, WAL, and FULL sync passed",
                            Some(scheduler.schema_version.to_string()),
                        )
                    } else {
                        failed(
                            "scheduler_sqlite",
                            ErrorCode::SchedulerCorrupt,
                            "scheduler database SQLite mode is invalid",
                            None,
                        )
                    });
                    checks.push(if scheduler.invalid_rows == 0 {
                        ok(
                            "scheduler_invariants",
                            "scheduler claim and token invariants passed",
                            Some("0".to_owned()),
                        )
                    } else {
                        failed(
                            "scheduler_invariants",
                            ErrorCode::SchedulerCorrupt,
                            "scheduler claim or token invariant failed",
                            Some(scheduler.invalid_rows.to_string()),
                        )
                    });
                    let summary = scheduler.summary;
                    checks.push(ok(
                        "scheduler_summary",
                        "scheduler bounded state summary inspected",
                        Some(format!(
                            "scheduled={} claimed={} discarding={} expired={}",
                            summary.scheduled,
                            summary.claimed,
                            summary.discarding,
                            summary.expired_claims
                        )),
                    ));
                    let consumers = scheduler.queue_consumers;
                    checks.push(
                        if consumers.orphan_batches == 0 && consumers.unavailable_dlq_targets == 0 {
                            ok(
                                "queue_consumer_invariants",
                                "Queue consumer batches and DLQ targets are consistent",
                                Some(format!(
                                    "consumers={} batches={} claimed={} dlq_pending={}",
                                    consumers.consumers,
                                    consumers.claimed_batches,
                                    consumers.claimed_messages,
                                    consumers.dlq_pending
                                )),
                            )
                        } else {
                            failed(
                                "queue_consumer_invariants",
                                ErrorCode::SchedulerCorrupt,
                                "Queue consumer batch or DLQ target invariant failed",
                                Some(format!(
                                    "orphan_batches={} unavailable_dlq_targets={}",
                                    consumers.orphan_batches, consumers.unavailable_dlq_targets
                                )),
                            )
                        },
                    );
                    let cron = scheduler.cron;
                    checks.push(
                        if cron.parser_version_mismatches == 0 && cron.invalid_next_fire == 0 {
                            ok(
                                "cron_invariants",
                                "Cron parser versions and next-fire projections are valid",
                                Some(format!(
                                    "schedules={} runs={} ready={} claimed={}",
                                    cron.schedules, cron.runs, cron.ready_runs, cron.claimed_runs
                                )),
                            )
                        } else {
                            failed(
                                "cron_invariants",
                                ErrorCode::SchedulerCorrupt,
                                "Cron parser version or next-fire invariant failed",
                                Some(format!(
                                    "parser_mismatch={} invalid_next_fire={}",
                                    cron.parser_version_mismatches, cron.invalid_next_fire
                                )),
                            )
                        },
                    );
                    match inspect_p23_cross_database(
                        &root.root.join("control.sqlite"),
                        &path,
                        loaded.config.storage.sqlite_busy_timeout_ms,
                    ) {
                        Ok(cross)
                            if cross.queue_consumer_projection_mismatches == 0
                                && cross.cron_projection_mismatches == 0
                                && cross.deployment_referrer_mismatches == 0 =>
                        {
                            checks.push(ok(
                                "p2_3_cross_database",
                                "Queue/Cron projections and deployment referrers match control authority",
                                Some("0".to_owned()),
                            ));
                        }
                        Ok(cross) => checks.push(failed(
                            "p2_3_cross_database",
                            ErrorCode::SchedulerCorrupt,
                            "Queue/Cron projection or deployment-referrer authority diverged",
                            Some(format!(
                                "queue={} cron={} referrers={}",
                                cross.queue_consumer_projection_mismatches,
                                cross.cron_projection_mismatches,
                                cross.deployment_referrer_mismatches,
                            )),
                        )),
                        Err(error) => checks.push(failed(
                            "p2_3_cross_database",
                            error.code(),
                            error.message(),
                            None,
                        )),
                    }
                }
                Err(error) => {
                    checks.push(failed(
                        "scheduler_sqlite",
                        error.code(),
                        error.message(),
                        None,
                    ));
                    checks.push(skipped(
                        "scheduler_invariants",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "scheduler_summary",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "queue_consumer_invariants",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "cron_invariants",
                        "scheduler database is not inspectable",
                    ));
                    checks.push(skipped(
                        "p2_3_cross_database",
                        "scheduler database is not inspectable",
                    ));
                }
            }
        }
        Some(_) => {
            checks.push(skipped(
                "scheduler_sqlite",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "scheduler_invariants",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "scheduler_summary",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "queue_consumer_invariants",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "cron_invariants",
                "data directory exclusive lock is held by another instance",
            ));
            checks.push(skipped(
                "p2_3_cross_database",
                "data directory exclusive lock is held by another instance",
            ));
        }
        None => {
            checks.push(skipped("scheduler_sqlite", "data directory is missing"));
            checks.push(skipped("scheduler_invariants", "data directory is missing"));
            checks.push(skipped("scheduler_summary", "data directory is missing"));
            checks.push(skipped(
                "queue_consumer_invariants",
                "data directory is missing",
            ));
            checks.push(skipped("cron_invariants", "data directory is missing"));
            checks.push(skipped("p2_3_cross_database", "data directory is missing"));
        }
    }

    checks.push(ok(
        "scheduler_policy",
        "scheduler lease exceeds dispatch timeout and guard",
        Some(loaded.config.scheduler.claim_lease_ms.to_string()),
    ));

    match (&inspected_key, &db_ok) {
        (Ok(key), Some(identity)) if key.fingerprint() != identity.master_key_id => {
            checks.push(failed(
                "master_key",
                ErrorCode::MasterKeyMismatch,
                "master key fingerprint does not match stored identity",
                Some(bound_value(key.fingerprint(), 16)),
            ));
        }
        (Ok(key), _) => checks.push(ok(
            "master_key",
            "master key fingerprint resolved",
            Some(bound_value(key.fingerprint(), 16)),
        )),
        (Err(err), _) => checks.push(failed("master_key", err.code(), err.message(), None)),
    }

    for receipt in [
        "last-snapshot.json",
        "last-restore.json",
        "last-upgrade.json",
    ] {
        checks.push(operation_receipt_check(loaded, receipt));
    }

    let hold_local = inspect.as_ref().is_some_and(|root| root.lock_available);
    let cache_dir = loaded
        .config
        .storage
        .data_dir
        .join("cache")
        .join("artifacts");
    let cache_meta = std::fs::symlink_metadata(&cache_dir);
    if !hold_local && inspect.is_some() {
        checks.push(skipped(
            "cache_integrity",
            "data directory exclusive lock is held by another instance",
        ));
    } else if cache_meta
        .as_ref()
        .is_ok_and(|m| !m.file_type().is_symlink() && m.file_type().is_dir())
    {
        match ArtifactCache::inspect_existing(cache_dir) {
            Ok(cache) => match sample_cache_integrity(&cache) {
                Ok(sample) if sample.corrupt => checks.push(failed(
                    "cache_integrity",
                    ErrorCode::CacheEntryCorrupt,
                    "cache entry failed integrity checks",
                    Some(sample.entries.to_string()),
                )),
                Ok(sample) => checks.push(ok(
                    "cache_integrity",
                    "cache integrity sample passed",
                    Some(sample.entries.to_string()),
                )),
                Err(err) => checks.push(failed("cache_integrity", err.code(), err.message(), None)),
            },
            Err(err) => checks.push(failed("cache_integrity", err.code(), err.message(), None)),
        }
    } else {
        checks.push(failed(
            "cache_integrity",
            ErrorCode::PathInvalid,
            "artifact cache directory is missing",
            None,
        ));
    }

    let runtime_version = runtime::inspect(&mut checks, loaded);

    match (inspect.as_ref(), db_ok.as_ref(), runtime_version.as_ref()) {
        (Some(root), Some(identity), Some(version)) => match inspect_durable_object_storage(
            &root.root,
            &identity.platform_id.to_string(),
            version,
        ) {
            Ok(_) => checks.push(ok(
                "do_storage",
                "Durable Object localDisk marker and filesystem passed",
                Some("format_v1".to_owned()),
            )),
            Err(error) => checks.push(failed("do_storage", error.code(), error.message(), None)),
        },
        _ => checks.push(skipped(
            "do_storage",
            "data identity and verified workerd are prerequisites",
        )),
    }

    let s3_client = match resolve_s3_credentials(&loaded.config.s3) {
        Ok(creds) => match S3ArtifactClient::connect(
            &loaded.config.s3,
            &creds,
            loaded.config.cache.max_artifact_bytes,
        ) {
            Ok(client) => Some(client),
            Err(err) => {
                checks.push(failed("s3_connectivity", err.code(), err.message(), None));
                None
            }
        },
        Err(err) => {
            checks.push(failed("s3_connectivity", err.code(), err.message(), None));
            None
        }
    };

    if let Some(client) = s3_client.as_ref() {
        match client.probe_connectivity().await {
            Ok(()) => checks.push(ok(
                "s3_connectivity",
                "signed s3 connectivity probe succeeded",
                None,
            )),
            Err(err) => checks.push(failed("s3_connectivity", err.code(), err.message(), None)),
        }
    }

    if mode == DoctorMode::Full
        && let Some(root) = inspect.as_ref().filter(|root| root.holds_inspect_lock())
    {
        runtime::run_full_extras(
            &mut checks,
            loaded,
            root,
            s3_client.as_ref(),
            db_ok.as_ref().map(|i| i.platform_id),
        )
        .await;
    } else if mode == DoctorMode::Full {
        let reason = if inspect.is_some() {
            "data directory exclusive lock is held by another instance"
        } else {
            "data directory is missing"
        };
        checks.push(skipped("s3_canary", reason));
        checks.push(skipped("r2_canary", reason));
        checks.push(skipped("runtime_cycle", reason));
    } else {
        checks.push(skipped(
            "s3_canary",
            "full doctor is required for the s3 canary",
        ));
        checks.push(skipped(
            "r2_canary",
            "full doctor is required for the R2 capability canary",
        ));
        checks.push(skipped(
            "runtime_cycle",
            "full doctor is required for a temporary workerd cycle",
        ));
    }

    checks.sort_by_key(|c| c.name);
    let result = if checks.iter().any(|c| c.status == CheckStatus::Failed) {
        "failed"
    } else {
        "ok"
    };
    DoctorReport {
        schema_version: 1,
        command: "doctor",
        result,
        checks,
    }
}

fn operation_receipt_check(loaded: &LoadedConfig, name: &'static str) -> DoctorCheck {
    let path = loaded.config.storage.data_dir.join("operations").join(name);
    let check_name = match name {
        "last-snapshot.json" => "last_snapshot_receipt",
        "last-restore.json" => "last_restore_receipt",
        "last-upgrade.json" => "last_upgrade_receipt",
        _ => "operation_receipt",
    };
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return warning(check_name, "operation receipt has not been recorded", None);
        }
        Err(_) => return warning(check_name, "operation receipt cannot be inspected", None),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > 64 * 1024
    {
        return warning(check_name, "operation receipt is invalid", None);
    }
    match read_operation_receipt(&loaded.config.storage.data_dir, name, 64 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    {
        Some(value) => ok(
            check_name,
            "operation receipt is valid JSON",
            value
                .get("completed_at_ms")
                .or_else(|| value.get("restored_at_ms"))
                .or_else(|| value.get("created_at_ms"))
                .and_then(serde_json::Value::as_i64)
                .map(|value| value.to_string()),
        ),
        None => warning(check_name, "operation receipt is invalid", None),
    }
}

fn ok(name: &'static str, message: &'static str, value: Option<String>) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Ok,
        code: None,
        message,
        value,
    }
}

fn warning(name: &'static str, message: &'static str, value: Option<String>) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Warning,
        code: None,
        message,
        value,
    }
}

fn failed(
    name: &'static str,
    code: ErrorCode,
    message: &'static str,
    value: Option<String>,
) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Failed,
        code: Some(code.as_str()),
        message,
        value,
    }
}

fn skipped(name: &'static str, message: &'static str) -> DoctorCheck {
    DoctorCheck {
        name,
        status: CheckStatus::Skipped,
        code: None,
        message,
        value: None,
    }
}

fn bound_value(s: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        let mut buf = [0u8; 4];
        let encoded = ch.encode_utf8(&mut buf);
        if out.len().saturating_add(encoded.len()) > max_bytes {
            break;
        }
        out.push(ch);
    }
    out
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
