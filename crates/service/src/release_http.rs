//! Bounded HTTP fetch for formal release metadata and binaries.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, StatusCode, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use open_compute_core::{ErrorCode, PlatformError};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default GitHub download base (`…/releases/download`).
pub const DEFAULT_RELEASE_DOWNLOAD_BASE: &str =
    "https://github.com/elliothux/open-compute/releases/download";

/// Default GitHub API base for resolving the latest stable tag.
pub const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";

/// Bound for release.json / SHA256SUMS / API metadata responses.
pub const MAX_METADATA_BYTES: usize = 256 * 1024;

/// Bound for a single `ocd` release binary download.
pub const MAX_BINARY_BYTES: usize = 256 * 1024 * 1024;

/// Per-request HTTP timeout for release metadata and binary fetches.
pub const RELEASE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, PlatformError>> + Send + 'a>>;

/// Bounded HTTP fetch used by upgrade and update-check (injectable in tests).
pub trait ReleaseHttp: Send + Sync {
    /// GET `url` and return at most `max_bytes` response bytes.
    fn get(&self, url: &str, max_bytes: usize) -> FetchFuture<'_>;
}

/// Production HTTPS/HTTP client with strict timeouts and size limits.
#[derive(Clone, Debug)]
pub struct LiveReleaseHttp {
    client: Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>,
    timeout: Duration,
    user_agent: String,
}

impl LiveReleaseHttp {
    /// Build a client with webpki roots and the production timeout.
    pub fn new() -> Result<Self, PlatformError> {
        Self::with_timeout(RELEASE_HTTP_TIMEOUT)
    }

    /// Build a client with an explicit per-request timeout (tests inject short values).
    pub fn with_timeout(timeout: Duration) -> Result<Self, PlatformError> {
        crate::tls::install_default_provider();
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            timeout,
            user_agent: format!("open-compute-ocd/{}", env!("CARGO_PKG_VERSION")),
        })
    }
}

impl ReleaseHttp for LiveReleaseHttp {
    fn get(&self, url: &str, max_bytes: usize) -> FetchFuture<'_> {
        let url = url.to_owned();
        Box::pin(async move {
            tokio::time::timeout(self.timeout, async {
                let mut current = url::Url::parse(&url).map_err(|_| {
                    PlatformError::new(ErrorCode::ReleaseUnsupported, "release URL is invalid")
                })?;
                for redirects in 0..=MAX_REDIRECTS {
                    if !matches!(current.scheme(), "https" | "http") {
                        return Err(PlatformError::new(
                            ErrorCode::ReleaseUnsupported,
                            "release URL must use HTTP or HTTPS",
                        ));
                    }
                    let uri: Uri = current.as_str().parse().map_err(|_| {
                        PlatformError::new(ErrorCode::ReleaseUnsupported, "release URL is invalid")
                    })?;
                    let request = Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .header("user-agent", &self.user_agent)
                        .header("accept", "application/octet-stream, application/json")
                        .body(Full::new(Bytes::new()))
                        .map_err(|_| {
                            PlatformError::new(
                                ErrorCode::Internal,
                                "failed to build release HTTP request",
                            )
                        })?;
                    let response = self.client.request(request).await.map_err(|_| {
                        PlatformError::new(
                            ErrorCode::PlatformUnavailable,
                            "release HTTP request failed",
                        )
                    })?;
                    if is_redirect(response.status()) {
                        if redirects == MAX_REDIRECTS {
                            return Err(PlatformError::new(
                                ErrorCode::PlatformUnavailable,
                                "release HTTP redirect limit exceeded",
                            ));
                        }
                        let location = response
                            .headers()
                            .get(hyper::header::LOCATION)
                            .and_then(|value| value.to_str().ok())
                            .ok_or_else(|| {
                                PlatformError::new(
                                    ErrorCode::PlatformUnavailable,
                                    "release HTTP redirect has no valid location",
                                )
                            })?;
                        let next = current.join(location).map_err(|_| {
                            PlatformError::new(
                                ErrorCode::ReleaseUnsupported,
                                "release HTTP redirect location is invalid",
                            )
                        })?;
                        if current.scheme() == "https" && next.scheme() != "https" {
                            return Err(PlatformError::new(
                                ErrorCode::ReleaseUnsupported,
                                "release HTTP redirect must not downgrade HTTPS",
                            ));
                        }
                        current = next;
                        continue;
                    }
                    if !response.status().is_success() {
                        return Err(PlatformError::new(
                            ErrorCode::PlatformUnavailable,
                            "release HTTP request returned a non-success status",
                        ));
                    }
                    return collect_body(response.into_body(), max_bytes).await;
                }
                Err(PlatformError::new(
                    ErrorCode::PlatformUnavailable,
                    "release HTTP redirect limit exceeded",
                ))
            })
            .await
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::PlatformUnavailable,
                    "release HTTP request timed out",
                )
            })?
        })
    }
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

/// In-memory fixture HTTP client for unit tests (no real network).
#[derive(Clone, Debug, Default)]
pub struct FixtureReleaseHttp {
    inner: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl FixtureReleaseHttp {
    /// Insert a successful response body for an exact URL.
    pub fn insert(&self, url: impl Into<String>, body: impl Into<Vec<u8>>) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(url.into(), body.into());
        }
    }
}

impl ReleaseHttp for FixtureReleaseHttp {
    fn get(&self, url: &str, max_bytes: usize) -> FetchFuture<'_> {
        let url = url.to_owned();
        Box::pin(async move {
            let body = self
                .inner
                .lock()
                .map_err(|_| PlatformError::new(ErrorCode::Internal, "fixture HTTP lock poisoned"))?
                .get(&url)
                .cloned()
                .ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::PlatformUnavailable,
                        "fixture HTTP has no response for the requested URL",
                    )
                })?;
            if body.len() > max_bytes {
                return Err(PlatformError::new(
                    ErrorCode::LimitInvalid,
                    "fixture HTTP response exceeds the size bound",
                ));
            }
            Ok(body)
        })
    }
}

async fn collect_body(body: Incoming, max_bytes: usize) -> Result<Vec<u8>, PlatformError> {
    let mut out = Vec::new();
    let mut body = body;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "failed to read release HTTP body",
            )
        })?;
        if let Some(data) = frame.data_ref() {
            if out.len().saturating_add(data.len()) > max_bytes {
                return Err(PlatformError::new(
                    ErrorCode::LimitInvalid,
                    "release HTTP body exceeds the size bound",
                ));
            }
            out.extend_from_slice(data);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "release_http_tests.rs"]
mod tests;
