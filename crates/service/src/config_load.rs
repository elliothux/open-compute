//! Secure absolute-path config load with no-follow semantics.

use crate::ai_tokenizer::AiTokenizerRegistry;
use open_compute_core::config::validate_bootstrap_config_path;
use open_compute_core::{ErrorCode, PlatformConfig, PlatformError};
use rustix::fs::{Mode, OFlags};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Conservative TOML size bound.
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// Config loaded from an exact path. Secrets are not resolved.
#[derive(Clone, Debug)]
pub struct LoadedConfig {
    /// Exact operator-supplied path (not canonicalized).
    pub path: PathBuf,
    /// Parsed and statically validated configuration.
    pub config: PlatformConfig,
}

/// Open `path` without following links, require a regular UTF-8 file, parse strictly.
pub fn load_platform_config(path: &Path) -> Result<LoadedConfig, PlatformError> {
    validate_bootstrap_config_path(path)?;
    let file = open_regular_nofollow(path)?;
    let meta = file.metadata().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must be a regular file",
        )
    })?;
    if !meta.file_type().is_file() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must be a regular file",
        ));
    }
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config file exceeds the conservative size limit",
        ));
    }
    let mut bytes = Vec::new();
    let mut file = file;
    file.read_to_end(&mut bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "failed to read bootstrap --config file",
        )
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigParseFailed,
            "bootstrap --config file is not valid UTF-8",
        )
    })?;
    let config = PlatformConfig::from_toml_str(text)?;
    let _ = AiTokenizerRegistry::load(&config.ai)?;
    Ok(LoadedConfig {
        path: path.to_path_buf(),
        config,
    })
}

fn open_regular_nofollow(path: &Path) -> Result<File, PlatformError> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|err| {
        if err == rustix::io::Errno::LOOP {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "bootstrap --config path must not be a symlink",
            )
        } else if err == rustix::io::Errno::NOENT {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "bootstrap --config path must be a regular file",
            )
        } else {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "failed to open bootstrap --config without following links",
            )
        }
    })?;
    Ok(File::from(fd))
}
