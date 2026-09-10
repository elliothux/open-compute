//! HTTPS transport with DNS answers pinned for the complete Git import.

use gix_features::io::pipe;
use gix_transport::client::blocking_io::http::{
    Error, GetResponse, Http, PostBodyDataKind, PostResponse,
};
use reqwest::blocking::{Body, Client};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::io::{Read as _, Write as _};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) struct ValidatedRemote {
    pub(super) host: String,
    pub(super) addresses: Vec<SocketAddr>,
}

pub(super) fn canonical_public_https_remote(
    remote: &str,
) -> Result<String, open_compute_core::PlatformError> {
    let parsed = url::Url::parse(remote).map_err(|_| remote_invalid())?;
    let host = parsed.host().ok_or_else(remote_invalid)?;
    if parsed.scheme() != "https"
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port_or_known_default() != Some(443)
        || match host {
            url::Host::Domain(_) => false,
            url::Host::Ipv4(ip) => !public_ip(IpAddr::V4(ip)),
            url::Host::Ipv6(ip) => !public_ip(IpAddr::V6(ip)),
        }
    {
        return Err(remote_invalid());
    }
    Ok(parsed.to_string())
}

pub(super) fn validate_public_remote(
    remote: &str,
) -> Result<ValidatedRemote, open_compute_core::PlatformError> {
    let parsed =
        url::Url::parse(&canonical_public_https_remote(remote)?).map_err(|_| remote_invalid())?;
    let host = parsed.host_str().ok_or_else(remote_invalid)?;
    let addresses = (host, 443)
        .to_socket_addrs()
        .map_err(|_| upstream_unavailable())?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(remote_invalid());
    }
    Ok(ValidatedRemote {
        host: host.to_owned(),
        addresses,
    })
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            let segments = ip.segments();
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && segments[1] <= 0x01ff)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
                && segments[0] & 0xfff0 != 0x3ff0
        }
    }
}

#[derive(Clone)]
pub(super) struct PinnedHttp {
    client: Client,
    host: String,
    max_response_bytes: u64,
    failure: Arc<Mutex<Option<RemoteFailure>>>,
}

#[derive(Clone, Copy)]
enum RemoteFailure {
    InvalidUrl,
    AuthenticationRequired,
    NotFound,
    UpstreamUnavailable,
    MemoryLimit,
}

impl PinnedHttp {
    pub(super) fn new(
        host: String,
        addresses: &[SocketAddr],
        max_response_bytes: u64,
        timeout: Duration,
    ) -> Result<Self, open_compute_core::PlatformError> {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout)
            .timeout(timeout)
            .resolve_to_addrs(&host, addresses)
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            client,
            host,
            max_response_bytes,
            failure: Arc::new(Mutex::new(None)),
        })
    }

    pub(super) fn failure_error(&self) -> open_compute_core::PlatformError {
        let failure = self
            .failure
            .lock()
            .ok()
            .and_then(|failure| *failure)
            .unwrap_or(RemoteFailure::InvalidUrl);
        match failure {
            RemoteFailure::InvalidUrl => invalid_git_remote(),
            RemoteFailure::AuthenticationRequired => remote_auth_required(),
            RemoteFailure::NotFound => remote_not_found(),
            RemoteFailure::UpstreamUnavailable => upstream_unavailable(),
            RemoteFailure::MemoryLimit => import_memory_limit(),
        }
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        body: Option<pipe::Reader>,
    ) -> Result<(pipe::Reader, pipe::Reader), Error> {
        let url = reqwest::Url::parse(url).map_err(detail)?;
        if url.scheme() != "https"
            || url.host_str() != Some(&self.host)
            || url.port_or_known_default() != Some(443)
        {
            return Err(detail("Artifact import transport target changed"));
        }
        let mut header_map = HeaderMap::new();
        for line in headers {
            let (name, value) = line
                .as_ref()
                .split_once(':')
                .ok_or_else(|| detail("Invalid Git HTTP header"))?;
            let name = HeaderName::try_from(name).map_err(detail)?;
            let value = HeaderValue::try_from(value.trim()).map_err(detail)?;
            header_map.insert(name, value);
        }
        let (mut headers_tx, headers_rx) = pipe::unidirectional(0);
        let (mut body_tx, body_rx) = pipe::unidirectional(0);
        let client = self.client.clone();
        let maximum = self.max_response_bytes;
        let failure = Arc::clone(&self.failure);
        std::thread::spawn(move || {
            let mut request = client.request(method, url).headers(header_map);
            if let Some(body) = body {
                request = request.body(Body::new(body));
            }
            let mut response = match request.send() {
                Ok(response) if response.status().is_success() => response,
                Ok(response) => {
                    record_failure(&failure, classify_status(response.status()));
                    send_error(&headers_tx, &body_tx, "Artifact import HTTP error");
                    return;
                }
                Err(_) => {
                    record_failure(&failure, RemoteFailure::UpstreamUnavailable);
                    send_error(&headers_tx, &body_tx, "Artifact import transport error");
                    return;
                }
            };
            for (name, value) in response.headers() {
                if headers_tx.write_all(name.as_str().as_bytes()).is_err()
                    || headers_tx.write_all(b":").is_err()
                    || headers_tx.write_all(value.as_bytes()).is_err()
                    || headers_tx.write_all(b"\n").is_err()
                {
                    return;
                }
            }
            drop(headers_tx);
            let mut sent = 0_u64;
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let read = match response.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) => {
                        record_failure(&failure, RemoteFailure::UpstreamUnavailable);
                        let _ = body_tx.channel.send(Err(error));
                        return;
                    }
                };
                sent = sent.saturating_add(read as u64);
                if sent > maximum {
                    record_failure(&failure, RemoteFailure::MemoryLimit);
                    let _ = body_tx.channel.send(Err(std::io::Error::other(
                        "Artifact import response exceeded its byte limit",
                    )));
                    return;
                }
                if body_tx.write_all(&buffer[..read]).is_err() {
                    return;
                }
            }
        });
        Ok((headers_rx, body_rx))
    }
}

