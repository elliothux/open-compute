//! Bounded authenticated probes for remote Wrangler targets.

use crate::target_registry::TargetRecord;
use hyper::StatusCode;
use hyper::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use open_compute_core::{ErrorCode, InstanceId, PlatformError, SecretString};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

const TARGET_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TARGET_RESPONSE_BYTES: usize = 256 * 1024;

type GetFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, PlatformError>> + Send + 'a>>;

/// Injectable authenticated GET boundary for target probes.
pub trait TargetHttp: Send + Sync {
    /// GET one exact target URL without following redirects.
    fn get<'a>(&'a self, url: &'a str, token: &'a SecretString) -> GetFuture<'a>;
}

/// Production target client using system-independent web PKI roots.
#[derive(Clone, Debug)]
pub struct LiveTargetHttp {
    client: crate::operator_http::OperatorHttpClient,
    timeout: Duration,
}

impl LiveTargetHttp {
    /// Build the strict production target client.
    pub fn new() -> Result<Self, PlatformError> {
        Ok(Self {
            client: crate::operator_http::OperatorHttpClient::from_process_env()?,
            timeout: TARGET_HTTP_TIMEOUT,
        })
    }
}

impl TargetHttp for LiveTargetHttp {
    fn get<'a>(&'a self, url: &'a str, token: &'a SecretString) -> GetFuture<'a> {
        Box::pin(async move {
            let url = url::Url::parse(url)
                .map_err(|_| target_unavailable("target request URL is invalid"))?;
            let request = self
                .client
                .request(reqwest::Method::GET, url)
                .map_err(|_| target_unavailable("target request URL is invalid"))?
                .header(
                    USER_AGENT,
                    format!("open-compute-ocd/{}", env!("CARGO_PKG_VERSION")),
                )
                .header(ACCEPT, "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", token.expose()));
            tokio::time::timeout(self.timeout, async {
                let response = request.send().await.map_err(|_| {
                    target_unavailable("target request failed before a response was received")
                })?;
                if is_redirect(response.status()) {
                    return Err(target_unavailable(
                        "target request refused an HTTP redirect",
                    ));
                }
                if !response.status().is_success() {
                    return Err(target_unavailable(
                        "target rejected authentication, account, or capability discovery",
                    ));
                }
                collect_body(response).await
            })
            .await
            .map_err(|_| target_unavailable("target request timed out"))?
        })
    }
}

/// Verified target facts required by the Wrangler launcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetCapabilities {
    /// Exact Wrangler version certified by the selected target.
    pub wrangler_version: String,
}

/// Verify account discovery and capabilities for an explicit target test.
pub async fn probe_target(
    http: &dyn TargetHttp,
    record: &TargetRecord,
    token: &SecretString,
) -> Result<TargetCapabilities, PlatformError> {
    let account_url = record
        .api_base_url
        .endpoint(&format!("/accounts/{}", record.instance_id));
    let account_body = http.get(&account_url, token).await?;
    let account: Envelope<AccountResult> = serde_json::from_slice(&account_body)
        .map_err(|_| target_unavailable("target account response is invalid"))?;
    if !account.success || account.result.id != record.instance_id {
        return Err(target_unavailable(
            "target account response does not match the configured account",
        ));
    }
    fetch_capabilities(http, record, token).await
}

/// Fetch and validate only the target capability contract used before launch.
pub async fn fetch_capabilities(
    http: &dyn TargetHttp,
    record: &TargetRecord,
    token: &SecretString,
) -> Result<TargetCapabilities, PlatformError> {
    fetch_capabilities_at(http, record.api_base_url.as_str(), token).await
}

/// Fetch capabilities from one already-selected local or remote API base URL.
pub async fn fetch_capabilities_at(
    http: &dyn TargetHttp,
    api_base_url: &str,
    token: &SecretString,
) -> Result<TargetCapabilities, PlatformError> {
    let body = http
        .get(&format!("{api_base_url}/open-compute/capabilities"), token)
        .await?;
    let capabilities: Envelope<CapabilitiesResult> = serde_json::from_slice(&body)
        .map_err(|_| target_unavailable("target capabilities response is invalid"))?;
    if !capabilities.success || !valid_version(&capabilities.result.wrangler_version) {
        return Err(target_unavailable(
            "target capabilities do not contain a valid Wrangler pin",
        ));
    }
    Ok(TargetCapabilities {
        wrangler_version: capabilities.result.wrangler_version,
    })
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    result: T,
}

#[derive(Deserialize)]
struct AccountResult {
    id: InstanceId,
}

#[derive(Deserialize)]
struct CapabilitiesResult {
    wrangler_version: String,
}

fn valid_version(value: &str) -> bool {
    let mut parts = value.split('.');
    value.len() <= 32
        && (0..3).all(|_| {
            parts.next().is_some_and(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        && parts.next().is_none()
}

async fn collect_body(mut response: reqwest::Response) -> Result<Vec<u8>, PlatformError> {
    let mut output = Vec::new();
    while let Some(data) = response
        .chunk()
        .await
        .map_err(|_| target_unavailable("target response body could not be read"))?
    {
        if output.len().saturating_add(data.len()) > MAX_TARGET_RESPONSE_BYTES {
            return Err(target_unavailable("target response exceeds its size limit"));
        }
        output.extend_from_slice(&data);
    }
    Ok(output)
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn target_unavailable(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::PlatformUnavailable, message)
}

#[cfg(test)]
#[path = "target_http_tests.rs"]
mod tests;
