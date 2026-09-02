//! Allowlisted, secret-scanned local P1 support bundle generation.

use crate::auth::resolve_admin_auth;
use crate::capabilities::platform_release_metadata;
use crate::config_load::LoadedConfig;
use crate::doctor::{DoctorMode, doctor_report};
use crate::metrics::MetricsRegistry;
use base64::Engine as _;
use open_compute_artifacts::resolve_s3_credentials;
use open_compute_core::{
    AiAuthConfig, BindingKind, ErrorCode, PlatformError, PlatformStatus, ResourceAvailability,
};
use open_compute_storage::{
    AI_SEARCH_SCHEMA_VERSION, VECTORIZE_SCHEMA_VERSION, inspect_control_db, inspect_master_key,
    inspect_operator_event_count, inspect_resources, inspect_scheduler_db, read_operation_receipt,
};
use rustix::fs::{Mode, OFlags};
use serde::Serialize;
use sha2::Digest as _;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path};

const MAX_RECEIPT_BYTES: u64 = 64 * 1024;

/// Secret-free support bundle completion result.
#[derive(Clone, Debug, Serialize)]
pub struct SupportBundleResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Absolute output path.
    pub output: String,
    /// Final archive byte size.
    pub bytes: u64,
    /// SHA-256 of the complete archive.
    pub sha256: String,
    /// Number of allowlisted archive entries.
    pub entries: u32,
}

/// Generate a bounded local tar containing only explicit secret-free diagnostics.
pub async fn create_support_bundle(
    loaded: &LoadedConfig,
    output: &Path,
) -> Result<SupportBundleResult, PlatformError> {
    validate_output(output)?;
    let release = platform_release_metadata(loaded)?;
    let mut doctor = doctor_report(loaded, DoctorMode::Basic).await;
    for check in &mut doctor.checks {
        if check.name == "resource_catalog"
            && let Some(value) = check.value.as_deref()
        {
            check.value = Some(format!("sha256:{}", hash_identifier(value)));
        }
    }
    let metrics = MetricsRegistry::new(
        &loaded.config.metrics,
        env!("CARGO_PKG_VERSION"),
        &release.release.workerd_version,
    )?
    .render(&PlatformStatus::starting());
    let mut entries = vec![
        json_entry("release.json", &release)?,
        json_entry("config-policy.json", &redacted_policy(loaded))?,
        json_entry("doctor.json", &doctor)?,
        ("metrics.prom".to_owned(), metrics.into_bytes()),
        json_entry("schema.json", &schema_summary(loaded))?,
        json_entry("files.json", &file_summary(loaded))?,
        json_entry("operator-events.json", &operator_event_summary(loaded))?,
        json_entry("search.json", &search_summary(loaded)?)?,
    ];
    entries.extend(receipt_entries(loaded)?);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let max_bytes = usize::try_from(loaded.config.hardening.max_support_bundle_bytes)
        .map_err(|_| bundle_invalid())?;
    let mut archive = Vec::new();
    for (name, bytes) in &entries {
        append_tar_entry(&mut archive, name, bytes, max_bytes)?;
    }
    append_bounded(&mut archive, &[0_u8; 1024], max_bytes)?;
    let needles = secret_needles(loaded)?;
    scan_secrets(&archive, &needles)?;
    let fd = rustix::fs::open(
        output,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| bundle_invalid())?;
    let mut file = File::from(fd);
    let write_result = file.write_all(&archive).and_then(|()| file.sync_all());
    drop(file);
    if write_result.is_err() {
        let _ = std::fs::remove_file(output);
        return Err(bundle_invalid());
    }
    let mut persisted = Vec::with_capacity(archive.len());
    File::open(output)
        .and_then(|mut file| file.read_to_end(&mut persisted))
        .map_err(|_| bundle_invalid())?;
    if persisted != archive {
        let _ = std::fs::remove_file(output);
        return Err(bundle_invalid());
    }
    scan_secrets(&persisted, &needles)?;
    let digest = sha2::Sha256::digest(&persisted);
    Ok(SupportBundleResult {
        schema_version: 1,
        output: output.to_string_lossy().into_owned(),
        bytes: archive.len() as u64,
        sha256: hex::encode(digest),
        entries: u32::try_from(entries.len()).map_err(|_| bundle_invalid())?,
    })
}

fn redacted_policy(loaded: &LoadedConfig) -> serde_json::Value {
    let config = &loaded.config;
    serde_json::json!({
        "schema_version": 1,
        "server": {
            "public_bind": config.server.public_bind,
            "admin_bind": config.server.admin_bind,
            "admin_auth_configured": true,
        },
        "storage": {
            "data_dir": config.storage.data_dir,
            "sqlite_busy_timeout_ms": config.storage.sqlite_busy_timeout_ms,
            "free_space_soft_bytes": config.storage.free_space_soft_bytes,
            "free_space_hard_bytes": config.storage.free_space_hard_bytes,
            "master_key_source": if config.storage.master_key_env.is_some() { "env_or_file" } else { "file" },
        },
        "s3": {
            "endpoint": config.s3.endpoint,
            "region": config.s3.region,
            "bucket": config.s3.bucket,
            "system_prefix": config.s3.prefix,
            "r2_prefix": config.s3.r2_prefix,
            "credential_source_configured": true,
        },
        "limits": {
            "hardening": config.hardening,
            "workers": config.workers,
            "kv": config.kv,
            "r2": config.r2,
            "d1": config.d1,
            "durable_objects": config.durable_objects,
            "scheduler": config.scheduler,
        }
    })
}

