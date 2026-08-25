//! Doctor: default is strictly read-only; `--full` authorizes canary and a temporary runtime.

use crate::config_load::LoadedConfig;
use crate::metrics::MetricsRegistry;
use open_compute_artifacts::{
    ArtifactCache, S3ArtifactClient, preflight_r2, preflight_s3, resolve_s3_credentials,
    sample_cache_integrity,
};
use open_compute_core::ids::{PlatformId, StartupId};
use open_compute_core::{ErrorCode, PlatformError, Redactor, ResourceAvailability, SystemClock};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, OsJitter, PlatformReleaseMeta,
    StaticConfigCompiler, SupervisorState, WorkerdSupervisor, WorkerdSupervisorOptions,
    verify_runtime_binary,
};
use open_compute_storage::{
    inspect_control_db, inspect_data_root, inspect_durable_object_storage, inspect_master_key,
    inspect_resources,
};
use serde::Serialize;
use std::io::Write;

use std::sync::Arc;
use std::time::Duration;

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

    let verified_runtime = match verify_runtime_binary(
        &loaded.config.runtime.lock_file,
        &loaded.config.runtime.binary,
        Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
        &Redactor::new(),
    )
    .await
    {
        Ok(rt) => {
            let ver = rt.version_output().to_owned();
            checks.push(ok(
                "runtime_binary",
                "workerd binary hash and version match the lock",
                Some(bound_value(&ver, 32)),
            ));
            Some(rt)
        }
        Err(err) => {
            checks.push(failed("runtime_binary", err.code(), err.message(), None));
            None
        }
    };

    match (inspect.as_ref(), db_ok.as_ref(), verified_runtime.as_ref()) {
        (Some(root), Some(identity), Some(runtime)) => match inspect_durable_object_storage(
            &root.root,
            &identity.platform_id.to_string(),
            runtime.version_output(),
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

    if mode == DoctorMode::Full && hold_local {
        run_full_extras(
            &mut checks,
            loaded,
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

async fn run_full_extras(
    checks: &mut Vec<DoctorCheck>,
    loaded: &LoadedConfig,
    client: Option<&S3ArtifactClient>,
    platform_id: Option<PlatformId>,
) {
    match (client, platform_id) {
        (Some(client), Some(platform_id)) => {
            match preflight_s3(client, platform_id, StartupId::generate()).await {
                Ok(_) => checks.push(ok("s3_canary", "s3 preflight canary succeeded", None)),
                Err(err) => checks.push(failed("s3_canary", err.code(), err.message(), None)),
            }
            match preflight_r2(client, platform_id, StartupId::generate()).await {
                Ok(outcome) => checks.push(ok(
                    "r2_canary",
                    "R2 provider capability preflight succeeded",
                    Some(if outcome.multi_delete {
                        "multi_delete".to_owned()
                    } else {
                        "single_delete_fallback".to_owned()
                    }),
                )),
                Err(err) => checks.push(failed("r2_canary", err.code(), err.message(), None)),
            }
        }
        _ => checks.push(skipped(
            "s3_canary",
            "s3 canary requires connectivity and stored identity",
        )),
    }
    if client.is_none() || platform_id.is_none() {
        checks.push(skipped(
            "r2_canary",
            "R2 canary requires connectivity and stored identity",
        ));
    }

    let runtime = verify_runtime_binary(
        &loaded.config.runtime.lock_file,
        &loaded.config.runtime.binary,
        Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
        &Redactor::new(),
    )
    .await;
    let Ok(runtime) = runtime else {
        checks.push(skipped(
            "runtime_cycle",
            "runtime binary verification is a prerequisite",
        ));
        return;
    };
    let data_runtime = loaded.config.storage.data_dir.join("runtime");
    if !data_runtime.is_dir() || !loaded.config.runtime.assets_dir.is_dir() {
        checks.push(skipped(
            "runtime_cycle",
            "runtime data and assets directories are required",
        ));
        return;
    }
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        loaded.config.runtime.lock_file.clone(),
        loaded.config.runtime.assets_dir.clone(),
        data_runtime,
        PlatformReleaseMeta {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Duration::from_millis(loaded.config.runtime.startup_timeout_ms),
        Redactor::new(),
    )
    .with_durable_objects_config(loaded.config.durable_objects.clone());
    let Ok(runtime_source) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary runtime-source listener could not be bound",
            None,
        ));
        return;
    };
    let Ok(runtime_source_addr) = runtime_source.local_addr() else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary runtime-source listener address is unavailable",
            None,
        ));
        return;
    };
    let Ok(binding_backend) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary binding-backend listener could not be bound",
            None,
        ));
        return;
    };
    let Ok(binding_backend_addr) = binding_backend.local_addr() else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeUnavailable,
            "temporary binding-backend listener address is unavailable",
            None,
        ));
        return;
    };
    let runtime_external =
        match ExternalServiceAddress::loopback("runtime-source", runtime_source_addr) {
            Ok(external) => external,
            Err(err) => {
                checks.push(failed("runtime_cycle", err.code(), err.message(), None));
                return;
            }
        };
    let binding_external =
        match ExternalServiceAddress::loopback("binding-backend", binding_backend_addr) {
            Ok(external) => external,
            Err(err) => {
                checks.push(failed("runtime_cycle", err.code(), err.message(), None));
                return;
            }
        };
    let Some(platform_id) = platform_id else {
        checks.push(skipped(
            "runtime_cycle",
            "temporary runtime requires stored platform identity",
        ));
        return;
    };
    let do_storage = match inspect_durable_object_storage(
        &loaded.config.storage.data_dir,
        &platform_id.to_string(),
        runtime.version_output(),
    ) {
        Ok(path) => path,
        Err(error) => {
            checks.push(failed("runtime_cycle", error.code(), error.message(), None));
            return;
        }
    };
    let directory = match DirectoryServicePath::local("do-storage", &do_storage) {
        Ok(directory) => directory,
        Err(error) => {
            checks.push(failed("runtime_cycle", error.code(), error.message(), None));
            return;
        }
    };
    let supervisor = WorkerdSupervisor::new_with_services_and_auth(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: loaded.config.runtime.clone(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: Redactor::new(),
            lease_path: None,
        },
        vec![runtime_external, binding_external],
        vec![directory],
        Vec::new(),
    );
    supervisor.start();
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(loaded.config.runtime.startup_timeout_ms);
    let mut rx = supervisor.subscribe();
    let mut ready = false;
    loop {
        if rx.borrow().state == SupervisorState::Running {
            ready = true;
            break;
        }
        if tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
    supervisor.begin_drain();
    supervisor.shutdown().await;
    if ready {
        checks.push(ok(
            "runtime_cycle",
            "temporary workerd compile start probe stop succeeded",
            None,
        ));
    } else {
        checks.push(failed(
            "runtime_cycle",
            ErrorCode::RuntimeExitedBeforeReady,
            "temporary workerd did not become ready",
            None,
        ));
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

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
