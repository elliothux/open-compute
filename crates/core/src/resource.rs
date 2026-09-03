//! Shared resource lifecycle and version-binding value types.

use crate::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// Product resource kind recognized by the static binding registry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    /// Workers KV namespace.
    KvNamespace,
    /// R2-compatible object bucket.
    R2Bucket,
    /// D1-compatible SQL database.
    D1Database,
    /// Durable Object namespace.
    DoNamespace,
    /// Vectorize exact-search index.
    VectorizeIndex,
    /// AI Search namespace containing account-scoped instances.
    AiSearchNamespace,
    /// AI Search built-in-storage instance.
    AiSearchInstance,
    /// Queue producer runtime binding; never a generic P0 resource driver.
    QueueProducer,
    /// Logical Workflow caller binding; execution state is not a generic resource driver.
    Workflow,
}

impl BindingKind {
    /// Stable database and protocol token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KvNamespace => "kv_namespace",
            Self::R2Bucket => "r2_bucket",
            Self::D1Database => "d1_database",
            Self::DoNamespace => "do_namespace",
            Self::VectorizeIndex => "vectorize_index",
            Self::AiSearchNamespace => "ai_search_namespace",
            Self::AiSearchInstance => "ai_search_instance",
            Self::QueueProducer => "queue_producer",
            Self::Workflow => "workflow",
        }
    }
}

impl Display for BindingKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BindingKind {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "kv_namespace" => Ok(Self::KvNamespace),
            "r2_bucket" => Ok(Self::R2Bucket),
            "d1_database" => Ok(Self::D1Database),
            "do_namespace" => Ok(Self::DoNamespace),
            "vectorize_index" => Ok(Self::VectorizeIndex),
            "ai_search_namespace" => Ok(Self::AiSearchNamespace),
            "ai_search_instance" => Ok(Self::AiSearchInstance),
            "queue_producer" => Ok(Self::QueueProducer),
            "workflow" => Ok(Self::Workflow),
            _ => Err(resource_type_error()),
        }
    }
}

/// Durable resource lifecycle state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    /// Authority row exists while the physical identity is being created.
    Creating,
    /// Resource may be bound and called.
    Ready,
    /// New calls are fenced while physical deletion converges.
    Deleting,
    /// Resource identity is permanently retired.
    Tombstoned,
}

impl ResourceState {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }
}

impl FromStr for ResourceState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "creating" => Ok(Self::Creating),
            "ready" => Ok(Self::Ready),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(resource_invariant()),
        }
    }
}

/// Persisted resource health independent from lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAvailability {
    /// Driver probe confirms normal service.
    Healthy,
    /// Resource is serving with a stable degraded-health reason.
    Degraded,
    /// Operations fail closed until a successful probe or operator repair.
    Unavailable,
}

impl ResourceAvailability {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }
}

impl FromStr for ResourceAvailability {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(resource_invariant()),
        }
    }
}

/// Canonical capability permissions shared by P0 resource adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalPermissions {
    /// Read-like methods are authorized.
    pub read: bool,
    /// Mutation methods are authorized.
    pub write: bool,
}

impl Default for CanonicalPermissions {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
        }
    }
}

/// Canonical product configuration carried by an immutable binding.
///
/// Fields are admitted only by their owning product adapter; a non-Workflow
/// binding rejects the Workflow fields even though version input shares
/// this closed wire shape.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalBindingConfig {
    /// Exact `WorkflowEntrypoint` export selected by a Workflow binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_class_name: Option<String>,
    /// Internal fencing generation for an upload-first Workflow reservation.
    ///
    /// This value is populated only by the authenticated Worker upload adapter and is never
    /// accepted from, or emitted to, the public binding wire contract.
    #[serde(skip)]
    pub workflow_reservation_fence: Option<i64>,
    /// Direct cron schedules attached to a Workflow binding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_schedules: Vec<String>,
}

fn resource_type_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::BindingTypeMismatch,
        "resource or binding kind is not supported",
    )
}

fn resource_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "persisted resource state is invalid",
    )
}

#[cfg(test)]
#[path = "resource_tests.rs"]
mod tests;
