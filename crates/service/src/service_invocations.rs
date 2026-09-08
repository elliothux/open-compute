//! Generation-local Service invocation budgets, authority, and version leases.

use open_compute_core::{ErrorCode, PlatformError, VersionId};
use open_compute_storage::{ResolvedServiceTarget, ServiceRepository};
use open_compute_workers::{ServiceDescriptorV1, VersionPin, VersionPins};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[path = "service_invocations/websocket_handoff.rs"]
mod websocket_handoff;
pub(crate) use websocket_handoff::ServiceWebSocketLease;

const MAX_DEPTH: u32 = 16;
const MAX_TOTAL_CALLS: u32 = 128;
const MAX_CONCURRENT_CALLS: u32 = 32;
const CALL_DEADLINE: Duration = Duration::from_secs(30);
/// Poll cadence for the binding-backend-owned invocation deadline reaper.
pub(crate) const DEADLINE_REAPER_INTERVAL: Duration = Duration::from_secs(1);

/// Service operation category used only for authority and low-cardinality policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceOperation {
    /// Default binding fetch, including target static-asset routing.
    DefaultFetch,
    /// Fetch on a declared named entrypoint.
    NamedFetch,
    /// Native RPC on default or named entrypoint.
    Rpc,
    /// Native raw TCP connect handler on default or named entrypoint.
    Connect,
}

/// Private resolve request containing control metadata only.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceResolveRequest {
    /// Caller version frozen into the raw system capability.
    pub caller_version_id: VersionId,
    /// Persisted environment binding name.
    pub binding_name: String,
    /// Lowercase canonical descriptor digest.
    pub descriptor_sha256: String,
    /// Trusted parent frame; absent only for a root event.
    pub parent_frame: Option<String>,
    /// Requested dispatch category.
    pub operation: ServiceOperation,
}

/// Private capability-operation request.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityBeginRequest {
    /// Live retained capability identity.
    pub retention: String,
    /// Trusted current caller frame, or absent for the original root caller.
    pub parent_frame: Option<String>,
}

/// Which version owns a capability crossing the current call.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionOwner {
    /// Capability returned by the target.
    Target,
    /// Callback capability supplied by the caller.
    Caller,
}

/// Private capability retention request.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceRetainRequest {
    /// Current admitted operation handle.
    pub handle: String,
    /// Capability ownership side.
    pub owner: RetentionOwner,
}

/// Idempotent completion or retention-release request.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceReleaseRequest {
    /// Operation handle or retention identity.
    pub handle: String,
}

/// Root-event completion request from the trusted loader wrapper.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceRootCompleteRequest {
    /// Caller frame returned when the root was first admitted.
    pub frame: String,
}

/// Atomic completion of one native connect operation and its root event.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceConnectFinalizeRequest {
    /// Connect operation handle returned by admission.
    pub handle: String,
    /// Root caller frame returned by the same admission.
    pub caller_frame: String,
}

/// Immutable target identity returned to the trusted workerd controller.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceTargetPayload {
    /// Canonical loader key.
    pub loader_key: String,
    /// Target descriptor digest.
    pub worker_code_sha256: String,
    /// Target route generation.
    pub route_generation: u64,
    /// Target content discriminator.
    pub content_kind: open_compute_storage::VersionContentKind,
    /// Persisted optional named entrypoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Deployer-authenticated immutable properties for the target `ExecutionContext`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub props: Option<serde_json::Value>,
}

/// Admitted native invocation returned to the trusted controller.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAdmission {
    /// Idempotent completion handle.
    pub handle: String,
    /// Child frame restored only by the target wrapper.
    pub frame: String,
    /// Root caller frame cached only by the trusted controller.
    pub caller_frame: String,
    /// Remaining root deadline in milliseconds.
    pub deadline_ms: u64,
    /// Fixed target identity for this invocation.
    pub target: ServiceTargetPayload,
}

/// Admitted call on a previously returned or delegated capability.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityAdmission {
    /// Idempotent completion handle.
    pub handle: String,
    /// Child frame restored for the capability method.
    pub frame: String,
    /// Remaining root deadline in milliseconds.
    pub deadline_ms: u64,
}

#[derive(Debug)]
struct Root {
    deadline: Instant,
    total_calls: u32,
    concurrent_calls: u32,
    anchor_owner: String,
    closing: bool,
}

struct Owner {
    root: String,
    version_id: VersionId,
    _pin: VersionPin,
    operations: u32,
    retentions: u32,
    anchor: bool,
}

