//! Generation-local Service invocation budgets, authority, and deployment leases.

use open_compute_core::{DeploymentId, ErrorCode, PlatformError};
use open_compute_storage::{ResolvedServiceTarget, ServiceRepository};
use open_compute_workers::{DeploymentPin, DeploymentPins};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_DEPTH: u32 = 16;
const MAX_TOTAL_CALLS: u32 = 128;
const MAX_CONCURRENT_CALLS: u32 = 32;
const CALL_DEADLINE: Duration = Duration::from_secs(30);

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
}

/// Private resolve request containing control metadata only.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceResolveRequest {
    /// Caller deployment frozen into the raw system capability.
    pub caller_deployment_id: DeploymentId,
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

/// Which deployment owns a capability crossing the current call.
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
    pub content_kind: open_compute_storage::DeploymentContentKind,
    /// Persisted optional named entrypoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
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
    deployment_id: DeploymentId,
    _pin: DeploymentPin,
    operations: u32,
    retentions: u32,
    anchor: bool,
}

impl std::fmt::Debug for Owner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Owner")
            .field("root", &self.root)
            .field("deployment_id", &self.deployment_id)
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
    completed: bool,
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
#[derive(Clone)]
pub struct ServiceInvocationRegistry {
    storage: Arc<open_compute_storage::PlatformStorage>,
    pins: DeploymentPins,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for ServiceInvocationRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        formatter
            .debug_struct("ServiceInvocationRegistry")
            .field("roots", &inner.roots.len())
            .field("owners", &inner.owners.len())
            .field("operations", &inner.operations.len())
            .field("retentions", &inner.retentions.len())
            .finish()
    }
}

