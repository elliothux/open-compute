//! Deterministic static-config input digest.

use crate::fsutil::{
    MAX_ASSETS_TOTAL_BYTES, hex_sha256, list_files_sorted, open_dir_nofollow,
    read_regular_nofollow, reject_symlink_escape,
};
use crate::verify::VerifiedRuntime;
use open_compute_core::{DurableObjectsConfig, ErrorCode, PlatformError, SecretString};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const DIGEST_TAG: &[u8] = b"open-compute-static-config-v1\0";
pub(crate) const TOKEN_PLACEHOLDER: &str = "__OPEN_COMPUTE_INTERNAL_TOKEN__";
pub(crate) const BINDING_TOKEN_PLACEHOLDER: &str = "__OPEN_COMPUTE_BINDING_TOKEN__";
pub(crate) const COMPATIBILITY_DATE_PLACEHOLDER: &str = "__OPEN_COMPUTE_COMPATIBILITY_DATE__";
pub(crate) const SYSTEM_COMPATIBILITY_FLAGS_PLACEHOLDER: &str =
    "__OPEN_COMPUTE_SYSTEM_COMPATIBILITY_FLAGS__";
pub(crate) const REQUIRED_COMPATIBILITY_FLAGS_JSON_PLACEHOLDER: &str =
    "__OPEN_COMPUTE_REQUIRED_COMPATIBILITY_FLAGS_JSON__";
pub(crate) const TOKEN_HEX_LEN: usize = 64;
type DurableObjectPolicyPlaceholder = (&'static str, fn(&DurableObjectsConfig) -> String);
const DO_POLICY_PLACEHOLDERS: [DurableObjectPolicyPlaceholder; 5] = [
    ("__OPEN_COMPUTE_DO_MAX_OBJECT_NAME_BYTES__", |v| {
        v.max_object_name_bytes.to_string()
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
    let workers_dir = assets_dir.join("dist");
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
    Ok(asset_bytes_sha256(&template, &workers))
}

fn asset_bytes_sha256(template: &[u8], workers: &[(String, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/runtime-assets/v1\0");
    put_bytes(&mut hasher, template);
    put_u64(&mut hasher, workers.len() as u64);
    for (name, bytes) in workers {
        put_bytes(&mut hasher, name.as_bytes());
        put_bytes(&mut hasher, bytes);
    }
    hex::encode(hasher.finalize())
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
    lock_bytes: &[u8],
    runtime: &VerifiedRuntime,
    platform: &PlatformReleaseMeta,
    token: &SecretString,
    binding_token: &SecretString,
) -> Result<(String, String, WorkerFiles), PlatformError> {
    digest_for_with_tokens_and_policy(
        assets_dir,
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
    lock_bytes: &[u8],
    runtime: &VerifiedRuntime,
    platform: &PlatformReleaseMeta,
    token: &SecretString,
    binding_token: &SecretString,
    durable_objects: &DurableObjectsConfig,
) -> Result<(String, String, WorkerFiles), PlatformError> {
    let (template, workers, _) = load_assets(assets_dir)?;
    if let Some(expected) = runtime.expected_assets_sha256
        && asset_bytes_sha256(&template, &workers) != expected
    {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "runtime compiler inputs do not match the embedded payload",
        ));
    }
    let template_str = std::str::from_utf8(&template).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "config template is not UTF-8",
        )
    })?;
    let rendered = render_config_with_tokens(template_str, token, binding_token)?;
    let rendered = render_do_policy(rendered, durable_objects)?;
    let rendered = render_lock_compatibility(rendered, runtime.lock())?;
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

pub(crate) fn render_lock_compatibility(
    mut rendered: String,
    lock: &crate::lock::RuntimeLock,
) -> Result<String, PlatformError> {
    for (placeholder, expected) in [
        (COMPATIBILITY_DATE_PLACEHOLDER, 1_usize),
        (SYSTEM_COMPATIBILITY_FLAGS_PLACEHOLDER, 3),
        (REQUIRED_COMPATIBILITY_FLAGS_JSON_PLACEHOLDER, 1),
    ] {
        if rendered.matches(placeholder).count() != expected {
            return Err(PlatformError::new(
                ErrorCode::ConfigCompileFailed,
                "config template must contain each lock compatibility placeholder exactly once",
            ));
        }
    }
    let date = lock.effective_compatibility_date.as_str();
    if date.bytes().any(|b| !b.is_ascii_digit() && b != b'-') {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "compatibility date is not safe to render into Cap'n Proto",
        ));
    }
    let system_flags = canonical_system_flags(lock);
    let required_json = capnp_escaped_json(&lock.required_compatibility_flags)?;
    rendered = rendered.replace(COMPATIBILITY_DATE_PLACEHOLDER, date);
    rendered = rendered.replace(
        SYSTEM_COMPATIBILITY_FLAGS_PLACEHOLDER,
        &capnp_text_list(&system_flags)?,
    );
    rendered = rendered.replace(
        REQUIRED_COMPATIBILITY_FLAGS_JSON_PLACEHOLDER,
        &required_json,
    );
    if rendered.contains(COMPATIBILITY_DATE_PLACEHOLDER)
        || rendered.contains(SYSTEM_COMPATIBILITY_FLAGS_PLACEHOLDER)
        || rendered.contains(REQUIRED_COMPATIBILITY_FLAGS_JSON_PLACEHOLDER)
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigCompileFailed,
            "config template still contains a lock compatibility placeholder",
        ));
    }
    Ok(rendered)
}

