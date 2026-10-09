//! Session-owned download directories; client paths never select host filesystem authority.

use open_compute_core::{ErrorCode, PlatformError};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Debug)]
pub(super) struct BrowserDownloads {
    root: PathBuf,
    maximum_bytes: u64,
    maximum_files: u32,
}

impl BrowserDownloads {
    pub(super) fn new(
        workspace: &std::path::Path,
        maximum_bytes: u64,
        maximum_files: u32,
    ) -> Result<Self, PlatformError> {
        let downloads = workspace.join("downloads");
        crate::fsutil::create_dir_secure(&downloads)?;
        let root = downloads.join(uuid::Uuid::now_v7().to_string());
        crate::fsutil::create_dir_secure(&root)?;
        Ok(Self {
            root,
            maximum_bytes,
            maximum_files,
        })
    }

    pub(super) fn configure(&self, params: &mut Value) -> Result<(), PlatformError> {
        let context = params
            .get("browserContextId")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        if params.get("behavior").and_then(Value::as_str) == Some("allowAndName") {
            let path = self.context(context);
            crate::fsutil::create_dir_secure(&path)?;
            params["downloadPath"] = path.to_str().ok_or_else(denied)?.into();
        } else {
            params
                .as_object_mut()
                .ok_or_else(denied)?
                .remove("downloadPath");
        }
        Ok(())
    }

    pub(super) fn remove_context(&self, context: &str) -> Result<(), PlatformError> {
        let path = self.context(context);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(denied()),
            Ok(_) => {}
        }
        crate::fsutil::open_dir_nofollow(&path)?;
        std::fs::remove_dir_all(path).map_err(|_| denied())
    }

    pub(super) fn remove(&self) -> Result<(), PlatformError> {
        crate::fsutil::open_dir_nofollow(&self.root)?;
        std::fs::remove_dir_all(&self.root).map_err(|_| denied())
    }

    pub(super) fn check(&self) -> Result<(), PlatformError> {
        crate::fsutil::open_dir_nofollow(&self.root)?;
        let mut files = 0_u32;
        let mut bytes = 0_u64;
        for (index, entry) in std::fs::read_dir(&self.root)
            .map_err(|_| denied())?
            .enumerate()
        {
            if index >= 4_096 {
                return Err(denied());
            }
            let context = entry.map_err(|_| denied())?.path();
            match std::fs::symlink_metadata(&context) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(denied()),
                Ok(metadata) if !metadata.is_dir() => return Err(denied()),
                Ok(_) => {}
            }
            crate::fsutil::open_dir_nofollow(&context)?;
            let entries = match std::fs::read_dir(context) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(denied()),
            };
            for entry in entries {
                let entry = entry.map_err(|_| denied())?;
                let metadata = match std::fs::symlink_metadata(entry.path()) {
                    Ok(metadata) => metadata,
                    // Chromium atomically renames .crdownload on completion.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => return Err(denied()),
                };
                if !metadata.is_file() {
                    return Err(denied());
                }
                files = files.checked_add(1).ok_or_else(denied)?;
                bytes = bytes.checked_add(metadata.len()).ok_or_else(denied)?;
                if files > self.maximum_files || bytes > self.maximum_bytes {
                    return Err(denied());
                }
            }
        }
        Ok(())
    }

    fn context(&self, context: &str) -> PathBuf {
        self.root
            .join(hex::encode(Sha256::digest(context.as_bytes())))
    }
}

fn denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserLimitExceeded,
        "browser download boundary exceeded",
    )
}

#[cfg(test)]
#[path = "downloads_tests.rs"]
mod tests;
