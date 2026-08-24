//! Read-only S3 connectivity and cache sampling.

use crate::cache::ArtifactCache;
use crate::client::S3ArtifactClient;
use crate::error;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use open_compute_core::PlatformError;

const IMPOSSIBLE_LEAF: &str = "__open_compute_connectivity_probe";

/// HEAD a reserved impossible object. Authenticated 404 is success.
pub(crate) async fn probe_connectivity(client: &S3ArtifactClient) -> Result<(), PlatformError> {
    let key = format!("{}{IMPOSSIBLE_LEAF}", client.prefix());
    match client
        .inner()
        .head_object()
        .bucket(client.bucket())
        .key(&key)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(err) if head_is_not_found(&err) => Ok(()),
        Err(err) => Err(error::from_head(&err)),
    }
}

fn head_is_not_found(
    err: &SdkError<HeadObjectError, aws_smithy_runtime_api::client::orchestrator::HttpResponse>,
) -> bool {
    match err {
        SdkError::ServiceError(svc) => {
            matches!(svc.err(), HeadObjectError::NotFound(_)) || svc.raw().status().as_u16() == 404
        }
        SdkError::ResponseError(resp) => resp.raw().status().as_u16() == 404,
        _ => false,
    }
}

/// Cache sample result. Never mutates entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheSample {
    /// Number of indexed entries.
    pub entries: u64,
    /// Tracked byte total.
    pub bytes: u64,
    /// Whether any sampled entry failed integrity verification.
    pub corrupt: bool,
}

/// Hash a bounded sample of cache entries without quarantine or LRU updates.
pub fn sample_cache_integrity(cache: &ArtifactCache) -> Result<CacheSample, PlatformError> {
    cache.sample_integrity()
}