fn canonical_system_flags(lock: &crate::lock::RuntimeLock) -> Vec<String> {
    lock.required_compatibility_flags
        .iter()
        .chain(lock.system_compatibility_flags.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn capnp_text_list(flags: &[String]) -> Result<String, PlatformError> {
    let mut items = Vec::with_capacity(flags.len());
    for flag in flags {
        require_renderable_flag(flag)?;
        items.push(format!("\"{flag}\""));
    }
    Ok(format!("[{}]", items.join(", ")))
}

fn capnp_escaped_json(flags: &[String]) -> Result<String, PlatformError> {
    for flag in flags {
        require_renderable_flag(flag)?;
    }
    let json = serde_json::to_string(flags).map_err(|_| {
        PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "required compatibility flags could not be encoded",
        )
    })?;
    Ok(json.replace('\\', "\\\\").replace('"', "\\\""))
}

fn require_renderable_flag(flag: &str) -> Result<(), PlatformError> {
    if flag.is_empty() || !flag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(PlatformError::new(
            ErrorCode::RuntimeInvalid,
            "compatibility flag is not safe to render into Cap'n Proto",
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{RuntimeLock, RuntimeTarget, WorkersSdkPin, WorkersTypesPin};
    use std::collections::BTreeMap;

    const TEMPLATE: &str = include_str!("../../../packages/runtime/config.capnp");

    fn lock(required: &[&str], system: &[&str]) -> RuntimeLock {
        RuntimeLock {
            schema_version: 1,
            release: "v1.20260830.1".to_owned(),
            revision: "e9dda5963aba7ee4323960db795690ec78fec118".to_owned(),
            expected_version_output: "workerd 2026-08-30".to_owned(),
            effective_compatibility_date: "2026-08-30".to_owned(),
            required_compatibility_flags: required.iter().map(|flag| (*flag).to_owned()).collect(),
            system_compatibility_flags: system.iter().map(|flag| (*flag).to_owned()).collect(),
            process_flags: vec!["--experimental".to_owned()],
            workers_types: WorkersTypesPin {
                version: "5.20260830.1".to_owned(),
                git_head: "e9dda5963aba7ee4323960db795690ec78fec118".to_owned(),
                package_sha256: "aa".repeat(32),
                ast_sha256: "bb".repeat(32),
            },
            workers_sdk: WorkersSdkPin {
                revision: "f8085545bcaa2c639f171c25e4424685036a0e10".to_owned(),
                wrangler_version: "4.127.1".to_owned(),
                vite_plugin_version: "1.54.2".to_owned(),
            },
            targets: BTreeMap::from([(
                "darwin-arm64".to_owned(),
                RuntimeTarget {
                    archive_name: "workerd-darwin-arm64.gz".to_owned(),
                    archive_url: "https://github.com/cloudflare/workerd/releases/download/v1.20260830.1/workerd-darwin-arm64.gz".to_owned(),
                    archive_sha256: "aa".repeat(32),
                    binary_sha256: "bb".repeat(32),
                },
            )]),
        }
    }

    #[test]
    fn packaged_template_has_one_lock_authority_and_separates_tenant_system_flags() {
        assert_eq!(TEMPLATE.matches(COMPATIBILITY_DATE_PLACEHOLDER).count(), 1);
        assert_eq!(
            TEMPLATE
                .matches(SYSTEM_COMPATIBILITY_FLAGS_PLACEHOLDER)
                .count(),
            3
        );
        assert_eq!(
            TEMPLATE
                .matches(REQUIRED_COMPATIBILITY_FLAGS_JSON_PLACEHOLDER)
                .count(),
            1
        );
        let lock = lock(
            &["nodejs_compat"],
            &["experimental", "service_binding_extra_handlers"],
        );
        let first = render_lock_compatibility(TEMPLATE.to_owned(), &lock).unwrap();
        let second = render_lock_compatibility(TEMPLATE.to_owned(), &lock).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("compatibilityDate = .compatibilityDate"));
        assert_eq!(
            first.matches(
                r#"compatibilityFlags = ["experimental", "nodejs_compat", "service_binding_extra_handlers"]"#
            )
            .count(),
            3
        );
        assert!(first.contains(r#"const compatibilityDate :Text = "2026-08-30";"#));
        assert!(
            first
                .contains(r#"const requiredCompatibilityFlagsJson :Text = "[\"nodejs_compat\"]";"#)
        );
        assert!(!first.contains(COMPATIBILITY_DATE_PLACEHOLDER));
        assert!(!first.contains(SYSTEM_COMPATIBILITY_FLAGS_PLACEHOLDER));
        assert!(!first.contains(REQUIRED_COMPATIBILITY_FLAGS_JSON_PLACEHOLDER));
    }

    #[test]
    fn lock_rendering_is_fail_closed_for_missing_duplicate_and_unsafe_placeholders() {
        let lock = lock(&[], &["experimental"]);
        assert_eq!(
            render_lock_compatibility("no placeholders".to_owned(), &lock)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigCompileFailed
        );
        let duplicated = TEMPLATE.replacen(
            COMPATIBILITY_DATE_PLACEHOLDER,
            &format!("{COMPATIBILITY_DATE_PLACEHOLDER}{COMPATIBILITY_DATE_PLACEHOLDER}"),
            1,
        );
        assert_eq!(
            render_lock_compatibility(duplicated, &lock)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigCompileFailed
        );
        let mut unsafe_lock = lock.clone();
        unsafe_lock.required_compatibility_flags = vec!["bad-flag".to_owned()];
        assert_eq!(
            render_lock_compatibility(TEMPLATE.to_owned(), &unsafe_lock)
                .unwrap_err()
                .code(),
            ErrorCode::RuntimeInvalid
        );
    }

    #[test]
    fn system_compatibility_flags_are_a_sorted_union_independent_of_input_order() {
        let first = lock(
            &["nodejs_compat", "rpc"],
            &["service_binding_extra_handlers", "experimental"],
        );
        let second = lock(
            &["rpc", "nodejs_compat"],
            &["experimental", "service_binding_extra_handlers"],
        );
        let rendered_first = render_lock_compatibility(TEMPLATE.to_owned(), &first).unwrap();
        let rendered_second = render_lock_compatibility(TEMPLATE.to_owned(), &second).unwrap();
        let expected = r#"compatibilityFlags = ["experimental", "nodejs_compat", "rpc", "service_binding_extra_handlers"]"#;
        assert_eq!(rendered_first.matches(expected).count(), 3);
        assert_eq!(rendered_second.matches(expected).count(), 3);
        assert!(rendered_first.contains(
            r#"const requiredCompatibilityFlagsJson :Text = "[\"nodejs_compat\",\"rpc\"]";"#
        ));
        assert!(rendered_second.contains(
            r#"const requiredCompatibilityFlagsJson :Text = "[\"rpc\",\"nodejs_compat\"]";"#
        ));
        assert!(!rendered_first.contains(r#"\"experimental\""#));
        assert!(!rendered_second.contains(r#"\"service_binding_extra_handlers\""#));
    }
}
