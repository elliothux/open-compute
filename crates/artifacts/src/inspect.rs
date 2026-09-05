//! Read-only object-storage and cache inspection.

use crate::backend::{BackendError, HeadOptions, ObjectBackend, ObjectKey};
use crate::cache::ArtifactCache;
use crate::error;
use open_compute_core::PlatformError;

const IMPOSSIBLE_LEAF: &str = "__open_compute_connectivity_probe";

/// Read an impossible reserved object. Absence proves the authority is reachable.
pub async fn probe_object_storage(backend: &ObjectBackend) -> Result<(), PlatformError> {
    let key = ObjectKey::new(format!("{}{IMPOSSIBLE_LEAF}", backend.prefix()))
        .map_err(error::from_backend)?;
    match backend.head(&key, HeadOptions::default()).await {
        Ok(_) | Err(BackendError::NotFound) => Ok(()),
        Err(failure) => Err(error::from_backend(failure)),
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
