//! Production AWS S3 client construction.

use crate::credentials::S3Credentials;
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    BehaviorVersion, Region, RequestChecksumCalculation, ResponseChecksumValidation,
    retry::RetryConfig, timeout::TimeoutConfig,
};
use aws_smithy_http_client::Builder as HttpBuilder;
use aws_smithy_http_client::tls::rustls_provider::CryptoMode;
use aws_smithy_http_client::tls::{Provider as TlsProvider, TlsContext, TrustStore};
use open_compute_core::{ErrorCode, PlatformError, S3Config};
use std::time::Duration;

/// Configured production S3 client plus bucket/prefix context.
#[derive(Debug, Clone)]
pub struct S3ArtifactClient {
    inner: Client,
    bucket: String,
    prefix: String,
    max_artifact_bytes: u64,
}

impl S3ArtifactClient {
    /// Build a `SigV4` client from validated config and resolved credentials.
    pub fn connect(
        config: &S3Config,
        credentials: &S3Credentials,
        max_artifact_bytes: u64,
    ) -> Result<Self, PlatformError> {
        if !config.verify_tls {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "s3.verify_tls cannot be disabled",
            ));
        }
        if max_artifact_bytes == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "configured limit must be greater than zero",
            ));
        }
        let creds = Credentials::new(
            credentials.access_key_id().expose(),
            credentials.secret_access_key().expose(),
            None,
            None,
            "open-compute-artifacts",
        );
        let timeout = TimeoutConfig::builder()
            .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
            .operation_timeout(Duration::from_millis(config.request_timeout_ms))
            .build();
        let retry = RetryConfig::standard()
            .with_max_attempts(config.max_retries.saturating_add(1).min(8))
            .with_initial_backoff(Duration::from_millis(config.retry_backoff_ms));
        let conf = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .endpoint_url(&config.endpoint)
            .force_path_style(config.force_path_style)
            .credentials_provider(creds)
            .timeout_config(timeout)
            .retry_config(retry)
            .http_client(build_verified_http_client())
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();
        Ok(Self {
            inner: Client::from_conf(conf),
            bucket: config.bucket.clone(),
            prefix: config.prefix.clone(),
            max_artifact_bytes,
        })
    }

    pub(crate) fn inner(&self) -> &Client {
        &self.inner
    }

    pub(crate) fn bucket(&self) -> &str {
        &self.bucket
    }

    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    pub(crate) fn max_artifact_bytes(&self) -> u64 {
        self.max_artifact_bytes
    }

    /// Read-only signed HEAD of a reserved impossible key.
    ///
    /// Authenticated `404` is success: the client can sign and the bucket exists.
    pub async fn probe_connectivity(&self) -> Result<(), PlatformError> {
        crate::inspect::probe_connectivity(self).await
    }
}

fn build_verified_http_client() -> aws_smithy_runtime_api::client::http::SharedHttpClient {
    let mut trust = TrustStore::empty();
    for cert in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        trust = trust.with_pem_certificate(der_to_pem(cert.as_ref()));
    }
    let tls = TlsContext::builder()
        .with_trust_store(trust)
        .build()
        .unwrap_or_else(|_| {
            TlsContext::builder()
                .with_trust_store(TrustStore::empty())
                .build()
                .unwrap_or_else(|_| {
                    TlsContext::builder()
                        .build()
                        .unwrap_or_else(|_| unreachable!("tls context builder"))
                })
        });
    HttpBuilder::new()
        .tls_provider(TlsProvider::Rustls(CryptoMode::AwsLc))
        .tls_context(tls)
        .build_https()
}

fn der_to_pem(der: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = b"-----BEGIN CERTIFICATE-----\n".to_vec();
    for chunk in encoded.as_bytes().chunks(64) {
        pem.extend_from_slice(chunk);
        pem.push(b'\n');
    }
    pem.extend_from_slice(b"-----END CERTIFICATE-----\n");
    pem
}
