//! Workers KV composition state shared by the v4 API and runtime bindings.

use crate::kv_backend::SqliteKvBindingExecutor;
use crate::snapshot_pins::SnapshotPins;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, KvConfig, PlatformError, RequestId, ResourceId,
    ResourceState,
};
use open_compute_storage::{
    KvBackupState, KvEngine, KvNamespaceRepository, KvPaths, PlatformStorage,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use open_compute_workers::{CreateResourceOutcome, CreateResourceResult, ResourcePins};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::sync::Arc;
use std::time::Duration;

const KV_BACKUP_MANIFEST_SCHEMA: u32 = 1;

#[path = "kv_backup.rs"]
pub(crate) mod backup;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KvBackupManifest {
    backup_schema: u32,
    backup_id: String,
    source_resource_id: ResourceId,
    kv_schema_version: u32,
    sha256: String,
    size_bytes: u64,
    created_at_ms: i64,
}

/// Shared Workers KV composition state.
#[derive(Clone)]
pub struct KvApiState {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    pins: ResourcePins,
    executor: Arc<SqliteKvBindingExecutor>,
    config: KvConfig,
    max_resources_per_account: u32,
    delete_drain_timeout: Duration,
    snapshot_pins: Arc<SnapshotPins>,
}

impl std::fmt::Debug for KvApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KvApiState")
            .field("artifacts", &self.artifacts)
            .field("pins", &self.pins)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl KvApiState {
    /// Bind central authority, object-backed backup storage, pins, and product limits.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        pins: ResourcePins,
        executor: Arc<SqliteKvBindingExecutor>,
        config: KvConfig,
        max_resources_per_account: u32,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            pins,
            executor,
            config,
            max_resources_per_account,
            delete_drain_timeout,
            snapshot_pins: Arc::new(SnapshotPins::empty()),
        }
    }

    /// Use the authenticated immutable-object pins frozen at daemon startup.
    #[must_use]
    pub(crate) fn with_snapshot_pins(mut self, pins: Arc<SnapshotPins>) -> Self {
        self.snapshot_pins = pins;
        self
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    pub(crate) fn executor(&self) -> &Arc<SqliteKvBindingExecutor> {
        &self.executor
    }

    pub(crate) const fn config(&self) -> &KvConfig {
        &self.config
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
    }
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "KV management operation failed")
}
