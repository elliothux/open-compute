//! Authenticated P1 platform snapshot manifest contract.

use crate::{
    AccountId, ErrorCode, PlatformError, PlatformId, PlatformReleaseIdentityV1, ResourceId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::str::FromStr as _;

/// Role of one local-authority file stored in a full platform snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFileRole {
    /// `control.sqlite` online backup.
    ControlSqlite,
    /// `scheduler.sqlite` online backup.
    SchedulerSqlite,
    /// One Workers KV namespace database.
    KvSqlite,
    /// One Workers D1 database.
    D1Sqlite,
    /// One opaque regular file from the stopped workerd Durable Object tree.
    DurableObjectFile,
}

/// One immutable external S3 object referenced, but not duplicated, by a snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotImmutableReferenceV1 {
    /// Stable reference role.
    pub role: String,
    /// SHA-256 of the referenced bytes.
    pub sha256: String,
    /// Canonical system-prefix object key.
    pub object_key: String,
    /// Exact object size.
    pub size: u64,
}

/// One local file copied into a snapshot-owned object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotFileV1 {
    /// Local authority role.
    pub role: SnapshotFileRole,
    /// Stable control/resource identity used for cross-checks.
    pub logical_id: String,
    /// Canonical data-dir-relative installation path.
    pub restore_path: String,
    /// Canonical snapshot-owned S3 object key.
    pub object_key: String,
    /// Exact byte count.
    pub size: u64,
    /// Lowercase SHA-256 of the object bytes.
    pub sha256: String,
    /// Restrictive owner permission bits.
    pub mode: u32,
}

/// Aggregate manifest counts used for bounded preflight.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTotalsV1 {
    /// Number of snapshot-owned file objects.
    pub files: u32,
    /// Sum of snapshot-owned file bytes.
    pub bytes: u64,
}

/// Committed full-platform snapshot identity and object inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSnapshotManifestV1 {
    /// Manifest schema version.
    pub schema_version: u32,
    /// Canonical `UUIDv7` snapshot identity.
    pub snapshot_id: String,
    /// Stable platform identity.
    pub platform_id: String,
    /// Bounded operator label that never participates in object keys.
    pub label: String,
    /// Audit timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Exact source release and persisted-format identity.
    pub source_release: PlatformReleaseIdentityV1,
    /// Product schema tuple captured during the offline window.
    pub source_schemas: BTreeMap<String, u32>,
    /// Non-secret fingerprint of the required master key.
    pub master_key_fingerprint: String,
    /// SHA-256 fingerprint of endpoint/region/bucket/system/R2 authority.
    pub s3_authority_fingerprint: String,
    /// SHA-256 fingerprint of the configured R2 prefix policy.
    pub r2_prefix_fingerprint: String,
    /// SHA-256 of the redacted storage/product/hardening policy required for restore.
    pub config_policy_sha256: String,
    /// External immutable objects that must remain pinned.
    pub immutable_references: Vec<SnapshotImmutableReferenceV1>,
    /// Local authority objects owned by this snapshot.
    pub files: Vec<SnapshotFileV1>,
    /// Checked aggregate counts.
    pub totals: SnapshotTotalsV1,
    /// Lowercase HMAC-SHA-256 of the canonical unsigned JSON.
    pub manifest_mac: String,
}

#[derive(Serialize)]
struct UnsignedManifest<'a> {
    schema_version: u32,
    snapshot_id: &'a str,
    platform_id: &'a str,
    label: &'a str,
    created_at_ms: i64,
    source_release: &'a PlatformReleaseIdentityV1,
    source_schemas: &'a BTreeMap<String, u32>,
    master_key_fingerprint: &'a str,
    s3_authority_fingerprint: &'a str,
    r2_prefix_fingerprint: &'a str,
    config_policy_sha256: &'a str,
    immutable_references: &'a [SnapshotImmutableReferenceV1],
    files: &'a [SnapshotFileV1],
    totals: SnapshotTotalsV1,
}

