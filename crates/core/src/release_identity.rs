//! Current release identity shared by capabilities and authenticated snapshots.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Complete executable and public-format identity for one Open Compute release.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformReleaseIdentityV1 {
    /// Format version.
    pub schema_version: u32,
    /// `ocd` semantic version.
    pub platform_version: String,
    /// Source revision embedded by the build, or `unknown` for an unversioned local build.
    pub git_revision: String,
    /// Workspace Rust MSRV.
    pub rust_msrv: String,
    /// Exact workerd version output from the formal lock.
    pub workerd_version: String,
    /// SHA-256 of the exact `workerd.lock.json` bytes.
    pub workerd_lock_sha256: String,
    /// SHA-256 of packaged runtime assets.
    pub runtime_assets_sha256: String,
    /// SHA-256 of embedded operator dashboard static assets.
    pub dashboard_assets_sha256: String,
    /// Version of the checked-in system Worker facade registry.
    pub facade_capability_version: u32,
    /// Full platform snapshot format version.
    pub snapshot_format_version: u32,
}

impl PlatformReleaseIdentityV1 {
    /// Validate fixed-width hashes, format versions, and required identities.
    pub fn validate(&self) -> bool {
        self.schema_version == 1
            && !self.platform_version.is_empty()
            && !self.git_revision.is_empty()
            && !self.rust_msrv.is_empty()
            && !self.workerd_version.is_empty()
            && is_sha256(&self.workerd_lock_sha256)
            && is_sha256(&self.runtime_assets_sha256)
            && is_sha256(&self.dashboard_assets_sha256)
            && self.facade_capability_version > 0
            && self.snapshot_format_version == 1
    }
}

/// Machine-readable release metadata derived from the executable's embedded inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformReleaseMetadataV1 {
    /// Metadata format version.
    pub schema_version: u32,
    /// Exact packaged release identity.
    pub release: PlatformReleaseIdentityV1,
    /// Single current immutable object format version for each owner.
    pub object_formats: BTreeMap<String, u32>,
    /// Stock-workerd local-disk compatibility Gate result identity.
    pub workerd_local_disk_gate_result: String,
    /// Capability/conformance result identity.
    pub conformance_result: String,
    /// Conditional WebSocket hibernation Gate verdict.
    pub websocket_hibernation_result: String,
}

impl PlatformReleaseMetadataV1 {
    /// Validate the release contract without consulting runtime state.
    pub fn validate(&self) -> bool {
        self.schema_version == 1
            && self.release.validate()
            && self
                .object_formats
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                == [
                    "ai_search_objects",
                    "artifacts",
                    "d1_backups",
                    "kv_backups",
                    "r2",
                    "snapshots",
                ]
            && self.object_formats.values().all(|version| *version > 0)
            && self.object_formats.get("snapshots").copied()
                == Some(self.release.snapshot_format_version)
            && !self.workerd_local_disk_gate_result.is_empty()
            && !self.conformance_result.is_empty()
            && !self.websocket_hibernation_result.is_empty()
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
#[path = "release_identity_tests.rs"]
mod tests;
