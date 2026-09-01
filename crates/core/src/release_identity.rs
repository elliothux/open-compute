//! Current release identity shared by capabilities and authenticated snapshots.

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
    /// Current KV resource schema version.
    pub kv_schema_version: u32,
    /// Current D1 resource schema version.
    pub d1_schema_version: u32,
    /// Full platform snapshot format version.
    pub snapshot_format_version: u32,
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
            && self.kv_schema_version > 0
            && self.d1_schema_version > 0
            && self.snapshot_format_version == 1
    }
}

/// One checksummed SQL definition in the current control schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSchemaDefinitionV1 {
    /// Contiguous position in the current schema definition sequence.
    pub version: u32,
    /// Schema definition name.
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
    /// Current project-owned schema tuple.
    pub target_schemas: BTreeMap<String, u32>,
    /// Complete ordered definition of the current control schema.
    pub schema_definitions: Vec<ReleaseSchemaDefinitionV1>,
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
            && self.target_schemas.len() == 4
            && self.target_schemas.get("control").copied()
                == Some(self.release.control_schema_version)
            && self.target_schemas.get("scheduler").copied()
                == Some(self.release.scheduler_schema_version)
            && self.target_schemas.get("kv").copied() == Some(self.release.kv_schema_version)
            && self.target_schemas.get("d1").copied() == Some(self.release.d1_schema_version)
            && !self.schema_definitions.is_empty()
            && self
                .schema_definitions
                .iter()
                .enumerate()
                .all(|(index, definition)| {
                    definition.version == (index + 1) as u32
                        && !definition.name.is_empty()
                        && is_sha256(&definition.sha256)
                })
            && self
                .schema_definitions
                .last()
                .map(|definition| definition.version)
                == Some(self.release.control_schema_version)
            && self
                .object_formats
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                == ["artifacts", "d1_backups", "kv_backups", "r2", "snapshots"]
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