impl ServiceInvocationRegistry {
    /// Bind persistent authority to the one process-local deployment pin registry.
    #[must_use]
    pub fn new(storage: Arc<open_compute_storage::PlatformStorage>, pins: DeploymentPins) -> Self {
        Self {
            storage,
            pins,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Re-authorize, dynamically resolve, pin, and budget one Service call.
    pub fn resolve(
        &self,
        request: &ServiceResolveRequest,
    ) -> Result<ServiceAdmission, PlatformError> {
        let digest = parse_digest(&request.descriptor_sha256)?;
        let target = self.resolve_and_pin(request, &digest)?;
        let target_deployment_id = target.0.target_deployment_id;
        let target_worker_id = target.0.service.target_worker_id;
        let target_pin = target.1;

        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let (root_id, caller_owner, caller_frame, depth) = match request.parent_frame.as_deref() {
            Some(parent) => {
                let frame = inner.frames.get(parent).ok_or_else(denied)?;
                let owner = inner.owners.get(&frame.owner).ok_or_else(denied)?;
                if owner.deployment_id != request.caller_deployment_id {
                    return Err(denied());
                }
                (
                    frame.root.clone(),
                    frame.owner.clone(),
                    parent.to_owned(),
                    frame.depth.saturating_add(1),
                )
            }
            None => {
                let caller_pin = self.pins.pin(request.caller_deployment_id)?;
                let root_id = token();
                let anchor_owner = token();
                inner.owners.insert(
                    anchor_owner.clone(),
                    Owner {
                        root: root_id.clone(),
                        deployment_id: request.caller_deployment_id,
                        _pin: caller_pin,
                        operations: 0,
                        retentions: 0,
                        anchor: true,
                    },
                );
                inner.roots.insert(
                    root_id.clone(),
                    Root {
                        deadline: now + CALL_DEADLINE,
                        total_calls: 0,
                        concurrent_calls: 0,
                        anchor_owner: anchor_owner.clone(),
                        closing: false,
                    },
                );
                let caller_frame = token();
                inner.frames.insert(
                    caller_frame.clone(),
                    Frame {
                        root: root_id.clone(),
                        owner: anchor_owner.clone(),
                        depth: 0,
                    },
                );
                (root_id, anchor_owner, caller_frame, 1)
            }
        };
        admit_budget(&mut inner, &root_id, depth, now)?;
        let owner_id = token();
        let frame_id = token();
        let handle = token();
        inner.owners.insert(
            owner_id.clone(),
            Owner {
                root: root_id.clone(),
                deployment_id: target_deployment_id,
                _pin: target_pin,
                operations: 1,
                retentions: 0,
                anchor: false,
            },
        );
        inner.frames.insert(
            frame_id.clone(),
            Frame {
                root: root_id.clone(),
                owner: owner_id.clone(),
                depth,
            },
        );
        inner.operations.insert(
            handle.clone(),
            Operation {
                root: root_id.clone(),
                owner: owner_id,
                caller_owner,
                frame: frame_id.clone(),
                completed: false,
            },
        );
        let deadline_ms = remaining_ms(inner.roots.get(&root_id).ok_or_else(denied)?, now);
        Ok(ServiceAdmission {
            handle,
            frame: frame_id,
            caller_frame,
            deadline_ms,
            target: ServiceTargetPayload {
                loader_key: format!(
                    "{}/{}/{}",
                    target.0.account_id, target_worker_id, target_deployment_id
                ),
                worker_code_sha256: hex::encode(target.0.target_worker_code_sha256),
                route_generation: target.0.target_route_generation,
                content_kind: target.0.target_content_kind,
                entrypoint: target.0.service.entrypoint,
            },
        })
    }

    /// Admit a method call on one retained native capability without re-resolving active.
    pub fn begin_capability(
        &self,
        request: &CapabilityBeginRequest,
    ) -> Result<CapabilityAdmission, PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let retention = inner
            .retentions
            .get(&request.retention)
            .ok_or_else(denied)?;
        let root_id = retention.root.clone();
        let owner_id = retention.owner.clone();
        let caller_owner = match request.parent_frame.as_deref() {
            Some(parent) => {
                let frame = inner.frames.get(parent).ok_or_else(denied)?;
                if frame.root != root_id {
                    return Err(denied());
                }
                frame.owner.clone()
            }
            None => inner
                .roots
                .get(&root_id)
                .ok_or_else(denied)?
                .anchor_owner
                .clone(),
        };
        let parent_depth = request
            .parent_frame
            .as_deref()
            .and_then(|parent| inner.frames.get(parent).map(|frame| frame.depth))
            .unwrap_or(0);
        let depth = retention.depth.max(parent_depth).saturating_add(1);
        let now = Instant::now();
        admit_budget(&mut inner, &root_id, depth, now)?;
        let owner = inner.owners.get_mut(&owner_id).ok_or_else(denied)?;
        owner.operations = owner.operations.checked_add(1).ok_or_else(limit)?;
        let frame_id = token();
        let handle = token();
        inner.frames.insert(
            frame_id.clone(),
            Frame {
                root: root_id.clone(),
                owner: owner_id.clone(),
                depth,
            },
        );
        inner.operations.insert(
            handle.clone(),
            Operation {
                root: root_id.clone(),
                owner: owner_id.clone(),
                caller_owner,
                frame: frame_id.clone(),
                completed: false,
            },
        );
        Ok(CapabilityAdmission {
            handle,
            frame: frame_id,
            deadline_ms: remaining_ms(inner.roots.get(&root_id).ok_or_else(denied)?, now),
        })
    }

    /// Retain the target or caller deployment for a returned/delegated capability.
    pub fn retain(&self, request: &ServiceRetainRequest) -> Result<String, PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let operation = inner.operations.get(&request.handle).ok_or_else(denied)?;
        if operation.completed {
            return Err(denied());
        }
        let owner_id = match request.owner {
            RetentionOwner::Target => operation.owner.clone(),
            RetentionOwner::Caller => operation.caller_owner.clone(),
        };
        let frame = inner.frames.get(&operation.frame).ok_or_else(denied)?;
        let root_id = operation.root.clone();
        let depth = frame.depth;
        let owner = inner.owners.get_mut(&owner_id).ok_or_else(denied)?;
        owner.retentions = owner.retentions.checked_add(1).ok_or_else(limit)?;
        let retention_id = token();
        inner.retentions.insert(
            retention_id.clone(),
            Retention {
                root: root_id,
                owner: owner_id,
                depth,
            },
        );
        Ok(retention_id)
    }

