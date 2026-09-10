//! Persisted Cloudflare Artifacts catalog and binding records.

use open_compute_core::{
    AccountId, ArtifactRepoId, ArtifactTokenId, BindingId, CanonicalPermissions, ErrorCode,
    PlatformError, ResourceId, VersionId,
};
use serde::Serialize;
use std::str::FromStr;

/// Current local Git repository schema.
pub const ARTIFACT_REPOSITORY_SCHEMA_VERSION: u32 = 1;
/// Frozen Day 1 repository count per namespace.
pub const ARTIFACT_MAX_REPOSITORIES: u32 = 1_000;
/// Frozen Day 1 active token count per repository.
pub const ARTIFACT_MAX_TOKENS_PER_REPOSITORY: u32 = 32;

/// Account-scoped Cloudflare Artifacts namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactNamespaceRecord {
    /// Stable namespace identity used by immutable Worker bindings.
    pub id: ResourceId,
    /// Owning account.
    pub account_id: AccountId,
    /// Wrangler-facing namespace name.
    pub name: String,
    /// Optional Cloudflare jurisdiction token.
    pub jurisdiction: Option<String>,
    /// Frozen repository quota.
    pub max_repositories: u32,
    /// Frozen active-token quota per repository.
    pub max_tokens_per_repository: u32,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last metadata mutation timestamp.
    pub updated_at_ms: i64,
}

/// Durable Git repository catalog row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRepositoryRecord {
    /// Stable repository identity and on-disk directory key.
    pub id: ArtifactRepoId,
    /// Owning namespace.
    pub namespace_id: ResourceId,
    /// Namespace-local repository name.
    pub name: String,
    /// User-facing description.
    pub description: String,
    /// Symbolic HEAD branch.
    pub default_branch: String,
    /// Durable lifecycle state.
    pub state: ArtifactRepositoryState,
    /// Whether mutations are fenced.
    pub read_only: bool,
    /// Optional import source URL retained as metadata.
    pub source: Option<String>,
    /// Mutation-fencing generation.
    pub generation: u64,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last metadata mutation timestamp.
    pub updated_at_ms: i64,
    /// Last successful Git push timestamp.
    pub last_push_at_ms: Option<i64>,
    /// Deletion completion timestamp for retained tombstones.
    pub deleted_at_ms: Option<i64>,
}

/// Repository lifecycle persisted before filesystem mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRepositoryState {
    /// Catalog reservation exists and filesystem initialization must converge.
    Creating,
    /// A public remote clone is in progress and cannot be served yet.
    Importing,
    /// An independent repository copy is in progress and cannot be served yet.
    Forking,
    /// Repository is available.
    Ready,
    /// New operations are fenced while deletion converges.
    Deleting,
    /// Creation or integrity verification failed closed.
    Failed,
    /// Filesystem authority was removed and only an identity tombstone remains.
    Tombstoned,
}

impl FromStr for ArtifactRepositoryState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "creating" => Ok(Self::Creating),
            "importing" => Ok(Self::Importing),
            "forking" => Ok(Self::Forking),
            "ready" => Ok(Self::Ready),
            "deleting" => Ok(Self::Deleting),
            "failed" => Ok(Self::Failed),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

impl ArtifactRepositoryState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Importing => "importing",
            Self::Forking => "forking",
            Self::Ready => "ready",
            Self::Deleting => "deleting",
            Self::Failed => "failed",
            Self::Tombstoned => "tombstoned",
        }
    }
}

/// Repository access token metadata. Plaintext is never persisted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactTokenRecord {
    /// Stable opaque token identity retained only in metadata.
    pub id: ArtifactTokenId,
    /// Authorized repository.
    pub repository_id: ArtifactRepoId,
    /// Read or write scope.
    pub scope: ArtifactTokenScope,
    /// Expiry timestamp.
    pub expires_at_ms: i64,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Revocation timestamp.
    pub revoked_at_ms: Option<i64>,
}

/// Artifact repository token scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactTokenScope {
    /// Clone, fetch, and content reads.
    Read,
    /// Read plus push and repository mutation.
    Write,
}

