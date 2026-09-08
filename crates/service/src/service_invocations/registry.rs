use super::*;

/// Process-local invocation authority backed by persisted deployment state.
#[derive(Clone)]
pub struct ServiceInvocationRegistry {
    storage: Arc<open_compute_storage::PlatformStorage>,
    pins: VersionPins,
    call_deadline: Duration,
    pub(super) inner: Arc<Mutex<Inner>>,
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
    /// Bind persistent authority to the one process-local version pin registry.
    #[must_use]
    pub fn new(storage: Arc<open_compute_storage::PlatformStorage>, pins: VersionPins) -> Self {
        Self {
            storage,
            pins,
            call_deadline: CALL_DEADLINE,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    #[cfg(test)]
    pub(super) fn with_deadline(
        storage: Arc<open_compute_storage::PlatformStorage>,
        pins: VersionPins,
        call_deadline: Duration,
    ) -> Self {
        Self {
            storage,
            pins,
            call_deadline,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Reap expired invocation roots until the owning binding backend begins shutdown.
    pub(crate) async fn reap_deadlines_until_shutdown(
        &self,
        interval: Duration,
        shutdown: impl Future<Output = ()>,
    ) {
        let mut ticks = tokio::time::interval(interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                _ = ticks.tick() => self.reap_expired(),
            }
        }
    }

    /// Re-authorize, dynamically resolve, pin, and budget one Service call.
    pub fn resolve(
        &self,
        request: &ServiceResolveRequest,
    ) -> Result<ServiceAdmission, PlatformError> {
        let digest = parse_digest(&request.descriptor_sha256)?;
        let target = self.resolve_and_pin(request, &digest)?;
        let props = target
            .0
            .service
            .props_json
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()
            .map_err(|_| denied())?;
        let verified_descriptor = ServiceDescriptorV1::new(
            target.0.service.binding_name.clone(),
            target.0.service.target_worker_id,
            target.0.service.entrypoint.clone(),
            props.clone(),
        )
        .map_err(|_| denied())?;
        let canonical_props = verified_descriptor
            .props
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| denied())?;
        if canonical_props != target.0.service.props_json
            || verified_descriptor.sha256().map_err(|_| denied())?
                != target.0.service.descriptor_sha256
        {
            return Err(denied());
        }
        let target_version_id = target.0.target_version_id;
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
                if owner.version_id != request.caller_version_id {
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
                let caller_pin = self.pins.pin(request.caller_version_id)?;
                let root_id = token();
                let anchor_owner = token();
                inner.owners.insert(
                    anchor_owner.clone(),
                    Owner {
                        root: root_id.clone(),
                        version_id: request.caller_version_id,
                        _pin: caller_pin,
                        operations: 0,
                        retentions: 0,
                        anchor: true,
                    },
                );
                inner.roots.insert(
                    root_id.clone(),
                    Root {
                        deadline: now + self.call_deadline,
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
                version_id: target_version_id,
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
                connect: request.operation == ServiceOperation::Connect,
                websocket_allowed: matches!(
                    request.operation,
                    ServiceOperation::DefaultFetch | ServiceOperation::NamedFetch
                ),
                websocket: websocket_handoff::WebSocketHandoffState::Ordinary,
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
                    target.0.account_id, target_worker_id, target_version_id
                ),
                worker_code_sha256: hex::encode(target.0.target_worker_code_sha256),
                route_generation: target.0.target_route_generation,
                content_kind: target.0.target_content_kind,
                entrypoint: target.0.service.entrypoint,
                props,
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
                connect: false,
                websocket_allowed: false,
                websocket: websocket_handoff::WebSocketHandoffState::Ordinary,
            },
        );
        Ok(CapabilityAdmission {
            handle,
            frame: frame_id,
            deadline_ms: remaining_ms(inner.roots.get(&root_id).ok_or_else(denied)?, now),
        })
    }

    /// Retain the target or caller version for a returned/delegated capability.
    pub fn retain(&self, request: &ServiceRetainRequest) -> Result<String, PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let operation = inner.operations.get(&request.handle).ok_or_else(denied)?;
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
        complete_operation(&mut inner, &request.handle);
        Ok(())
    }

    /// Atomically and idempotently finalize a native connect and close its root event.
    pub fn finalize_connect(
        &self,
        request: &ServiceConnectFinalizeRequest,
    ) -> Result<(), PlatformError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(operation) = inner.operations.get(&request.handle) else {
            return Ok(());
        };
        if !operation.connect {
            return Err(denied());
        }
        let root_id = operation.root.clone();
        let root_frame = inner.frames.get(&request.caller_frame).ok_or_else(denied)?;
        let root = inner.roots.get(&root_id).ok_or_else(denied)?;
        if root_frame.root != root_id
            || root_frame.owner != root.anchor_owner
            || root_frame.depth != 0
        {
            return Err(denied());
        }
        inner.roots.get_mut(&root_id).ok_or_else(denied)?.closing = true;
        complete_operation(&mut inner, &request.handle);
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

    pub(super) fn reap_expired(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let expired = inner
            .roots
            .iter()
            .filter(|(root_id, root)| {
                root.deadline <= now
                    && !inner.operations.values().any(|operation| {
                        operation.root == **root_id
                            && operation.websocket
                                == websocket_handoff::WebSocketHandoffState::Active
                    })
            })
            .map(|(root_id, _)| root_id.clone())
            .collect::<Vec<_>>();
        for root_id in expired {
            remove_root(&mut inner, &root_id);
        }
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

    /// Drop state only when it still belongs to the named private-protocol generation.
    pub fn clear_generation(&self, generation: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.generation.as_deref() == Some(generation) {
            *inner = Inner::default();
        }
    }

    /// Drop every invocation, capability, frame, and version pin after workerd exits.
    ///
    /// The process supervisor owns this unconditional transition and calls it only after the
    /// previously running child is confirmed absent, or immediately before admitting a known
    /// replacement child. The private protocol generation is intentionally not exposed through
    /// the supervisor snapshot.
    pub fn clear_after_child_exit(&self) {
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Inner::default();
    }

    fn resolve_and_pin(
        &self,
        request: &ServiceResolveRequest,
        digest: &[u8; 32],
    ) -> Result<(ResolvedServiceTarget, VersionPin), PlatformError> {
        let repository = ServiceRepository::new(self.storage.db());
        for _ in 0..3 {
            let target =
                repository.resolve(request.caller_version_id, &request.binding_name, digest)?;
            match (request.operation, target.service.entrypoint.as_ref()) {
                (ServiceOperation::DefaultFetch, Some(_))
                | (ServiceOperation::NamedFetch, None) => return Err(denied()),
                (ServiceOperation::Rpc, _)
                | (ServiceOperation::Connect, _)
                | (ServiceOperation::DefaultFetch, None)
                | (ServiceOperation::NamedFetch, Some(_)) => {}
            }
            if request.operation != ServiceOperation::DefaultFetch
                && target.target_content_kind
                    == open_compute_storage::VersionContentKind::AssetsOnly
            {
                return Err(PlatformError::new(
                    ErrorCode::ServiceEntrypointNotFound,
                    "Assets-only Service target has no RPC entrypoint",
                ));
            }
            let pin = self.pins.pin(target.target_version_id).map_err(|_| {
                PlatformError::new(
                    ErrorCode::ServiceTargetNotReady,
                    "Service target is fenced for deletion",
                )
            })?;
            let confirmed =
                repository.resolve(request.caller_version_id, &request.binding_name, digest)?;
            if confirmed.target_version_id == target.target_version_id
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
