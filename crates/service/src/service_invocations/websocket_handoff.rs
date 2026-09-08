//! Native WebSocket ownership for Service fetch operations.

use super::*;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WebSocketHandoffState {
    Ordinary,
    Active,
}

/// RAII ownership of Service operation pins for one public WebSocket tunnel.
#[derive(Debug)]
pub(crate) struct ServiceWebSocketLease {
    registry: ServiceInvocationRegistry,
    handles: Vec<String>,
}

impl Drop for ServiceWebSocketLease {
    fn drop(&mut self) {
        let mut inner = self
            .registry
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for handle in &self.handles {
            complete_operation(&mut inner, handle);
        }
    }
}

impl ServiceInvocationRegistry {
    /// Atomically transfer Service fetch operations to one native WebSocket tunnel.
    pub(crate) fn activate_websocket_handoffs(
        &self,
        handles: &[String],
    ) -> Result<ServiceWebSocketLease, PlatformError> {
        if handles.is_empty() || handles.len() > MAX_DEPTH as usize {
            return Err(denied());
        }
        let mut unique = HashSet::with_capacity(handles.len());
        for handle in handles {
            let parsed = Uuid::parse_str(handle).map_err(|_| denied())?;
            if parsed.get_version_num() != 7
                || parsed.to_string() != *handle
                || !unique.insert(handle.as_str())
            {
                return Err(denied());
            }
        }

        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut root_id = None;
        for handle in handles {
            let operation = inner.operations.get(handle).ok_or_else(denied)?;
            if !operation.websocket_allowed
                || operation.websocket != WebSocketHandoffState::Ordinary
            {
                return Err(denied());
            }
            if root_id.as_ref().is_some_and(|root| root != &operation.root) {
                return Err(denied());
            }
            root_id.get_or_insert_with(|| operation.root.clone());
        }
        let root = inner
            .roots
            .get(root_id.as_deref().ok_or_else(denied)?)
            .ok_or_else(denied)?;
        if root.deadline <= Instant::now() {
            return Err(PlatformError::new(
                ErrorCode::ServiceTimeout,
                "Service invocation deadline expired before WebSocket handoff",
            ));
        }
        for handle in handles {
            inner
                .operations
                .get_mut(handle)
                .ok_or_else(denied)?
                .websocket = WebSocketHandoffState::Active;
        }
        Ok(ServiceWebSocketLease {
            registry: self.clone(),
            handles: handles.to_vec(),
        })
    }
}