impl ArtifactTokenScope {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// Validated inputs used to reserve one new Artifact repository.
#[derive(Clone, Copy, Debug)]
pub struct NewArtifactRepository<'a> {
    /// Public repository name.
    pub name: &'a str,
    /// Optional human-readable description.
    pub description: &'a str,
    /// Initial default branch.
    pub default_branch: &'a str,
    /// Whether pushes are forbidden.
    pub read_only: bool,
    /// Sanitized import or fork source.
    pub source: Option<&'a str>,
    /// Initial lifecycle state.
    pub initial_state: ArtifactRepositoryState,
    /// Creation wall-clock timestamp.
    pub now_ms: i64,
}

/// Secret-free inputs used to persist one Artifact repository token.
#[derive(Clone, Copy, Debug)]
pub struct NewArtifactToken<'a> {
    /// Opaque token identity.
    pub id: ArtifactTokenId,
    /// Installation-keyed token digest.
    pub digest: &'a [u8; 32],
    /// Granted repository scope.
    pub scope: ArtifactTokenScope,
    /// Absolute expiry timestamp.
    pub expires_at_ms: i64,
    /// Creation wall-clock timestamp.
    pub now_ms: i64,
    /// Maximum active tokens for the repository.
    pub max_tokens: u32,
}

impl FromStr for ArtifactTokenScope {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            _ => Err(invariant()),
        }
    }
}

/// Immutable Artifact namespace binding stored with one Worker version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewVersionArtifactBinding {
    /// Binding identity.
    pub id: BindingId,
    /// Tenant environment name.
    pub name: String,
    /// Frozen namespace identity.
    pub namespace_id: ResourceId,
    /// Frozen namespace generation, currently one.
    pub namespace_generation: u64,
    /// Static facade capability version.
    pub capability_version: u32,
    /// Canonical method permissions.
    pub permissions: CanonicalPermissions,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
}

/// Persisted immutable Artifact namespace binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionArtifactBindingRecord {
    /// Binding identity.
    pub id: BindingId,
    /// Owning Worker version.
    pub version_id: VersionId,
    /// Tenant environment name.
    pub name: String,
    /// Frozen namespace identity.
    pub namespace_id: ResourceId,
    /// Frozen namespace generation.
    pub namespace_generation: u64,
    /// Static facade capability version.
    pub capability_version: u32,
    /// Canonical method permissions.
    pub permissions: CanonicalPermissions,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Version-scoped Artifact binding authorization result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedArtifactBinding {
    /// Immutable binding authority.
    pub binding: VersionArtifactBindingRecord,
    /// Current bound namespace authority.
    pub namespace: ArtifactNamespaceRecord,
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "Artifact catalog invariant is invalid",
    )
}

pub(super) fn validate_namespace_name(value: &str) -> Result<(), PlatformError> {
    if valid_slug(value, 64) {
        Ok(())
    } else {
        Err(invalid("Artifact namespace name is invalid"))
    }
}

pub(super) fn validate_repository_name(value: &str) -> Result<(), PlatformError> {
    if valid_slug(value, 128) {
        Ok(())
    } else {
        Err(invalid("Artifact repository name is invalid"))
    }
}

fn valid_slug(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn validate_description(value: &str) -> Result<(), PlatformError> {
    if value.len() <= 2048 && !value.bytes().any(|byte| byte == 0) {
        Ok(())
    } else {
        Err(invalid("Artifact repository description is invalid"))
    }
}

pub(super) fn validate_jurisdiction(value: Option<&str>) -> Result<(), PlatformError> {
    if value.is_none() {
        Ok(())
    } else {
        Err(invalid(
            "Artifact namespace jurisdiction is unsupported by local placement",
        ))
    }
}

pub(super) fn validate_ref_name(value: &str) -> Result<(), PlatformError> {
    if !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/' | b'.'))
        && !value.contains("..")
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.ends_with('.')
    {
        Ok(())
    } else {
        Err(invalid("Artifact default branch is invalid"))
    }
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::ConfigInvalid, message)
}
