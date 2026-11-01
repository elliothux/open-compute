//! Broker-owned generation credentials and their fixed private rootfs transport.

use super::{
    contract::{invalid, opaque},
    engine::Engine,
};
use axum::http::Method;
use open_compute_core::PlatformError;
use serde::{Deserialize, Serialize};

pub(super) const CONTROL_HEADER: &str = "X-Open-Compute-Container-Control";
pub(super) const AUTH_PATH: &str = "/run/open-compute/control.json";

/// Private native control credentials. Never serialize into tenant-visible data.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrokerControlCredentials {
    /// Token authenticating every sidecar control request.
    pub control_token: String,
    /// Token authenticating sidecar callbacks into the trusted workerd listener.
    pub callback_token: String,
}

impl std::fmt::Debug for BrokerControlCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BrokerControlCredentials([REDACTED])")
    }
}

impl BrokerControlCredentials {
    pub(super) fn generate() -> Result<Self, PlatformError> {
        Ok(Self {
            control_token: hex::encode(rand::random::<[u8; 32]>()),
            callback_token: hex::encode(rand::random::<[u8; 32]>()),
        })
    }

    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if !opaque(&self.control_token)
            || !opaque(&self.callback_token)
            || self.control_token == self.callback_token
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(super) async fn install(
    engine: &Engine,
    name: &str,
    credentials: &BrokerControlCredentials,
) -> Result<(), PlatformError> {
    credentials.validate()?;
    let bytes = serde_json::to_vec(credentials).map_err(|_| invalid())?;
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_path("control.json").map_err(|_| invalid())?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append(&header, bytes.as_slice())
        .map_err(|_| invalid())?;
    let archive = builder.into_inner().map_err(|_| invalid())?;
    let response = engine
        .send(
            Method::PUT,
            &format!("/containers/{name}/archive?path=%2Frun%2Fopen-compute"),
            archive,
            false,
        )
        .await?;
    if response.status().as_u16() != 200 {
        return Err(invalid());
    }
    super::engine::bounded_response(response, 1_024).await?;
    Ok(())
}

/// Validate the sole native archive producer: a CA file under the fixed /etc target.
pub(super) fn validate_ca_archive(bytes: &[u8]) -> Result<(), PlatformError> {
    open_compute_artifacts::verify_container_archive(std::io::Cursor::new(bytes), 65_536, 1)?;
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut entries = archive.entries().map_err(|_| invalid())?;
    let mut entry = entries.next().ok_or_else(invalid)?.map_err(|_| invalid())?;
    if !entry.header().entry_type().is_file()
        || entry.path().map_err(|_| invalid())?.as_ref()
            != std::path::Path::new("cloudflare/certs/cloudflare-containers-ca.crt")
        || entry.header().mode().map_err(|_| invalid())? != 0o644
        || entry.size() > 16_384
        || entries.next().is_some()
    {
        return Err(invalid());
    }
    use std::io::Read as _;
    let mut certificate = Vec::new();
    entry.read_to_end(&mut certificate).map_err(|_| invalid())?;
    reqwest::Certificate::from_pem(&certificate).map_err(|_| invalid())?;
    Ok(())
}
