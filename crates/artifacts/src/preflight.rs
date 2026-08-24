//! Startup S3 preflight using the production `SigV4` client.

use crate::client::S3ArtifactClient;
use crate::error::{self, S3Stage};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::primitives::ByteStream;
use open_compute_core::{ErrorCode, PlatformError, PlatformId, StartupId};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::fmt::{Debug, Formatter};

const META_SHA256: &str = "sha256";

/// Successful preflight. Contains no object keys or secrets.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PreflightOutcome {
    payload_bytes: usize,
    puts: u8,
    heads: u8,
    gets: u8,
    deletes: u8,
}

impl PreflightOutcome {
    /// Number of bytes written during preflight.
    #[must_use]
    pub const fn payload_bytes(self) -> usize {
        self.payload_bytes
    }

    /// Successful PUT operations completed.
    #[must_use]
    pub const fn puts(self) -> u8 {
        self.puts
    }

    /// Successful HEAD operations completed.
    #[must_use]
    pub const fn heads(self) -> u8 {
        self.heads
    }

    /// Successful GET operations completed.
    #[must_use]
    pub const fn gets(self) -> u8 {
        self.gets
    }

    /// Successful DELETE operations completed.
    #[must_use]
    pub const fn deletes(self) -> u8 {
        self.deletes
    }

    /// Fixture for metrics tests: PUT/HEAD/GET/DELETE/HEAD.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub const fn successful_canary() -> Self {
        Self {
            payload_bytes: 32,
            puts: 1,
            heads: 2,
            gets: 1,
            deletes: 1,
        }
    }
}

impl Debug for PreflightOutcome {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreflightOutcome")
            .field("payload_bytes", &self.payload_bytes)
            .field("puts", &self.puts)
            .field("heads", &self.heads)
            .field("gets", &self.gets)
            .field("deletes", &self.deletes)
            .finish()
    }
}

/// Run PUT/HEAD/GET/DELETE/HEAD preflight under the internal prefix.
pub async fn preflight_s3(
    client: &S3ArtifactClient,
    platform_id: PlatformId,
    startup_id: StartupId,
) -> Result<PreflightOutcome, PlatformError> {
    let mut nonce = [0_u8; 16];
    rand::rng().fill(&mut nonce);
    let nonce_hex = hex::encode(nonce);
    let key = format!(
        "{}preflight/{platform_id}/{startup_id}/{nonce_hex}",
        client.prefix()
    );
    let mut payload = [0_u8; 32];
    rand::rng().fill(&mut payload);
    let digest = hex::encode(Sha256::digest(payload));

    let result = run_stages(client, &key, &payload, &digest).await;
    if result.is_err() {
        let _ = client
            .inner()
            .delete_object()
            .bucket(client.bucket())
            .key(&key)
            .send()
            .await;
    }
    result
}

async fn run_stages(
    client: &S3ArtifactClient,
    key: &str,
    payload: &[u8],
    digest: &str,
) -> Result<PreflightOutcome, PlatformError> {
    client
        .inner()
        .put_object()
        .bucket(client.bucket())
        .key(key)
        .body(ByteStream::from(payload.to_vec()))
        .content_length(payload.len() as i64)
        .metadata(META_SHA256, digest)
        .send()
        .await
        .map_err(|err| error::from_put(&err))?;

    let head = client
        .inner()
        .head_object()
        .bucket(client.bucket())
        .key(key)
        .send()
        .await
        .map_err(|err| error::from_head(&err))?;
    let len = u64::try_from(head.content_length().unwrap_or(0)).unwrap_or(0);
    if len != payload.len() as u64 {
        return Err(error::integrity_error());
    }
    let meta = head.metadata().and_then(|m| m.get(META_SHA256).cloned());
    if meta.as_deref() != Some(digest) {
        return Err(error::integrity_error());
    }

    let got = client
        .inner()
        .get_object()
        .bucket(client.bucket())
        .key(key)
        .send()
        .await
        .map_err(|err| error::from_get(&err))?;
    let body = got
        .body
        .collect()
        .await
        .map_err(|_| error::unavailable(S3Stage::Server))?
        .into_bytes();
    let got_digest = hex::encode(Sha256::digest(&body));
    if body.as_ref() != payload || got_digest != digest {
        return Err(error::integrity_error());
    }

    client
        .inner()
        .delete_object()
        .bucket(client.bucket())
        .key(key)
        .send()
        .await
        .map_err(|err| error::from_delete(&err))?;

    match client
        .inner()
        .head_object()
        .bucket(client.bucket())
        .key(key)
        .send()
        .await
    {
        Err(SdkError::ServiceError(svc)) if svc.raw().status().as_u16() == 404 => {}
        Err(err) => {
            if !error::is_not_found(&error::from_head(&err)) {
                return Err(error::unavailable(S3Stage::Delete));
            }
        }
        Ok(_) => {
            return Err(PlatformError::new(
                ErrorCode::S3Unavailable,
                "s3 object delete failed",
            ));
        }
    }

    Ok(PreflightOutcome {
        payload_bytes: payload.len(),
        puts: 1,
        heads: 2,
        gets: 1,
        deletes: 1,
    })
}
