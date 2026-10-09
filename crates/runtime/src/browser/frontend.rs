//! Immutable, explicitly prepared native `DevTools` resources for a browser installation.

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::bufread::GzDecoder;
use open_compute_core::{ErrorCode, PlatformError};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const MAX_PACKED: u64 = 16 * 1024 * 1024;
const MAX_DECODED: u64 = 32 * 1024 * 1024;
const MAX_ASSET: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Bundle {
    binary_sha256: String,
    version: String,
    revision: String,
    protocol: Value,
    assets: Vec<EncodedAsset>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncodedAsset {
    path: String,
    sha256: String,
    media_type: String,
    data: String,
}

#[derive(Debug)]
struct Asset {
    bytes: Vec<u8>,
    media_type: String,
}

/// Verified native resources bound to one executable, version, and source revision.
#[derive(Debug)]
pub struct BrowserFrontend {
    pub(super) sha256: String,
    product: String,
    revision: String,
    protocol: Value,
    assets: BTreeMap<String, Asset>,
}

impl BrowserFrontend {
    pub(super) fn open(
        path: &Path,
        binary_sha256: &str,
        version: &str,
    ) -> Result<Self, PlatformError> {
        let file = crate::fsutil::open_nofollow(path, false, false)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o022 != 0
            || !(1..=MAX_PACKED).contains(&metadata.len())
        {
            return Err(invalid());
        }
        let mut packed = Vec::new();
        file.take(MAX_PACKED + 1)
            .read_to_end(&mut packed)
            .map_err(|_| invalid())?;
        if packed.len() as u64 != metadata.len() {
            return Err(invalid());
        }
        let sha256 = hex::encode(Sha256::digest(&packed));
        let mut decoded = Vec::new();
        let mut decoder = GzDecoder::new(packed.as_slice());
        decoder
            .by_ref()
            .take(MAX_DECODED + 1)
            .read_to_end(&mut decoded)
            .map_err(|_| invalid())?;
        if decoded.len() as u64 > MAX_DECODED || !decoder.into_inner().is_empty() {
            return Err(invalid());
        }
        let bundle: Bundle = serde_json::from_slice(&decoded).map_err(|_| invalid())?;
        let number = version
            .strip_prefix("Google Chrome for Testing ")
            .ok_or_else(invalid)?;
        if bundle.binary_sha256 != binary_sha256
            || bundle.version != version
            || !bundle.revision.strip_prefix('@').is_some_and(|revision| {
                revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || bundle
                .protocol
                .pointer("/version/major")
                .and_then(Value::as_str)
                != Some("1")
            || bundle
                .protocol
                .pointer("/version/minor")
                .and_then(Value::as_str)
                != Some("3")
            || !bundle
                .protocol
                .get("domains")
                .and_then(Value::as_array)
                .is_some_and(|domains| !domains.is_empty() && domains.len() <= 256)
            || bundle.assets.is_empty()
            || bundle.assets.len() > 4096
        {
            return Err(invalid());
        }
        let mut assets = BTreeMap::new();
        let mut total = 0usize;
        for asset in bundle.assets {
            let name = asset.path;
            if !valid_path(&name)
                || !valid_media(&asset.media_type)
                || asset.data.len() > MAX_ASSET * 4 / 3 + 4
            {
                return Err(invalid());
            }
            let bytes = STANDARD.decode(&asset.data).map_err(|_| invalid())?;
            total = total.checked_add(bytes.len()).ok_or_else(invalid)?;
            if bytes.is_empty()
                || bytes.len() > MAX_ASSET
                || total as u64 > MAX_DECODED
                || hex::encode(Sha256::digest(&bytes)) != asset.sha256
            {
                return Err(invalid());
            }
            if assets
                .insert(
                    name,
                    Asset {
                        bytes,
                        media_type: asset.media_type,
                    },
                )
                .is_some()
            {
                return Err(invalid());
            }
        }
        for name in [
            "inspector.html",
            "entrypoints/inspector/inspector.js",
            "LICENSE.headless_shell",
        ] {
            if !assets.contains_key(name) {
                return Err(invalid());
            }
        }
        Ok(Self {
            sha256,
            product: format!("HeadlessChrome/{number}"),
            revision: bundle.revision,
            protocol: bundle.protocol,
            assets,
        })
    }

    /// Read a prepared asset within the operator's response bound; no disk or network lookup.
    pub fn asset(&self, name: &str, maximum: usize) -> Result<(&[u8], &str), PlatformError> {
        let asset = self.assets.get(name).ok_or_else(|| {
            PlatformError::new(
                ErrorCode::BrowserUnsupported,
                "browser frontend resource unavailable",
            )
        })?;
        if asset.bytes.len() > maximum {
            return Err(PlatformError::new(
                ErrorCode::BrowserLimitExceeded,
                "browser frontend resource exceeds limit",
            ));
        }
        Ok((&asset.bytes, &asset.media_type))
    }

    /// Complete native schema captured during explicit installation preparation.
    #[must_use]
    pub fn protocol(&self) -> &Value {
        &self.protocol
    }

    pub(super) fn matches_version(&self, reply: &Value) -> bool {
        reply.pointer("/result/product").and_then(Value::as_str) == Some(self.product.as_str())
            && reply.pointer("/result/revision").and_then(Value::as_str)
                == Some(self.revision.as_str())
    }
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
}

fn valid_media(media: &str) -> bool {
    matches!(
        media,
        "text/html; charset=utf-8"
            | "text/javascript; charset=utf-8"
            | "text/css; charset=utf-8"
            | "text/plain; charset=utf-8"
            | "application/json"
            | "image/svg+xml"
            | "image/png"
            | "image/jpeg"
            | "image/webp"
            | "image/gif"
            | "font/woff2"
            | "font/woff"
            | "application/wasm"
            | "application/octet-stream"
    )
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::RuntimeInvalid,
        "browser frontend installation is invalid",
    )
}

#[cfg(test)]
#[path = "frontend_tests.rs"]
mod tests;
