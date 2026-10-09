//! Resolve the explicit native CDP endpoint without redirects or credential forwarding.

use crate::auth::resolve_bearer_auth;
use crate::operator_http::OperatorHttpClient;
use open_compute_core::{BrowserBackendConfig, BrowserConfig, ErrorCode, PlatformError};
use open_compute_runtime::browser::BrowserCdp;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;
use url::Url;

pub(super) struct BrowserBackend {
    pub(super) cdp: BrowserCdp,
    pub(super) contract: [u8; 32],
}

impl BrowserBackend {
    pub(super) async fn frontend(
        config: &BrowserConfig,
        transport: &OperatorHttpClient,
        asset: &str,
    ) -> Result<(Vec<u8>, String), PlatformError> {
        if asset.len() > 1024
            || asset
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
            || !asset.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.')
            })
        {
            return Err(unavailable());
        }
        let BrowserBackendConfig::Cdp { url, authorization } = &config.backend else {
            return Err(unavailable());
        };
        let mut url = open_compute_core::config::validate_cdp_url(url)?;
        let scheme = match url.scheme() {
            "ws" => "http",
            "wss" => "https",
            scheme => scheme,
        }
        .to_owned();
        url.set_scheme(&scheme).map_err(|()| unavailable())?;
        url.set_path(&format!("/devtools/{asset}"));
        let secret = authorization
            .as_ref()
            .map(resolve_bearer_auth)
            .transpose()?;
        let header = secret
            .as_ref()
            .map(|secret| format!("Bearer {}", secret.expose()));
        let bytes = read_bytes(
            transport,
            &url,
            header.as_deref(),
            Duration::from_millis(config.command_timeout_ms),
            config.max_result_bytes as usize,
        )
        .await?;
        let media = match asset.rsplit('.').next() {
            Some("html") => "text/html; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            Some("woff2") => "font/woff2",
            Some("wasm") => "application/wasm",
            _ => "application/octet-stream",
        };
        Ok((bytes, media.into()))
    }
    pub(super) async fn protocol(
        config: &BrowserConfig,
    ) -> Result<serde_json::Value, PlatformError> {
        let BrowserBackendConfig::Cdp { url, authorization } = &config.backend else {
            return Err(unavailable());
        };
        let mut url = open_compute_core::config::validate_cdp_url(url)?;
        // Chrome's browser endpoint and /json/protocol share one HTTP origin; no alternative URL or redirect is tried.
        let scheme = match url.scheme() {
            "ws" => "http",
            "wss" => "https",
            scheme => scheme,
        };
        let scheme = scheme.to_owned();
        url.set_scheme(&scheme).map_err(|()| unavailable())?;
        url.set_path("/json/protocol");
        let secret = authorization
            .as_ref()
            .map(resolve_bearer_auth)
            .transpose()?;
        let header = secret
            .as_ref()
            .map(|value| format!("Bearer {}", value.expose()));
        let value = read_json(
            &url,
            header.as_deref(),
            Duration::from_millis(config.command_timeout_ms),
            config.max_result_bytes as usize,
        )
        .await?;
        if !value
            .get("domains")
            .is_some_and(serde_json::Value::is_array)
        {
            return Err(unavailable());
        }
        Ok(value)
    }
    pub(super) async fn connect(
        config: &BrowserConfig,
        target: Option<&str>,
    ) -> Result<Self, PlatformError> {
        let BrowserBackendConfig::Cdp { url, authorization } = &config.backend else {
            // Managed admission requires the shared process owner and scoped CDP transport.
            return Err(unavailable());
        };
        let original = open_compute_core::config::validate_cdp_url(url)?;
        let secret = authorization
            .as_ref()
            .map(resolve_bearer_auth)
            .transpose()?;
        let header = secret
            .as_ref()
            .map(|value| format!("Bearer {}", value.expose()));
        let endpoint = discover(
            &original,
            header.as_deref(),
            Duration::from_millis(config.acquire_timeout_ms),
        )
        .await?;
        let mut socket_endpoint = endpoint.clone();
        if let Some(target) = target {
            socket_endpoint.set_path(&format!("/devtools/page/{target}"));
        }
        let cdp = BrowserCdp::connect(
            socket_endpoint.as_str(),
            header.as_deref(),
            config.max_message_bytes as usize,
            config.max_queued_messages as usize,
            Duration::from_millis(config.command_timeout_ms),
        )
        .await?;
        let version = cdp
            .command("Browser.getVersion", serde_json::json!({}), None)
            .await?;
        let result = version.get("result").ok_or_else(unavailable)?;
        if result
            .get("product")
            .and_then(serde_json::Value::as_str)
            .is_none()
            || result
                .get("protocolVersion")
                .and_then(serde_json::Value::as_str)
                != Some("1.3")
        {
            return Err(unavailable());
        }
        let mut digest = Sha256::new();
        digest.update(b"open-compute/browser/native-cdp/v1\0");
        digest.update(original.as_str().as_bytes());
        digest.update(b"\0");
        digest.update(endpoint.as_str().as_bytes());
        digest.update(b"\0");
        digest.update(result.to_string().as_bytes());
        Ok(Self {
            cdp,
            contract: digest.finalize().into(),
        })
    }
}

async fn discover(
    url: &Url,
    authorization: Option<&str>,
    deadline: Duration,
) -> Result<Url, PlatformError> {
    if matches!(url.scheme(), "ws" | "wss") {
        return Ok(url.clone());
    }
    let url = url.join("/json/version").map_err(|_| unavailable())?;
    let value = read_json(&url, authorization, deadline, 64 * 1024).await?;
    #[derive(Deserialize)]
    struct Version {
        #[serde(rename = "webSocketDebuggerUrl")]
        endpoint: String,
    }
    let response: Version = serde_json::from_value(value).map_err(|_| unavailable())?;
    let endpoint = open_compute_core::config::validate_cdp_url(&response.endpoint)
        .map_err(|_| unavailable())?;
    let expected = if url.scheme() == "https" { "wss" } else { "ws" };
    if endpoint.scheme() != expected
        || endpoint.host_str() != url.host_str()
        || endpoint.port_or_known_default() != url.port_or_known_default()
    {
        return Err(unavailable());
    }
    Ok(endpoint)
}

async fn read_json(
    url: &Url,
    authorization: Option<&str>,
    deadline: Duration,
    maximum: usize,
) -> Result<serde_json::Value, PlatformError> {
    let transport = OperatorHttpClient::from_process_env()?;
    serde_json::from_slice(&read_bytes(&transport, url, authorization, deadline, maximum).await?)
        .map_err(|_| unavailable())
}

async fn read_bytes(
    transport: &OperatorHttpClient,
    url: &Url,
    authorization: Option<&str>,
    deadline: Duration,
    maximum: usize,
) -> Result<Vec<u8>, PlatformError> {
    let mut request = transport
        .request(reqwest::Method::GET, url.clone())?
        .timeout(deadline);
    if let Some(value) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let mut response = request.send().await.map_err(|_| unavailable())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > maximum as u64)
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::BrowserUnavailable,
        "browser backend is unavailable",
    )
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