impl Http for PinnedHttp {
    type Headers = pipe::Reader;
    type ResponseBody = pipe::Reader;
    type PostBody = pipe::Writer;

    fn get(
        &mut self,
        url: &str,
        _base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<GetResponse<Self::Headers, Self::ResponseBody>, Error> {
        let (headers, body) = self.request(reqwest::Method::GET, url, headers, None)?;
        Ok(GetResponse { headers, body })
    }

    fn post(
        &mut self,
        url: &str,
        _base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        _kind: PostBodyDataKind,
    ) -> Result<PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>, Error> {
        let (post_body, body_rx) = pipe::unidirectional(0);
        let (headers, body) = self.request(reqwest::Method::POST, url, headers, Some(body_rx))?;
        Ok(PostResponse {
            post_body,
            headers,
            body,
        })
    }

    fn configure(
        &mut self,
        _config: &dyn std::any::Any,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        Ok(())
    }
}

fn classify_status(status: reqwest::StatusCode) -> RemoteFailure {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            RemoteFailure::AuthenticationRequired
        }
        reqwest::StatusCode::NOT_FOUND => RemoteFailure::NotFound,
        status if status.is_server_error() => RemoteFailure::UpstreamUnavailable,
        _ => RemoteFailure::InvalidUrl,
    }
}

fn record_failure(failure: &Mutex<Option<RemoteFailure>>, value: RemoteFailure) {
    if let Ok(mut failure) = failure.lock() {
        *failure = Some(value);
    }
}

fn send_error(headers: &pipe::Writer, body: &pipe::Writer, message: &'static str) {
    let kind = std::io::ErrorKind::ConnectionAborted;
    let _ = headers
        .channel
        .send(Err(std::io::Error::new(kind, message)));
    let _ = body.channel.send(Err(std::io::Error::new(kind, message)));
}

fn detail(error: impl std::fmt::Display) -> Error {
    Error::Detail {
        description: error.to_string(),
    }
}

fn unavailable() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::ResourceUnavailable,
        "Artifact import HTTPS transport is unavailable",
    )
}

pub(super) fn upstream_unavailable() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::ResourceUnavailable,
        "Artifact import upstream is unavailable",
    )
}

pub(super) fn remote_invalid() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::PathInvalid,
        "Artifact import URL is invalid",
    )
}

fn invalid_git_remote() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::ArtifactUnavailable,
        "Artifact import URL is not a Git repository",
    )
}

fn remote_auth_required() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::BindingPermissionDenied,
        "Artifact import remote requires authentication",
    )
}

fn remote_not_found() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::ResourceNotFound,
        "Artifact import remote was not found",
    )
}

fn import_memory_limit() -> open_compute_core::PlatformError {
    open_compute_core::PlatformError::new(
        open_compute_core::ErrorCode::ResourceLimitExceeded,
        "Artifact import response exceeded its byte limit",
    )
}

#[cfg(test)]
#[path = "import_http_tests.rs"]
mod tests;