impl std::fmt::Debug for Owner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Owner")
            .field("root", &self.root)
            .field("version_id", &self.version_id)
            .field("operations", &self.operations)
            .field("retentions", &self.retentions)
            .field("anchor", &self.anchor)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct Frame {
    root: String,
    owner: String,
    depth: u32,
}

#[derive(Debug)]
struct Operation {
    root: String,
    owner: String,
    caller_owner: String,
    frame: String,
    connect: bool,
    websocket_allowed: bool,
    websocket: websocket_handoff::WebSocketHandoffState,
}

#[derive(Debug)]
struct Retention {
    root: String,
    owner: String,
    depth: u32,
}

#[derive(Debug, Default)]
struct Inner {
    generation: Option<String>,
    roots: HashMap<String, Root>,
    owners: HashMap<String, Owner>,
    frames: HashMap<String, Frame>,
    operations: HashMap<String, Operation>,
    retentions: HashMap<String, Retention>,
}

/// Process-local Service call authority. Dropping it releases every generation pin.
mod registry;

pub use registry::ServiceInvocationRegistry;

fn admit_budget(
    inner: &mut Inner,
    root_id: &str,
    depth: u32,
    now: Instant,
) -> Result<(), PlatformError> {
    let root = inner.roots.get_mut(root_id).ok_or_else(denied)?;
    if now >= root.deadline {
        return Err(PlatformError::new(
            ErrorCode::ServiceTimeout,
            "Service invocation deadline expired",
        ));
    }
    if depth > MAX_DEPTH
        || root.total_calls >= MAX_TOTAL_CALLS
        || root.concurrent_calls >= MAX_CONCURRENT_CALLS
    {
        return Err(limit());
    }
    root.total_calls = root.total_calls.saturating_add(1);
    root.concurrent_calls = root.concurrent_calls.saturating_add(1);
    Ok(())
}

fn complete_operation(inner: &mut Inner, handle: &str) {
    let Some(operation) = inner.operations.remove(handle) else {
        return;
    };
    if let Some(root) = inner.roots.get_mut(&operation.root) {
        root.concurrent_calls = root.concurrent_calls.saturating_sub(1);
    }
    if let Some(owner) = inner.owners.get_mut(&operation.owner) {
        owner.operations = owner.operations.saturating_sub(1);
    }
    inner.frames.remove(&operation.frame);
    reap(inner, &operation.root, &operation.owner);
}

fn remove_root(inner: &mut Inner, root_id: &str) {
    inner
        .operations
        .retain(|_, operation| operation.root != root_id);
    inner
        .retentions
        .retain(|_, retention| retention.root != root_id);
    inner.frames.retain(|_, frame| frame.root != root_id);
    inner.owners.retain(|_, owner| owner.root != root_id);
    inner.roots.remove(root_id);
}

fn reap(inner: &mut Inner, root_id: &str, owner_id: &str) {
    let removable_owner = inner
        .owners
        .get(owner_id)
        .is_some_and(|owner| !owner.anchor && owner.operations == 0 && owner.retentions == 0);
    if removable_owner {
        inner.owners.remove(owner_id);
    }
    let active = inner.owners.values().any(|owner| {
        owner.root == root_id && (!owner.anchor || owner.operations > 0 || owner.retentions > 0)
    });
    if !active
        && inner.roots.get(root_id).is_some_and(|root| root.closing)
        && let Some(root) = inner.roots.remove(root_id)
    {
        inner.owners.remove(&root.anchor_owner);
        inner.frames.retain(|_, frame| frame.root != root_id);
    }
}

fn remaining_ms(root: &Root, now: Instant) -> u64 {
    u64::try_from(root.deadline.saturating_duration_since(now).as_millis()).unwrap_or(u64::MAX)
}

fn parse_digest(value: &str) -> Result<[u8; 32], PlatformError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(denied());
    }
    let bytes = hex::decode(value).map_err(|_| denied())?;
    bytes.as_slice().try_into().map_err(|_| denied())
}

fn token() -> String {
    Uuid::now_v7().to_string()
}

fn denied() -> PlatformError {
    PlatformError::new(
        ErrorCode::ServiceBindingDenied,
        "Service invocation scope or authority was denied",
    )
}

fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::ServiceLimitExceeded,
        "Service invocation budget was exhausted",
    )
}

#[cfg(test)]
#[path = "service_invocations_tests.rs"]
mod tests;
