//! Deterministic static-config input digest.

use crate::fsutil::{
    MAX_ASSETS_TOTAL_BYTES, hex_sha256, list_files_sorted, open_dir_nofollow,
    read_regular_nofollow, reject_symlink_escape,
};
use crate::lock::RuntimeLock;
use crate::verify::VerifiedRuntime;
use open_compute_core::{DurableObjectsConfig, ErrorCode, PlatformError, SecretString};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const DIGEST_TAG: &[u8] = b"open-compute-static-config-v1\0";
pub(crate) const TOKEN_PLACEHOLDER: &str = "__OPEN_COMPUTE_INTERNAL_TOKEN__";
pub(crate) const BINDING_TOKEN_PLACEHOLDER: &str = "__OPEN_COMPUTE_BINDING_TOKEN__";
pub(crate) const TOKEN_HEX_LEN: usize = 64;
type DurableObjectPolicyPlaceholder = (&'static str, fn(&DurableObjectsConfig) -> String);
const DO_POLICY_PLACEHOLDERS: [DurableObjectPolicyPlaceholder; 7] = [
    ("__OPEN_COMPUTE_DO_MAX_OBJECT_NAME_BYTES__", |v| {
        v.max_object_name_bytes.to_string()
    }),
    ("__OPEN_COMPUTE_DO_MAX_RPC_REQUEST_BYTES__", |v| {
        v.max_rpc_request_bytes.to_string()
    }),
    ("__OPEN_COMPUTE_DO_MAX_RPC_RESPONSE_BYTES__", |v| {
        v.max_rpc_response_bytes.to_string()
    }),
    ("__OPEN_COMPUTE_DO_MAX_FETCH_BODY_BYTES__", |v| {
        v.max_fetch_body_bytes.to_string()
    }),
    ("__OPEN_COMPUTE_DO_DISPATCH_TIMEOUT_MS__", |v| {
        v.dispatch_timeout_ms.to_string()
    }),
    ("__OPEN_COMPUTE_DO_MAX_IN_FLIGHT_DISPATCHES__", |v| {
        v.max_in_flight_dispatches.to_string()
    }),
    ("__OPEN_COMPUTE_DO_DISK_STOP_WRITES_PERCENT__", |v| {
        v.disk_stop_writes_percent.to_string()
    }),
];

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

/// Compute a deterministic SHA-256 over the packaged runtime template and system Workers.
pub fn runtime_assets_sha256(assets_dir: &Path) -> Result<String, PlatformError> {
    let (template, workers, _) = load_assets(assets_dir)?;
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/runtime-assets/v1\0");
    put_bytes(&mut hasher, &template);
    put_u64(&mut hasher, workers.len() as u64);
    for (name, bytes) in workers {
        put_bytes(&mut hasher, name.as_bytes());
        put_bytes(&mut hasher, &bytes);
    }
    Ok(hex::encode(hasher.finalize()))
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

pub(crate) fn render_config_with_tokens(
    template: &str,
    token: &SecretString,
    binding_token: &SecretString,
) -> Result<String, PlatformError> {
    validate_token(token)?;
    validate_token(binding_token)?;
    if token.expose() == binding_token.expose() {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "internal service tokens must be distinct",
        ));
    }
    if template.matches(TOKEN_PLACEHOLDER).count() != 1
        || template.matches(BINDING_TOKEN_PLACEHOLDER).count() != 1
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "config template must contain each internal token placeholder exactly once",
        ));
    }
    Ok(template
        .replace(TOKEN_PLACEHOLDER, token.expose())
        .replace(BINDING_TOKEN_PLACEHOLDER, binding_token.expose()))
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

#[cfg(test)]
pub(crate) fn digest_for(
    assets_dir: &Path,
    lock: &RuntimeLock,
    lock_bytes: &[u8],
    runtime: &VerifiedRuntime,
    platform: &PlatformReleaseMeta,
    token: &SecretString,
) -> Result<(String, String, WorkerFiles), PlatformError> {
    digest_for_with_tokens(
        assets_dir, lock, lock_bytes, runtime, platform, token, token,
    )
}

#[cfg(test)]
pub(crate) fn digest_for_with_tokens(
    assets_dir: &Path,
    lock: &RuntimeLock,
    lock_bytes: &[u8],
    runtime: &VerifiedRuntime,
    platform: &PlatformReleaseMeta,
    token: &SecretString,
    binding_token: &SecretString,
) -> Result<(String, String, WorkerFiles), PlatformError> {
    digest_for_with_tokens_and_policy(
        assets_dir,
        lock,
        lock_bytes,
        runtime,
        platform,
        token,
        binding_token,
        &DurableObjectsConfig::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn digest_for_with_tokens_and_policy(
    assets_dir: &Path,
    lock: &RuntimeLock,
    lock_bytes: &[u8],
    runtime: &VerifiedRuntime,
    platform: &PlatformReleaseMeta,
    token: &SecretString,
    binding_token: &SecretString,
    durable_objects: &DurableObjectsConfig,
) -> Result<(String, String, WorkerFiles), PlatformError> {
    let _ = lock;
    let (template, workers, _) = load_assets(assets_dir)?;
    let template_str = std::str::from_utf8(&template).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "config template is not UTF-8",
        )
    })?;
    let rendered = if token.expose() == binding_token.expose() {
        let once = render_config(template_str, token)?;
        if once.matches(BINDING_TOKEN_PLACEHOLDER).count() != 1 {
            return Err(PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "config template must contain exactly one binding token placeholder",
            ));
        }
        once.replace(BINDING_TOKEN_PLACEHOLDER, binding_token.expose())
    } else {
        render_config_with_tokens(template_str, token, binding_token)?
    };
    let rendered = render_do_policy(rendered, durable_objects)?;
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

fn render_do_policy(
    mut rendered: String,
    durable_objects: &DurableObjectsConfig,
) -> Result<String, PlatformError> {
    for (placeholder, value) in DO_POLICY_PLACEHOLDERS {
        if rendered.matches(placeholder).count() != 1 {
            return Err(PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "config template must contain each Durable Object policy placeholder exactly once",
            ));
        }
        rendered = rendered.replace(placeholder, &value(durable_objects));
    }
    Ok(rendered)
}
