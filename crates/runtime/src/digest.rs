//! Deterministic static-config input digest.

use crate::fsutil::{
    MAX_ASSETS_TOTAL_BYTES, hex_sha256, list_files_sorted, open_dir_nofollow,
    read_regular_nofollow, reject_symlink_escape,
};
use crate::lock::RuntimeLock;
use crate::verify::VerifiedRuntime;
use open_compute_core::{ErrorCode, PlatformError, SecretString};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const DIGEST_TAG: &[u8] = b"open-compute-static-config-v1\0";
pub(crate) const TOKEN_PLACEHOLDER: &str = "__OPEN_COMPUTE_INTERNAL_TOKEN__";
pub(crate) const TOKEN_HEX_LEN: usize = 64;

/// Platform release identity mixed into the config digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformReleaseMeta {
    /// Platform binary/release version string.
    pub version: String,
}

/// Inputs hashed into the compiled-config cache key.
#[derive(Debug)]
pub(crate) struct DigestInputs<'a> {
    /// Packaged Cap'n Proto template.
    pub config_template: &'a [u8],
    /// Sorted (relative path, bytes) system workers.
    pub workers: &'a [(String, Vec<u8>)],
    /// Raw lock file bytes.
    pub lock_bytes: &'a [u8],
    /// Verified runtime metadata.
    pub runtime: &'a VerifiedRuntime,
    /// Platform release metadata.
    pub platform: &'a PlatformReleaseMeta,
    /// Token-substituted Cap'n Proto text.
    pub rendered: &'a [u8],
}

/// Compute the schema-tagged SHA-256 hex digest.
#[must_use]
pub(crate) fn config_input_digest(inputs: &DigestInputs<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DIGEST_TAG);
    put_bytes(&mut hasher, inputs.config_template);
    put_u64(&mut hasher, inputs.workers.len() as u64);
    for (name, bytes) in inputs.workers {
        put_bytes(&mut hasher, name.as_bytes());
        put_bytes(&mut hasher, bytes);
    }
    put_bytes(&mut hasher, inputs.lock_bytes);
    put_bytes(&mut hasher, inputs.runtime.target().as_bytes());
    put_bytes(&mut hasher, inputs.runtime.release().as_bytes());
    put_bytes(&mut hasher, inputs.runtime.binary_sha256().as_bytes());
    put_bytes(&mut hasher, inputs.runtime.version_output().as_bytes());
    put_bytes(&mut hasher, inputs.platform.version.as_bytes());
    put_bytes(&mut hasher, inputs.rendered);
    hex_sha256(&hasher.finalize().into())
}

fn put_u64(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_be_bytes());
}

fn put_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    put_u64(hasher, bytes.len() as u64);
    hasher.update(bytes);
}

type WorkerFiles = Vec<(String, Vec<u8>)>;

pub(crate) fn load_assets(
    assets_dir: &Path,
) -> Result<(Vec<u8>, WorkerFiles, PathBuf), PlatformError> {
    crate::fsutil::require_absolute(assets_dir)?;
    let _ = open_dir_nofollow(assets_dir)?;
    let config_path = assets_dir.join("config.capnp");
    reject_symlink_escape(assets_dir, &config_path)?;
    let template = read_regular_nofollow(&config_path)?;
    let workers_dir = assets_dir.join("system-workers");
    reject_symlink_escape(assets_dir, &workers_dir)?;
    let files = list_files_sorted(&workers_dir)?;
    let mut workers = Vec::new();
    let mut total = template.len() as u64;
    for path in files {
        reject_symlink_escape(assets_dir, &path)?;
        let rel = path
            .strip_prefix(assets_dir)
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::PathInvalid,
                    "system worker is not under the assets directory",
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = read_regular_nofollow(&path)?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_ASSETS_TOTAL_BYTES {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "file exceeds the configured size bound",
            ));
        }
        workers.push((rel, bytes));
    }
    if workers.is_empty() {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "system worker sources are missing",
        ));
    }
    Ok((template, workers, config_path))
}

pub(crate) fn render_config(template: &str, token: &SecretString) -> Result<String, PlatformError> {
    validate_token(token)?;
    let count = template.matches(TOKEN_PLACEHOLDER).count();
    if count != 1 {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "config template must contain exactly one internal token placeholder",
        ));
    }
    Ok(template.replace(TOKEN_PLACEHOLDER, token.expose()))
}

pub(crate) fn validate_token(token: &SecretString) -> Result<(), PlatformError> {
    let value = token.expose();
    if value.len() != TOKEN_HEX_LEN || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "internal token must be a 256-bit hex string",
        ));
    }
    if value.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "internal token must be a 256-bit hex string",
        ));
    }
    Ok(())
}

pub(crate) fn digest_for(
    assets_dir: &Path,
    lock: &RuntimeLock,
    lock_bytes: &[u8],
    runtime: &VerifiedRuntime,
    platform: &PlatformReleaseMeta,
    token: &SecretString,
) -> Result<(String, String, WorkerFiles), PlatformError> {
    let _ = lock;
    let (template, workers, _) = load_assets(assets_dir)?;
    let template_str = std::str::from_utf8(&template).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "config template is not UTF-8",
        )
    })?;
    let rendered = render_config(template_str, token)?;
    let digest = config_input_digest(&DigestInputs {
        config_template: &template,
        workers: &workers,
        lock_bytes,
        runtime,
        platform,
        rendered: rendered.as_bytes(),
    });
    Ok((digest, rendered, workers))
}
