//! Doctor: default is strictly read-only; `--full` authorizes canary and a temporary runtime.

use crate::capabilities::platform_release_metadata;
use crate::config_load::LoadedConfig;
use crate::metrics::MetricsRegistry;
use crate::object_storage::connect_object_backend;
use crate::{ai_tokenizer::AiTokenizerRegistry, auth::resolve_admin_auth};
#[path = "doctor_runtime.rs"]
mod runtime;
#[path = "doctor_workflow.rs"]
mod workflow;
use open_compute_artifacts::{ArtifactCache, ObjectBackend, probe_object_storage};
use open_compute_core::{
    AiAuthConfig, AiConfig, ErrorCode, ObjectStorageConfig, ObjectStorageKind, PlatformError,
    ResourceAvailability,
};
use open_compute_storage::{
    inspect_control_db, inspect_data_root, inspect_durable_object_storage, inspect_master_key,
    inspect_p23_cross_database, inspect_resources, inspect_scheduler_db, read_operation_receipt,
};
use serde::Serialize;
use std::io::Write;

/// Doctor intensity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoctorMode {
    /// No mutation, no serving child.
    Basic,
    /// Object-storage canary and temporary workerd compile/start/stop.
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

mod report;

pub use report::doctor_report;

fn inspect_ai_provider_config(config: &AiConfig) -> Result<String, PlatformError> {
    config.validate()?;
    for backend in config.backends.values() {
        match &backend.auth {
            AiAuthConfig::Bearer { secret } | AiAuthConfig::Header { secret, .. } => {
                let _ = resolve_admin_auth(secret)?;
            }
            AiAuthConfig::None => {}
        }
    }
    let _ = AiTokenizerRegistry::load(config)?;
    for alias in config.embedding_models.keys() {
        let _ = config.resolve_embedding_model(Some(alias))?;
        let _ = config.resolve_tokenizer(Some(alias))?;
    }
    Ok(format!(
        "backends={} embedding_profiles={} embedding_models={} generation_models={}",
        config.backends.len(),
        config.embedding_profiles.len(),
        config.embedding_models.len(),
        config.generation_models.len(),
    ))
}

fn operation_receipt_check(loaded: &LoadedConfig, name: &'static str) -> DoctorCheck {
    let path = loaded.config.data.path.join("operations").join(name);
    let check_name = match name {
        "last-snapshot.json" => "last_snapshot_receipt",
        "last-restore.json" => "last_restore_receipt",
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
    match read_operation_receipt(&loaded.config.data.path, name, 64 * 1024)
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

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
