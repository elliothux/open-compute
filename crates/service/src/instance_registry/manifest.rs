//! Secure bounded OCD manifest reads, revisions, and atomic writes.

use super::{MANIFEST_NAME, MAX_MANIFEST_BYTES, OcdManifest, ensure_ocd_root, manifest_invalid};
use open_compute_core::PlatformError;
use open_compute_storage::atomic_write;
use rustix::fs::{Mode, OFlags};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn read_manifest(root: &Path) -> Result<OcdManifest, PlatformError> {
    let Some(bytes) = read_manifest_bytes(root)? else {
        return Ok(OcdManifest::default());
    };
    let mut manifest: OcdManifest =
        toml::from_slice(&bytes).map_err(|_| manifest_invalid("ocd.toml is invalid"))?;
    manifest.server.resolve_paths(root)?;
    manifest.server.validate()?;
    if let Some(gateway) = &mut manifest.gateway {
        gateway.resolve_paths(root)?;
        gateway.validate()?;
        crate::config_load::validate_caddy_sources(gateway)?;
    }
    manifest.artifacts.validate()?;
    manifest.metrics.validate()?;
    Ok(manifest)
}

fn read_manifest_bytes(root: &Path) -> Result<Option<Vec<u8>>, PlatformError> {
    let path = root.join(MANIFEST_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(manifest_invalid("failed to inspect ocd.toml")),
        Ok(metadata) => metadata,
    };
    validate_manifest_metadata(&metadata)?;
    let fd = rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| manifest_invalid("failed to open ocd.toml without following links"))?;
    let mut file = File::from(fd);
    let opened = file
        .metadata()
        .map_err(|_| manifest_invalid("failed to inspect opened ocd.toml"))?;
    validate_manifest_metadata(&opened)?;
    if opened.len() > MAX_MANIFEST_BYTES {
        return Err(manifest_invalid("ocd.toml exceeds its size limit"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_MANIFEST_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| manifest_invalid("failed to read ocd.toml"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(manifest_invalid("ocd.toml exceeds its size limit"));
    }
    Ok(Some(bytes))
}

pub(crate) fn manifest_digest(root: &Path) -> Result<Option<String>, PlatformError> {
    Ok(read_manifest_bytes(root)?.map(|bytes| hex::encode(Sha256::digest(bytes))))
}

pub(super) fn write_manifest(root: &Path, manifest: &OcdManifest) -> Result<(), PlatformError> {
    ensure_ocd_root(root)?;
    let body = toml::to_string_pretty(manifest)
        .map_err(|_| manifest_invalid("failed to encode ocd.toml"))?;
    let path = root.join(MANIFEST_NAME);
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(manifest_invalid("ocd.toml exceeds its size limit"));
    }
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        validate_manifest_metadata(&metadata)?;
    }
    atomic_write(&path, body.as_bytes())
        .map_err(|_| manifest_invalid("failed to persist ocd.toml"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| manifest_invalid("failed to secure ocd.toml"))?;
    File::open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|_| manifest_invalid("failed to sync ocd.toml"))
}

fn validate_manifest_metadata(metadata: &fs::Metadata) -> Result<(), PlatformError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(manifest_invalid(
            "ocd.toml must be a regular non-symlink file",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(manifest_invalid(
            "ocd.toml must not be accessible by group or world",
        ));
    }
    Ok(())
}
