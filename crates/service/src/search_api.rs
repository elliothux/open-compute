//! Shared Vectorize and AI Search domain authority for the Cloudflare v4 adapters.

use crate::ai_search_backend::AiSearchBindingService;
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
    ai_search: Option<Arc<AiSearchBindingService>>,
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
            ai_search: None,
        }
    }

    /// Attach the single AI Search execution authority used by v4 and bindings.
    #[must_use]
    pub(crate) fn with_ai_search(mut self, service: Arc<AiSearchBindingService>) -> Self {
        self.ai_search = Some(service);
        self
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

    pub(crate) fn ai_search(&self) -> Option<&Arc<AiSearchBindingService>> {
        self.ai_search.as_ref()
    }
}