fn schema_summary(loaded: &LoadedConfig) -> serde_json::Value {
    let control = inspect_control_db(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )
    .ok()
    .map(|(version, identity)| {
        serde_json::json!({
            "version": version,
            "platform_id_hash": hash_identifier(&identity.platform_id.to_string()),
        })
    });
    let scheduler = inspect_scheduler_db(
        &loaded.config.storage.data_dir.join("scheduler.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
        unix_ms(),
    )
    .ok()
    .map(|value| {
        serde_json::json!({
            "version": value.schema_version,
            "invalid_rows": value.invalid_rows,
            "summary": value.summary,
        })
    });
    serde_json::json!({
        "schema_version": 1,
        "control": control,
        "scheduler": scheduler,
    })
}

fn file_summary(loaded: &LoadedConfig) -> serde_json::Value {
    let root = &loaded.config.storage.data_dir;
    let mut values = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten().take(128) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if matches!(
                name.as_str(),
                "keys" | "do" | "kv" | "d1" | "vectorize" | "ai-search"
            ) {
                values.push(serde_json::json!({"name": name, "type": "redacted_tree"}));
                continue;
            }
            if let Ok(metadata) = std::fs::symlink_metadata(entry.path()) {
                values.push(serde_json::json!({
                    "name": name,
                    "type": if metadata.file_type().is_symlink() { "symlink" } else if metadata.is_dir() { "directory" } else if metadata.is_file() { "file" } else { "other" },
                    "mode": metadata.permissions().mode() & 0o777,
                    "size": if metadata.is_file() { Some(metadata.len()) } else { None },
                }));
            }
        }
    }
    serde_json::json!({"schema_version": 1, "entries": values})
}

fn operator_event_summary(loaded: &LoadedConfig) -> serde_json::Value {
    let path = loaded.config.storage.data_dir.join("control.sqlite");
    let counts =
        inspect_operator_event_count(&path, loaded.config.storage.sqlite_busy_timeout_ms).ok();
    serde_json::json!({"schema_version": 1, "bounded_total": counts})
}

pub(crate) fn search_summary(loaded: &LoadedConfig) -> Result<serde_json::Value, PlatformError> {
    let path = loaded.config.storage.data_dir.join("control.sqlite");
    let resources = inspect_resources(&path, loaded.config.storage.sqlite_busy_timeout_ms, 10_000)?;
    let counts = |kind| {
        let selected = resources.iter().filter(|resource| resource.kind == kind);
        let total = selected.clone().count();
        let healthy = selected
            .clone()
            .filter(|resource| resource.availability == ResourceAvailability::Healthy)
            .count();
        let degraded = selected
            .clone()
            .filter(|resource| resource.availability == ResourceAvailability::Degraded)
            .count();
        let unavailable = selected
            .filter(|resource| resource.availability == ResourceAvailability::Unavailable)
            .count();
        serde_json::json!({
            "total": total,
            "healthy": healthy,
            "degraded": degraded,
            "unavailable": unavailable,
        })
    };
    Ok(serde_json::json!({
        "schema_version": 1,
        "resource_count_bound": 10_000,
        "resources": {
            "vectorize_index": counts(BindingKind::VectorizeIndex),
            "ai_search_namespace": counts(BindingKind::AiSearchNamespace),
            "ai_search_instance": counts(BindingKind::AiSearchInstance),
        },
        "contracts": {
            "vectorize_schema_version": VECTORIZE_SCHEMA_VERSION,
            "ai_search_schema_version": AI_SEARCH_SCHEMA_VERSION,
            "ai_provider_contract_sha256": ai_provider_contract_sha256(loaded)?,
        }
    }))
}