    /// Idempotently mark an operation result and all tracked background work drained.
    pub fn complete(&self, request: &ServiceReleaseRequest) -> Result<(), PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(mut operation) = inner.operations.remove(&request.handle) else {
            return Ok(());
        };
        if operation.completed {
            return Ok(());
        }
        operation.completed = true;
        if let Some(root) = inner.roots.get_mut(&operation.root) {
            root.concurrent_calls = root.concurrent_calls.saturating_sub(1);
        }
        if let Some(owner) = inner.owners.get_mut(&operation.owner) {
            owner.operations = owner.operations.saturating_sub(1);
        }
        inner.frames.remove(&operation.frame);
        reap(&mut inner, &operation.root, &operation.owner);
        Ok(())
    }

    /// Idempotently release one final native capability reference group.
    pub fn release(&self, request: &ServiceReleaseRequest) -> Result<(), PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(retention) = inner.retentions.remove(&request.handle) else {
            return Ok(());
        };
        if let Some(owner) = inner.owners.get_mut(&retention.owner) {
            owner.retentions = owner.retentions.saturating_sub(1);
        }
        reap(&mut inner, &retention.root, &retention.owner);
        Ok(())
    }

    /// Idempotently close a trusted root event after handlers, streams, and waitUntil drain.
    pub fn complete_root(&self, request: &ServiceRootCompleteRequest) -> Result<(), PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(frame) = inner.frames.get(&request.frame) else {
            return Ok(());
        };
        let owner_id = frame.owner.clone();
        let root_id = frame.root.clone();
        let Some(root) = inner.roots.get(&root_id) else {
            return Ok(());
        };
        if root.anchor_owner != owner_id || frame.depth != 0 {
            return Err(denied());
        }
        if inner.owners.values().any(|owner| {
            owner.root == root_id && (!owner.anchor || owner.operations > 0 || owner.retentions > 0)
        }) {
            if let Some(root) = inner.roots.get_mut(&root_id) {
                root.closing = true;
            }
            return Ok(());
        }
        inner.roots.remove(&root_id);
        inner.owners.remove(&owner_id);
        inner.frames.retain(|_, value| value.root != root_id);
        Ok(())
    }

    /// Current process-local counts for tests and bounded diagnostics.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            inner.roots.len(),
            inner.operations.len(),
            inner.retentions.len(),
        )
    }

    /// Select the authenticated workerd generation before processing one controller request.
    ///
    /// A first request from a replacement generation atomically invalidates any state left by its
    /// predecessor. The binding backend calls this while generation authentication is fenced.
    pub fn activate_generation(&self, generation: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.generation.as_deref() != Some(generation) {
            *inner = Inner {
                generation: Some(generation.to_owned()),
                ..Inner::default()
            };
        }
    }

    /// Drop every invocation, capability, frame, and deployment pin after workerd exits.
    ///
    /// The supervisor watcher calls this only after the owning child is confirmed gone. A new
    /// generation therefore cannot reuse an old handle, and crashed tenant code cannot leave a
    /// process-lifetime deletion fence behind.
    pub fn clear_generation(&self, generation: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.generation.as_deref() == Some(generation) {
            *inner = Inner::default();
        }
    }

    fn resolve_and_pin(
        &self,
        request: &ServiceResolveRequest,
        digest: &[u8; 32],
    ) -> Result<(ResolvedServiceTarget, DeploymentPin), PlatformError> {
        let repository = ServiceRepository::new(self.storage.db());
        for _ in 0..3 {
            let target =
                repository.resolve(request.caller_deployment_id, &request.binding_name, digest)?;
            match (request.operation, target.service.entrypoint.as_ref()) {
                (ServiceOperation::DefaultFetch, Some(_))
                | (ServiceOperation::NamedFetch, None) => return Err(denied()),
                (ServiceOperation::Rpc, _)
                | (ServiceOperation::DefaultFetch, None)
                | (ServiceOperation::NamedFetch, Some(_)) => {}
            }
            if request.operation != ServiceOperation::DefaultFetch
                && target.target_content_kind
                    == open_compute_storage::DeploymentContentKind::AssetsOnly
            {
                return Err(PlatformError::new(
                    ErrorCode::ServiceEntrypointNotFound,
                    "Assets-only Service target has no RPC entrypoint",
                ));
            }
            let pin = self.pins.pin(target.target_deployment_id).map_err(|_| {
                PlatformError::new(
                    ErrorCode::ServiceTargetNotReady,
                    "Service target is fenced for deletion",
                )
            })?;
            let confirmed =
                repository.resolve(request.caller_deployment_id, &request.binding_name, digest)?;
            if confirmed.target_deployment_id == target.target_deployment_id
                && confirmed.target_worker_code_sha256 == target.target_worker_code_sha256
            {
                return Ok((confirmed, pin));
            }
            drop(pin);
        }
        Err(PlatformError::new(
            ErrorCode::ServiceUnavailable,
            "Service target changed during admission",
        ))
    }
}

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
