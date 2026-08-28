//! Versioned P1 release identity shared by capabilities, snapshots, and upgrades.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Complete format and executable identity for one Open Compute release.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformReleaseIdentityV1 {
    /// Format version.
    pub schema_version: u32,
    /// `platformd` semantic version.
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
    /// Version of the checked-in system Worker facade registry.
    pub facade_capability_version: u32,
    /// Current control database schema version.
    pub control_schema_version: u32,
    /// Current scheduler database schema version.
    pub scheduler_schema_version: u32,
    /// Minimum readable KV resource schema version.
    pub kv_schema_version_min: u32,
    /// Maximum readable KV resource schema version.
    pub kv_schema_version_max: u32,
    /// Minimum readable D1 resource schema version.
    pub d1_schema_version_min: u32,
    /// Maximum readable D1 resource schema version.
    pub d1_schema_version_max: u32,
    /// Full platform snapshot format version.
    pub snapshot_format_version: u32,
    /// Digest of the supported compatibility date and flag policy.
    pub compatibility_policy_sha256: String,
}

impl PlatformReleaseIdentityV1 {
    /// Validate fixed-width hashes, schema versions, and required identities.
    pub fn validate(&self) -> bool {
        self.schema_version == 1
            && !self.platform_version.is_empty()
            && !self.git_revision.is_empty()
            && !self.rust_msrv.is_empty()
            && !self.workerd_version.is_empty()
            && is_sha256(&self.workerd_lock_sha256)
            && is_sha256(&self.runtime_assets_sha256)
            && self.facade_capability_version > 0
            && self.control_schema_version > 0
            && self.scheduler_schema_version > 0
            && self.kv_schema_version_min > 0
            && self.kv_schema_version_min <= self.kv_schema_version_max
            && self.d1_schema_version_min > 0
            && self.d1_schema_version_min <= self.d1_schema_version_max
            && self.snapshot_format_version == 1
            && is_sha256(&self.compatibility_policy_sha256)
    }
}

/// One checksummed forward-only migration shipped in a release.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseMigrationV1 {
    /// Monotonic control schema version.
    pub version: u32,
    /// Stable migration name.
    pub name: String,
    /// Build-time SHA-256 of the exact SQL.
    pub sha256: String,
}

/// Machine-readable release metadata derived from the executable's embedded inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformReleaseMetadataV1 {
    /// Metadata format version.
    pub schema_version: u32,
    /// Exact packaged release identity.
    pub release: PlatformReleaseIdentityV1,
    /// Oldest control schema accepted by `upgrade apply`.
    pub upgrade_from_control_schema_min: u32,
    /// Platform semantic versions accepted for a forward upgrade.
    pub upgrade_from_platform_versions: Vec<String>,
    /// Platform semantic versions whose snapshots may be restored directly.
    pub restore_compatible_platform_versions: Vec<String>,
    /// Current project-owned schema tuple.
    pub target_schemas: BTreeMap<String, u32>,
    /// Complete ordered control migration registry.
    pub migrations: Vec<ReleaseMigrationV1>,
    /// Readable immutable object format versions by owner.
    pub readable_object_formats: BTreeMap<String, Vec<u32>>,
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
        let versions_are_valid = |values: &[String]| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| !value.is_empty() && value.len() <= 64)
        };
        self.schema_version == 1
            && self.release.validate()
            && self.upgrade_from_control_schema_min > 0
            && self.upgrade_from_control_schema_min <= self.release.control_schema_version
            && versions_are_valid(&self.upgrade_from_platform_versions)
            && versions_are_valid(&self.restore_compatible_platform_versions)
            && self.target_schemas.get("control").copied()
                == Some(self.release.control_schema_version)
            && self.target_schemas.get("scheduler").copied()
                == Some(self.release.scheduler_schema_version)
            && self.target_schemas.get("kv").copied() == Some(self.release.kv_schema_version_max)
            && self.target_schemas.get("d1").copied() == Some(self.release.d1_schema_version_max)
            && !self.migrations.is_empty()
            && self
                .migrations
                .iter()
                .enumerate()
                .all(|(index, migration)| {
                    migration.version == (index + 1) as u32
                        && !migration.name.is_empty()
                        && is_sha256(&migration.sha256)
                })
            && self.migrations.last().map(|migration| migration.version)
                == Some(self.release.control_schema_version)
            && !self.readable_object_formats.is_empty()
            && self
                .readable_object_formats
                .values()
                .all(|versions| !versions.is_empty() && versions.iter().all(|version| *version > 0))
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
