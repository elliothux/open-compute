//! Authenticated loopback reader for exact manual AI Search revisions.

use super::*;
use crate::auth::resolve_bearer_auth;
use crate::operator_http::OperatorHttpClient;
use open_compute_core::{AiSourceProviderConfig, SecretString};
use reqwest::{Method, Response, header};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const MAX_RESOLVE_BYTES: usize = 64 * 1024;

#[derive(Clone)]
/// One authenticated loopback provider bound to a fixed source namespace.
pub struct ManualAiSearchSourceReader {
    provider_id: String,
    config: AiSourceProviderConfig,
    credential: SecretString,
    http: OperatorHttpClient,
    timeout: Duration,
}

impl std::fmt::Debug for ManualAiSearchSourceReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManualAiSearchSourceReader")
            .field("provider_id", &self.provider_id)
            .field("source", &self.config.source)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceRequest<'a> {
    source: &'a str,
    key: &'a str,
    revision: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveResponse {
    revision: String,
    content_type: String,
    size: u64,
    sha256: String,
}

/// Exact immutable source metadata returned by `resolve`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualAiSearchResolvedSource {
    /// Provider-owned exact revision.
    pub revision: String,
    /// Canonical content type.
    pub content_type: String,
    /// Exact byte length.
    pub size: u64,
    /// Exact source SHA-256.
    pub sha256: [u8; 32],
}

impl ManualAiSearchSourceReader {
    /// Bind one configured provider and resolve its credential once.
    pub fn new(
        provider_id: String,
        config: AiSourceProviderConfig,
        timeout: Duration,
    ) -> Result<Self, PlatformError> {
        let credential = resolve_bearer_auth(&config.credential)?;
        Ok(Self {
            provider_id,
            config,
            credential,
            http: OperatorHttpClient::from_process_env()?,
            timeout,
        })
    }

    /// Resolve one requested exact revision without reading its bytes.
    pub async fn resolve(
        &self,
        key: &str,
        revision: &str,
    ) -> Result<ManualAiSearchResolvedSource, PlatformError> {
        let response = self
            .request("resolve", key, revision)?
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(unavailable());
        }
        if response.headers().get(header::CONTENT_ENCODING).is_some()
            || response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .map(str::trim)
                != Some("application/json")
        {
            return Err(integrity());
        }
        let bytes = collect(response, MAX_RESOLVE_BYTES).await?;
        let resolved: ResolveResponse = serde_json::from_slice(&bytes).map_err(|_| integrity())?;
        let sha256 = parse_digest(&resolved.sha256)?;
        if resolved.revision != revision
            || resolved.content_type.is_empty()
            || resolved.content_type.len() > 256
            || resolved.content_type.chars().any(char::is_control)
            || resolved.size == 0
            || resolved.size > self.config.max_source_bytes
        {
            return Err(integrity());
        }
        Ok(ManualAiSearchResolvedSource {
            revision: resolved.revision,
            content_type: resolved.content_type,
            size: resolved.size,
            sha256,
        })
    }

    pub(crate) async fn read_exact(
        &self,
        source: &open_compute_storage::AiSearchManualObjectReference,
        key: &str,
        content_type: &str,
    ) -> Result<Vec<u8>, PlatformError> {
        if source.provider_id != self.provider_id
            || source.source != self.config.source
            || source.object_size == 0
            || source.object_size > self.config.max_source_bytes
        {
            return Err(integrity());
        }
        let response = self
            .request("read", key, &source.revision)?
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success()
            || response.headers().get(header::CONTENT_ENCODING).is_some()
            || response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                != Some(content_type)
            || header_text(&response, "x-open-compute-revision") != Some(source.revision.as_str())
            || header_text(&response, "x-open-compute-size")
                .and_then(|value| value.parse::<u64>().ok())
                != Some(source.object_size)
            || header_text(&response, "x-open-compute-sha256")
                .and_then(|value| parse_digest(value).ok())
                != Some(source.sha256)
        {
            return Err(integrity());
        }
        let bytes = collect(
            response,
            usize::try_from(source.object_size).map_err(|_| limit())?,
        )
        .await?;
        if bytes.len() != usize::try_from(source.object_size).map_err(|_| limit())?
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != source.sha256
        {
            return Err(integrity());
        }
        Ok(bytes)
    }

    fn request(
        &self,
        operation: &str,
        key: &str,
        revision: &str,
    ) -> Result<reqwest::RequestBuilder, PlatformError> {
        let mut url = Url::parse(&self.config.endpoint).map_err(|_| integrity())?;
        url.set_path(&format!(
            "{}/{}",
            url.path().trim_end_matches('/'),
            operation
        ));
        let body = serde_json::to_vec(&SourceRequest {
            source: &self.config.source,
            key,
            revision,
        })
        .map_err(|_| integrity())?;
        Ok(self
            .http
            .request(Method::POST, url)?
            .bearer_auth(self.credential.expose())
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONTENT_TYPE, "application/json")
            .timeout(self.timeout)
            .body(body))
    }
}

