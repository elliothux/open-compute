//! Secure bounded OCD manifest reads, revisions, and atomic writes.

use super::{MANIFEST_NAME, MAX_MANIFEST_BYTES, OcdManifest, ensure_ocd_root, manifest_invalid};
use open_compute_core::PlatformError;
use open_compute_storage::fs::atomic_write;
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
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(manifest_invalid("failed to open ocd.toml")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| manifest_invalid("failed to read ocd.toml"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(manifest_invalid("ocd.toml exceeds its size limit"));
    }
    Ok(Some(bytes))
}

pub(super) fn write_manifest(root: &Path, manifest: &OcdManifest) -> Result<(), PlatformError> {
    ensure_ocd_root(root)?;
    let body = toml::to_string_pretty(manifest)
        .map_err(|_| manifest_invalid("failed to encode ocd.toml"))?;
    let path = root.join(MANIFEST_NAME);
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(manifest_invalid("ocd.toml exceeds its size limit"));
    }
    atomic_write(&path, body.as_bytes())
        .map_err(|_| manifest_invalid("failed to persist ocd.toml"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| manifest_invalid("failed to secure ocd.toml"))?;
    File::open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|_| manifest_invalid("failed to sync ocd.toml"))
}