impl PlatformSnapshotManifestV1 {
    /// Canonical JSON bytes authenticated by `manifest_mac`.
    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        serde_json::to_vec(&UnsignedManifest {
            schema_version: self.schema_version,
            snapshot_id: &self.snapshot_id,
            platform_id: &self.platform_id,
            label: &self.label,
            created_at_ms: self.created_at_ms,
            source_release: &self.source_release,
            source_schemas: &self.source_schemas,
            master_key_fingerprint: &self.master_key_fingerprint,
            s3_authority_fingerprint: &self.s3_authority_fingerprint,
            r2_prefix_fingerprint: &self.r2_prefix_fingerprint,
            config_policy_sha256: &self.config_policy_sha256,
            immutable_references: &self.immutable_references,
            files: &self.files,
            totals: self.totals,
        })
        .map_err(|_| snapshot_invalid())
    }

    /// Validate format, canonical paths, hashes, uniqueness, and configured hard caps.
    pub fn validate(
        &self,
        max_files: u32,
        max_file_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<(), PlatformError> {
        if self.schema_version != 1
            || !canonical_uuid_v7(&self.snapshot_id)
            || PlatformId::from_str(&self.platform_id).is_err()
            || self.label.is_empty()
            || self.label.len() > 128
            || self.label.bytes().any(|byte| byte.is_ascii_control())
            || self.created_at_ms <= 0
            || !self.source_release.validate()
            || !is_sha256(&self.master_key_fingerprint)
            || !is_sha256(&self.s3_authority_fingerprint)
            || !is_sha256(&self.r2_prefix_fingerprint)
            || !is_sha256(&self.config_policy_sha256)
            || !is_sha256(&self.manifest_mac)
            || self.files.is_empty()
            || self.files.len() > max_files as usize
            || self.immutable_references.len() > max_files as usize
            || self
                .source_schemas
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != ["control", "d1", "kv", "scheduler"]
            || self.source_schemas.get("control").copied()
                != Some(self.source_release.control_schema_version)
            || self.source_schemas.get("scheduler").copied()
                != Some(self.source_release.scheduler_schema_version)
            || self.source_schemas.get("kv").copied() != Some(self.source_release.kv_schema_version)
            || self.source_schemas.get("d1").copied() != Some(self.source_release.d1_schema_version)
        {
            return Err(snapshot_invalid());
        }
        let mut paths = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut control_files = 0_u8;
        let mut scheduler_files = 0_u8;
        let mut total = 0_u64;
        for file in &self.files {
            if file.logical_id.is_empty()
                || file.logical_id.len() > 128
                || !valid_restore_path(&file.restore_path)
                || !valid_object_key(&file.object_key)
                || !is_sha256(&file.sha256)
                || file.size > max_file_bytes
                || !matches!(file.mode, 0o600 | 0o700)
                || !valid_file_role(file, &self.platform_id)
                || !paths.insert(&file.restore_path)
                || !keys.insert(&file.object_key)
            {
                return Err(snapshot_invalid());
            }
            match file.role {
                SnapshotFileRole::ControlSqlite => control_files = control_files.saturating_add(1),
                SnapshotFileRole::SchedulerSqlite => {
                    scheduler_files = scheduler_files.saturating_add(1);
                }
                SnapshotFileRole::KvSqlite
                | SnapshotFileRole::D1Sqlite
                | SnapshotFileRole::DurableObjectFile => {}
            }
            total = total.checked_add(file.size).ok_or_else(snapshot_invalid)?;
            if total > max_total_bytes {
                return Err(snapshot_invalid());
            }
        }
        if control_files != 1
            || scheduler_files != 1
            || self.totals.files != self.files.len() as u32
            || self.totals.bytes != total
        {
            return Err(snapshot_invalid());
        }
        let mut reference_keys = BTreeSet::new();
        for reference in &self.immutable_references {
            if reference.role.is_empty()
                || !matches!(
                    reference.role.as_str(),
                    "deployment_artifact" | "kv_backup" | "d1_backup" | "r2_bucket_marker"
                )
                || !valid_object_key(&reference.object_key)
                || !is_sha256(&reference.sha256)
                || reference.size > max_file_bytes
                || !reference_keys.insert(&reference.object_key)
            {
                return Err(snapshot_invalid());
            }
        }
        Ok(())
    }
}

fn valid_file_role(file: &SnapshotFileV1, platform_id: &str) -> bool {
    let segments = file.restore_path.split('/').collect::<Vec<_>>();
    match file.role {
        SnapshotFileRole::ControlSqlite => {
            file.logical_id == "control" && segments == ["control.sqlite"]
        }
        SnapshotFileRole::SchedulerSqlite => {
            file.logical_id == "scheduler" && segments == ["scheduler.sqlite"]
        }
        SnapshotFileRole::KvSqlite | SnapshotFileRole::D1Sqlite => {
            let product = if file.role == SnapshotFileRole::KvSqlite {
                "kv"
            } else {
                "d1"
            };
            segments.len() == 4
                && segments[0] == product
                && AccountId::from_str(segments[1]).is_ok()
                && ResourceId::from_str(segments[2]).is_ok()
                && segments[2] == file.logical_id
                && segments[3] == "data.sqlite"
        }
        SnapshotFileRole::DurableObjectFile => {
            segments.len() >= 2 && segments[0] == "do" && file.logical_id == platform_id
        }
    }
}

fn valid_object_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('/')
        && !value.bytes().any(|byte| byte.is_ascii_control())
        && !value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
}

/// Validate a data-dir-relative allowlisted restore path without filesystem access.
#[must_use]
pub fn valid_restore_path(value: &str) -> bool {
    if value.is_empty()
        || value.contains('\0')
        || value.contains('\\')
        || value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return false;
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    matches!(
        path.components().next(),
        Some(Component::Normal(root))
            if root == "control.sqlite"
                || root == "scheduler.sqlite"
                || root == "kv"
                || root == "d1"
                || root == "do"
    )
}

fn canonical_uuid_v7(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == value)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn snapshot_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SnapshotInvalid,
        "platform snapshot manifest is invalid",
    )
}

#[cfg(test)]
#[path = "snapshot_manifest_tests.rs"]
mod tests;
