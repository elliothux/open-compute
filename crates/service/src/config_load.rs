//! Secure absolute-path config load with no-follow semantics.

use crate::ai_tokenizer::AiTokenizerRegistry;
use open_compute_core::config::validate_bootstrap_config_path;
use open_compute_core::{ErrorCode, PlatformConfig, PlatformError};
use rustix::fd::OwnedFd;
use rustix::fs::{Mode, OFlags};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Conservative TOML size bound.
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// Config loaded from an exact path. Secrets are not resolved.
#[derive(Clone, Debug)]
pub struct LoadedConfig {
    /// Canonical absolute path of the exact opened configuration file.
    pub path: PathBuf,
    /// Parsed and statically validated configuration.
    pub config: PlatformConfig,
}

/// Open `path` without following links, require a regular UTF-8 file, parse strictly.
pub fn load_platform_config(path: &Path) -> Result<LoadedConfig, PlatformError> {
    validate_bootstrap_config_path(path)?;
    let startup_cwd = std::env::current_dir().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "startup working directory is unavailable",
        )
    })?;
    load_platform_config_from(path, &startup_cwd)
}

/// Load a config using one previously captured startup working directory.
pub fn load_platform_config_from(
    path: &Path,
    startup_cwd: &Path,
) -> Result<LoadedConfig, PlatformError> {
    validate_bootstrap_config_path(path)?;
    if !startup_cwd.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "startup working directory must be absolute",
        ));
    }
    let candidate = lexical_absolute(startup_cwd, path)?;
    let leaf = candidate.file_name().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must name a regular file",
        )
    })?;
    let parent = candidate.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config parent is invalid",
        )
    })?;
    let config_base = std::fs::canonicalize(parent).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config parent is unavailable",
        )
    })?;
    let opened_path = config_base.join(leaf);
    let config_parent = open_absolute_dir_nofollow(&config_base)?;
    let file = open_regular_nofollow(&config_parent, leaf)?;
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
    file.take(MAX_CONFIG_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "failed to read bootstrap --config file",
            )
        })?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config file exceeds the conservative size limit",
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigParseFailed,
            "bootstrap --config file is not valid UTF-8",
        )
    })?;
    let config = PlatformConfig::from_toml_str_at(text, &config_base)?;
    verify_parent_unchanged(parent, &config_base, &config_parent)?;
    let _ = AiTokenizerRegistry::load(&config.ai)?;
    Ok(LoadedConfig {
        path: opened_path,
        config,
    })
}

pub(crate) fn lexical_absolute(base: &Path, path: &Path) -> Result<PathBuf, PlatformError> {
    use std::path::Component;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(PlatformError::new(
                        ErrorCode::ConfigPathInvalid,
                        "bootstrap --config path escapes the filesystem root",
                    ));
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    if !normalized.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path did not resolve to an absolute path",
        ));
    }
    Ok(normalized)
}

fn open_regular_nofollow(parent: &OwnedFd, leaf: &std::ffi::OsStr) -> Result<File, PlatformError> {
    let fd = rustix::fs::openat(
        parent,
        leaf,
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

fn verify_parent_unchanged(
    requested_parent: &Path,
    resolved_parent: &Path,
    opened_parent: &OwnedFd,
) -> Result<(), PlatformError> {
    let current = std::fs::canonicalize(requested_parent).map_err(|_| parent_changed())?;
    if current != resolved_parent {
        return Err(parent_changed());
    }
    let current_fd = open_absolute_dir_nofollow(&current)?;
    let opened = rustix::fs::fstat(opened_parent).map_err(|_| parent_changed())?;
    let current = rustix::fs::fstat(&current_fd).map_err(|_| parent_changed())?;
    if opened.st_dev != current.st_dev || opened.st_ino != current.st_ino {
        return Err(parent_changed());
    }
    Ok(())
}

fn parent_changed() -> PlatformError {
    PlatformError::new(
        ErrorCode::ConfigPathInvalid,
        "bootstrap --config parent changed while the file was loaded",
    )
}

fn open_absolute_dir_nofollow(path: &Path) -> Result<OwnedFd, PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "canonical bootstrap --config parent must be absolute",
        ));
    }
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "failed to open filesystem root for bootstrap --config",
        )
    })?;
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        directory = rustix::fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "failed to open canonical bootstrap --config parent",
            )
        })?;
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_config_parent_identity_detects_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().join("config");
        std::fs::create_dir(&parent).unwrap();
        let resolved = std::fs::canonicalize(&parent).unwrap();
        let opened = open_absolute_dir_nofollow(&resolved).unwrap();
        verify_parent_unchanged(&parent, &resolved, &opened).unwrap();

        let moved = temporary.path().join("moved");
        std::fs::rename(&parent, &moved).unwrap();
        std::fs::create_dir(&parent).unwrap();
        assert_eq!(
            verify_parent_unchanged(&parent, &resolved, &opened)
                .unwrap_err()
                .code(),
            ErrorCode::ConfigPathInvalid
        );
    }
}
