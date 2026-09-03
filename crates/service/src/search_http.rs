//! Shared Vectorize and AI Search domain authority for the Cloudflare v4 adapters.

use open_compute_storage::PlatformStorage;
use open_compute_workers::ResourcePins;
use std::sync::Arc;
use std::time::Duration;

/// Product storage and lifecycle authority used by the official management API.
#[derive(Clone, Debug)]
pub struct SearchApiState {
    storage: Arc<PlatformStorage>,
    pins: ResourcePins,
    busy_timeout_ms: u64,
    delete_drain_timeout: Duration,
}

impl SearchApiState {
    /// Bind product storage and lifecycle authority.
    #[must_use]
    pub const fn new(
        storage: Arc<PlatformStorage>,
        pins: ResourcePins,
        busy_timeout_ms: u64,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            pins,
            busy_timeout_ms,
            delete_drain_timeout,
        }
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    pub(crate) const fn busy_timeout_ms(&self) -> u64 {
        self.busy_timeout_ms
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
    }
}
