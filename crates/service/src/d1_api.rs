//! Shared D1 service composition without an HTTP transport contract.

use crate::d1_backend::D1BindingService;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::D1Config;
use open_compute_storage::PlatformStorage;
use open_compute_workers::ResourcePins;
use std::sync::Arc;
use std::time::Duration;

/// Shared D1 domain composition consumed by current API adapters.
#[derive(Clone)]
pub struct D1ApiState {
    pub(crate) storage: Arc<PlatformStorage>,
    pub(crate) artifacts: ArtifactStore,
    pins: ResourcePins,
    pub(crate) backend: Arc<D1BindingService>,
    pub(crate) config: D1Config,
    pub(crate) max_resources_per_account: u32,
    delete_drain_timeout: Duration,
}

impl std::fmt::Debug for D1ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("D1ApiState")
            .field("artifacts", &self.artifacts)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl D1ApiState {
    /// Bind central authority, system backup storage, shared operation lanes, and limits.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        pins: ResourcePins,
        backend: Arc<D1BindingService>,
        config: D1Config,
        max_resources_per_account: u32,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            pins,
            backend,
            config,
            max_resources_per_account,
            delete_drain_timeout,
        }
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    /// Borrow the one shared D1 coordinator-backed service.
    #[must_use]
    pub(crate) fn backend(&self) -> &Arc<D1BindingService> {
        &self.backend
    }

    pub(crate) const fn config(&self) -> &D1Config {
        &self.config
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
    }
}
