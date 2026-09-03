//! Shared durable Workflow catalog and execution identities.

use crate::VersionState;
use open_compute_core::{
    AccountId, BindingId, ResourceAvailability, ResourceState, VersionId, WorkerId, WorkflowId,
    WorkflowInstanceId, WorkflowOperationId, WorkflowToken, WorkflowVersionId,
};
use serde::{Deserialize, Serialize};

/// Account-scoped logical Workflow definition.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDefinition {
    /// Immutable definition identity.
    pub id: WorkflowId,
    /// Owning account.
    pub account_id: AccountId,
    /// Mutable display name, unique among live definitions.
    pub name: String,
    /// Durable lifecycle state.
    pub state: ResourceState,
    /// Availability independent from lifecycle.
    pub availability: ResourceAvailability,
    /// Low-cardinality failure reason.
    pub availability_code: Option<String>,
    /// Binding-breaking lifecycle generation.
    pub lifecycle_generation: i64,
    /// Class reserved by a Worker upload while the first Workflow version is pending.
    pub reserved_class_name: Option<String>,
    /// Version selected by new instance creation.
    pub current_version_id: Option<WorkflowVersionId>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Most recent catalog mutation.
    pub updated_at_ms: i64,
}

/// Immutable class, version, and capability identity copied into each instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowTarget {
    /// Owning account.
    pub account_id: AccountId,
    /// Logical definition identity.
    pub definition_id: WorkflowId,
    /// Definition name at the time this target is frozen.
    pub definition_name: String,
    /// Immutable Workflow version identity.
    pub workflow_version_id: WorkflowVersionId,
    /// Owning Worker identity.
    pub worker_id: WorkerId,
    /// Immutable ready version.
    pub worker_version_id: VersionId,
    /// Exact `WorkerCode` descriptor digest.
    pub worker_code_sha256: [u8; 32],
    /// Validated `WorkflowEntrypoint` named export.
    pub class_name: String,
    /// Frozen loader schema.
    pub loader_schema_version: i64,
    /// Frozen current execution capability, required to be one.
    pub capability_version: i64,
    /// Canonical frozen version descriptor digest.
    pub descriptor_sha256: [u8; 32],
}

/// Immutable Workflow version with its validation lifecycle.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVersion {
    /// Immutable target authority.
    pub target: WorkflowTarget,
    /// Monotonic logical definition version number.
    pub version_number: i64,
    /// Same staging/validation/ready/retirement lifecycle as versions.
    pub state: VersionState,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Sanitized permanent validation rejection.
    pub rejection_code: Option<String>,
}

/// Immutable caller binding descriptor; this exact shape is hashed once at version creation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowBindingDescriptor {
    /// Static product kind, always Workflow.
    pub kind: open_compute_core::BindingKind,
    /// Descriptor schema version.
    pub schema_version: u32,
    /// Immutable binding identity.
    pub binding_id: BindingId,
    /// Tenant environment name.
    pub name: String,
    /// Logical Workflow identity.
    pub definition_id: WorkflowId,
    /// Frozen definition lifecycle generation.
    pub definition_lifecycle_generation: i64,
    /// Exact `WorkflowEntrypoint` export selected by this binding.
    pub class_name: String,
    /// Supported facade capability.
    pub capability_version: u32,
    /// Exact direct cron schedules owned by this binding.
    pub schedules: Vec<String>,
}

/// Persisted immutable Workflow binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowBindingRecord {
    /// Canonical descriptor.
    pub descriptor: WorkflowBindingDescriptor,
    /// Caller version owning the binding.
    pub version_id: VersionId,
    /// Exact descriptor digest.
    pub descriptor_sha256: [u8; 32],
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Cross-database creation/referrer lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRefState {
    /// External ID and artifact reserved before scheduler insertion.
    Creating,
    /// Both authorities committed; instance may execute.
    Live,
    /// Terminal history still pins its immutable version until proven purge.
    Retained,
    /// A durable restart intent owns the active quota and blocks dispatch.
    Restarting,
    /// Terminal scheduler state observed; release is pending.
    Releasing,
    /// Terminal history retained without an execution artifact pin.
    Released,
}

/// Immutable instance identity shared by the control reservation and scheduler authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowInstanceIdentity {
    /// Internal instance identity.
    pub instance_id: WorkflowInstanceId,
    /// Definition-scoped public identity.
    pub external_instance_id: String,
    /// Frozen version identity.
    pub target: WorkflowTarget,
    /// Exact instance generation.
    pub instance_generation: i64,
    /// Cross-database identity proof, not a tenant-visible idempotency key.
    pub creation_nonce: WorkflowToken,
    /// Durable per-instance creation operation shared by control and scheduler authority.
    pub creation_operation_id: WorkflowOperationId,
    /// Durable create-batch identity used to recover publication as one atomic group.
    pub creation_batch_id: WorkflowOperationId,
    /// Stable event timestamp.
    pub created_at_ms: i64,
    /// Direct cron metadata, absent for programmatic and REST-created instances.
    pub schedule: Option<open_compute_core::WorkflowCronSchedule>,
}

/// Recoverable control reservation, with lifecycle read only from control authority.
#[derive(Clone, Debug)]
pub struct WorkflowReservation {
    /// Cross-database immutable identity and creation proof.
    pub identity: WorkflowInstanceIdentity,
    /// Creation/referrer lifecycle.
    pub state: WorkflowRefState,
    /// Last control-state mutation.
    pub updated_at_ms: i64,
}

/// Low-cardinality operation facts, with no identities, payloads or private nonce material.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowOperationInspection {
    /// Prepared restart intents not yet finalized or definitively cancelled.
    pub pending_restarts: u64,
    /// Prepared purge intents not yet finalized or definitively cancelled.
    pub pending_purges: u64,
    /// Creation time of the oldest unfinished operation, if any.
    pub oldest_operation_at_ms: Option<i64>,
}