fn ai_provider_contract_sha256(loaded: &LoadedConfig) -> Result<String, PlatformError> {
    let config = &loaded.config.ai;
    config.validate()?;
    let mut digest = sha2::Sha256::new();
    digest.update(b"open-compute/ai-provider-catalog/v1\0");
    for value in [
        u64::from(config.max_provider_in_flight),
        u64::from(config.max_embedding_inputs_per_batch),
        config.max_embedding_request_bytes,
        config.max_embedding_response_bytes,
        config.provider_timeout_ms,
        config.query_timeout_ms,
    ] {
        digest_part(&mut digest, &value.to_be_bytes());
    }
    digest_part(
        &mut digest,
        config
            .default_embedding_model
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    digest_part(
        &mut digest,
        config
            .default_generation_model
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    for (name, provider) in &config.providers {
        digest_part(&mut digest, name.as_bytes());
        digest_part(&mut digest, provider.base_url.as_bytes());
        digest_part(&mut digest, provider.auth.kind_token().as_bytes());
    }
    for alias in config.embedding_models.keys() {
        let contract = config.resolve_embedding_model(Some(alias))?;
        digest_part(&mut digest, contract.contract_sha256.as_bytes());
    }
    for (alias, model) in &config.generation_models {
        digest_part(&mut digest, alias.as_bytes());
        let bytes = serde_json::to_vec(model).map_err(|_| bundle_invalid())?;
        digest_part(&mut digest, &bytes);
    }
    Ok(hex::encode(digest.finalize()))
}

fn digest_part(digest: &mut sha2::Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

fn receipt_entries(loaded: &LoadedConfig) -> Result<Vec<(String, Vec<u8>)>, PlatformError> {
    let mut values = Vec::new();
    for name in ["last-snapshot.json", "last-restore.json"] {
        let path = loaded.config.storage.data_dir.join("operations").join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || metadata.len() > MAX_RECEIPT_BYTES
        {
            continue;
        }
        let bytes =
            read_operation_receipt(&loaded.config.storage.data_dir, name, MAX_RECEIPT_BYTES)
                .map_err(|_| bundle_invalid())?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| bundle_invalid())?;
        values.push((
            format!("receipts/{name}"),
            serde_json::to_vec(&value).map_err(|_| bundle_invalid())?,
        ));
    }
    Ok(values)
}

fn secret_needles(loaded: &LoadedConfig) -> Result<Vec<Vec<u8>>, PlatformError> {
    let key = inspect_master_key(&loaded.config.storage)?;
    let credentials = resolve_s3_credentials(&loaded.config.s3)?;
    let mut values = vec![
        key.bytes().expose().to_vec(),
        base64::engine::general_purpose::STANDARD
            .encode(key.bytes().expose())
            .into_bytes(),
        credentials.access_key_id().expose().as_bytes().to_vec(),
        credentials.secret_access_key().expose().as_bytes().to_vec(),
    ];
    values.push(
        resolve_admin_auth(&loaded.config.server.admin_auth)?
            .expose()
            .as_bytes()
            .to_vec(),
    );
    for provider in loaded.config.ai.providers.values() {
        if let AiAuthConfig::Bearer { secret } = &provider.auth {
            values.push(resolve_admin_auth(secret)?.expose().as_bytes().to_vec());
        }
    }
    values.retain(|value| value.len() >= 4);
    Ok(values)
}

fn scan_secrets(bytes: &[u8], needles: &[Vec<u8>]) -> Result<(), PlatformError> {
    if needles
        .iter()
        .any(|needle| bytes.windows(needle.len()).any(|window| window == needle))
    {
        return Err(PlatformError::new(
            ErrorCode::SupportBundleInvalid,
            "support bundle secret canary matched",
        ));
    }
    Ok(())
}

fn json_entry(name: &str, value: &impl Serialize) -> Result<(String, Vec<u8>), PlatformError> {
    Ok((
        name.to_owned(),
        serde_json::to_vec(value).map_err(|_| bundle_invalid())?,
    ))
}

fn append_tar_entry(
    archive: &mut Vec<u8>,
    name: &str,
    contents: &[u8],
    max_bytes: usize,
) -> Result<(), PlatformError> {
    if name.is_empty() || name.len() > 100 || name.starts_with('/') || name.contains("..") {
        return Err(bundle_invalid());
    }
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], 0o600)?;
    write_octal(&mut header[108..116], 0)?;
    write_octal(&mut header[116..124], 0)?;
    write_octal(&mut header[124..136], contents.len() as u64)?;
    write_octal(&mut header[136..148], 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let checksum_bytes = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(checksum_bytes.as_bytes());
    append_bounded(archive, &header, max_bytes)?;
    append_bounded(archive, contents, max_bytes)?;
    let padding = (512 - contents.len() % 512) % 512;
    append_bounded(archive, &vec![0_u8; padding], max_bytes)
}

fn write_octal(field: &mut [u8], value: u64) -> Result<(), PlatformError> {
    let width = field.len().checked_sub(1).ok_or_else(bundle_invalid)?;
    let value = format!("{value:0width$o}");
    if value.len() != width {
        return Err(bundle_invalid());
    }
    field[..width].copy_from_slice(value.as_bytes());
    field[width] = 0;
    Ok(())
}

fn append_bounded(output: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), PlatformError> {
    if output.len().saturating_add(bytes.len()) > max {
        return Err(PlatformError::new(
            ErrorCode::SupportBundleInvalid,
            "support bundle exceeds the configured limit",
        ));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn validate_output(output: &Path) -> Result<(), PlatformError> {
    if !output.is_absolute()
        || output
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || output.exists()
        || std::fs::symlink_metadata(output).is_ok()
    {
        return Err(bundle_invalid());
    }
    let parent = output.parent().ok_or_else(bundle_invalid)?;
    if std::fs::canonicalize(parent).map_err(|_| bundle_invalid())? != parent {
        return Err(bundle_invalid());
    }
    Ok(())
}

fn hash_identifier(value: &str) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(value.as_bytes()))
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn bundle_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SupportBundleInvalid,
        "support bundle operation failed validation",
    )
}
