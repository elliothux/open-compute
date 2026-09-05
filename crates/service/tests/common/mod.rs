//! Shared helpers for integration tests.

#![allow(
    dead_code,
    reason = "each integration-test crate compiles this shared module with a different helper subset"
)]

use open_compute_core::PlatformConfig;
use open_compute_service::config_load::{LoadedConfig, load_platform_config};
use std::path::Path;
use std::process::Command;

/// Drop serde default env references when credentials are file-only.
pub(crate) fn clear_file_only_s3_env_defaults(config: &mut PlatformConfig) {
    if let Some(s3) = config.object_storage.as_s3_mut() {
        s3.normalize_implicit_env_defaults();
    }
}

/// Load platform config without inheriting shell `S3_*` env names on file credentials.
pub(crate) fn load_file_only_platform_config(path: &Path) -> LoadedConfig {
    let mut loaded = load_platform_config(path).expect("platform config");
    clear_file_only_s3_env_defaults(&mut loaded.config);
    loaded
}

/// Prevent child `ocd` processes from seeing developer shell S3 credentials.
pub(crate) fn scrub_shell_s3_env(command: &mut Command) {
    command.env_remove("S3_ACCESS_KEY_ID");
    command.env_remove("S3_SECRET_ACCESS_KEY");
}