impl AiSearchSourceReader for ManualAiSearchSourceReader {
    fn read<'a>(
        &'a self,
        claim: &'a AiSearchJobClaim,
    ) -> TaskFuture<'a, Result<AiSearchSourceDocument, PlatformError>> {
        Box::pin(async move {
            let AiSearchSourceReference::Manual(source) = &claim.item.source else {
                return Err(integrity());
            };
            self.read_exact(source, &claim.item.key, &claim.item.content_type)
                .await
                .map(|bytes| AiSearchSourceDocument { bytes })
        })
    }
}

async fn collect(mut response: Response, maximum: usize) -> Result<Vec<u8>, PlatformError> {
    let mut output = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if output
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > maximum)
        {
            return Err(limit());
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn header_text<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

fn parse_digest(value: &str) -> Result<[u8; 32], PlatformError> {
    hex::decode(value)
        .ok()
        .and_then(|value| value.try_into().ok())
        .ok_or_else(integrity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_compute_core::{OperatorProxyPolicy, SecretReference};
    use tokio::io::AsyncWriteExt as _;

    async fn reader_with_response(
        response: String,
        max_source_bytes: u64,
    ) -> (ManualAiSearchSourceReader, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).await.expect("request");
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response");
        });
        (
            ManualAiSearchSourceReader {
                provider_id: "documents".to_owned(),
                config: AiSourceProviderConfig {
                    endpoint: format!("http://{address}/provider"),
                    source: "primary".to_owned(),
                    credential: SecretReference {
                        env: None,
                        file: None,
                    },
                    max_source_bytes,
                },
                credential: SecretString::new("secret"),
                http: OperatorHttpClient::new(
                    OperatorProxyPolicy::from_lookup(|_| None).expect("policy"),
                )
                .expect("client"),
                timeout: Duration::from_millis(200),
            },
            server,
        )
    }

    #[tokio::test]
    async fn manual_provider_resolve_and_read_verify_the_exact_revision() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let digest = hex::encode(Sha256::digest(b"abc"));
        let server_digest = digest.clone();
        let server = tokio::spawn(async move {
            for operation in ["resolve", "read"] {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = vec![0; 4096];
                let count = stream.read(&mut request).await.expect("request");
                let request = std::str::from_utf8(&request[..count]).expect("utf8");
                assert!(request.starts_with(&format!("POST /provider/{operation} HTTP/1.1")));
                assert!(request.contains("authorization: Bearer secret"));
                assert!(request.contains(r#""revision":"rev-1""#));
                let response = if operation == "resolve" {
                    let body = format!(
                        r#"{{"revision":"rev-1","contentType":"text/plain","size":3,"sha256":"{server_digest}"}}"#
                    );
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nx-open-compute-revision: rev-1\r\nx-open-compute-size: 3\r\nx-open-compute-sha256: {server_digest}\r\ncontent-length: 3\r\nconnection: close\r\n\r\nabc"
                    )
                };
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("response");
            }
        });
        let config = AiSourceProviderConfig {
            endpoint: format!("http://{address}/provider"),
            source: "primary".to_owned(),
            credential: SecretReference {
                env: None,
                file: None,
            },
            max_source_bytes: 1024,
        };
        let reader = ManualAiSearchSourceReader {
            provider_id: "documents".to_owned(),
            config,
            credential: SecretString::new("secret"),
            http: OperatorHttpClient::new(
                OperatorProxyPolicy::from_lookup(|_| None).expect("policy"),
            )
            .expect("client"),
            timeout: Duration::from_secs(1),
        };
        let resolved = reader
            .resolve("files/one.txt", "rev-1")
            .await
            .expect("resolve");
        let expected: [u8; 32] = Sha256::digest(b"abc").into();
        assert_eq!(resolved.sha256, expected);
        assert_eq!(
            reader
                .read_exact(
                    &open_compute_storage::AiSearchManualObjectReference {
                        provider_id: "other".to_owned(),
                        source: "primary".to_owned(),
                        revision: "rev-1".to_owned(),
                        sha256: resolved.sha256,
                        object_size: 3,
                    },
                    "files/one.txt",
                    "text/plain",
                )
                .await
                .unwrap_err()
                .code(),
            ErrorCode::ArtifactIntegrityError
        );
        let bytes = reader
            .read_exact(
                &open_compute_storage::AiSearchManualObjectReference {
                    provider_id: "documents".to_owned(),
                    source: "primary".to_owned(),
                    revision: "rev-1".to_owned(),
                    sha256: resolved.sha256,
                    object_size: 3,
                },
                "files/one.txt",
                "text/plain",
            )
            .await
            .expect("read");
        assert_eq!(bytes, b"abc");
        server.await.expect("server");
    }

    #[tokio::test]
    async fn manual_provider_drift_oversize_and_response_loss_fail_closed() {
        let digest = hex::encode(Sha256::digest(b"abc"));
        let drift = format!(
            r#"{{"revision":"rev-2","contentType":"text/plain","size":3,"sha256":"{digest}"}}"#
        );
        let (reader, server) = reader_with_response(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{drift}",
                drift.len()
            ),
            3,
        )
        .await;
        assert_eq!(
            reader.resolve("one", "rev-1").await.unwrap_err().code(),
            ErrorCode::ArtifactIntegrityError
        );
        server.await.expect("drift server");

        let source = open_compute_storage::AiSearchManualObjectReference {
            provider_id: "documents".to_owned(),
            source: "primary".to_owned(),
            revision: "rev-1".to_owned(),
            sha256: Sha256::digest(b"abc").into(),
            object_size: 3,
        };
        let (reader, server) = reader_with_response(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nx-open-compute-revision: rev-1\r\nx-open-compute-size: 3\r\nx-open-compute-sha256: {digest}\r\ncontent-length: 4\r\nconnection: close\r\n\r\nabcd"
            ),
            3,
        )
        .await;
        assert_eq!(
            reader
                .read_exact(&source, "one", "text/plain")
                .await
                .unwrap_err()
                .code(),
            ErrorCode::LimitInvalid
        );
        server.await.expect("oversize server");

        let (reader, server) = reader_with_response(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nx-open-compute-revision: rev-1\r\nx-open-compute-size: 3\r\nx-open-compute-sha256: {digest}\r\ncontent-length: 3\r\nconnection: close\r\n\r\nab"
            ),
            3,
        )
        .await;
        assert_eq!(
            reader
                .read_exact(&source, "one", "text/plain")
                .await
                .unwrap_err()
                .code(),
            ErrorCode::ObjectStorageUnavailable
        );
        server.await.expect("response-loss server");
    }
}
