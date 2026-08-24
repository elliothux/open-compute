//! Process-local resource operation pins and deletion fence.

use open_compute_core::{ErrorCode, PlatformError, ResourceId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct Entry {
    count: usize,
    fenced: bool,
}

#[derive(Debug, Default)]
struct Inner {
    entries: Mutex<HashMap<ResourceId, Entry>>,
    changed: Notify,
}

/// Process-local authority that prevents resource deletion during active I/O.
#[derive(Clone, Debug, Default)]
pub struct ResourcePins {
    inner: Arc<Inner>,
}

impl ResourcePins {
    /// Construct an empty pin registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire a pin unless deletion has fenced the resource.
    pub fn try_pin(&self, resource_id: ResourceId) -> Result<ResourcePin, PlatformError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(resource_id).or_default();
        if entry.fenced {
            return Err(PlatformError::new(
                ErrorCode::ResourceNotReady,
                "resource is fenced for deletion",
            ));
        }
        entry.count = entry.count.checked_add(1).ok_or_else(|| {
            PlatformError::new(ErrorCode::Internal, "resource pin count overflow")
        })?;
        Ok(ResourcePin {
            resource_id,
            inner: self.inner.clone(),
        })
    }

    /// Fence new operations and wait for all active calls and streams to drain.
    pub async fn fence_and_wait(
        &self,
        resource_id: ResourceId,
        deadline: Duration,
    ) -> Result<(), PlatformError> {
        {
            let mut entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.entry(resource_id).or_default().fenced = true;
        }
        let wait = async {
            loop {
                let notified = self.inner.changed.notified();
                let empty = self
                    .inner
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&resource_id)
                    .is_none_or(|entry| entry.count == 0);
                if empty {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(deadline, wait).await.map_err(|_| {
            PlatformError::new(
                ErrorCode::ResourceReferenced,
                "resource still has in-flight operations",
            )
        })
    }

    /// Remove a fence after a failed database transition or driver operation.
    pub fn unfence(&self, resource_id: ResourceId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&resource_id) {
            entry.fenced = false;
            if entry.count == 0 {
                entries.remove(&resource_id);
            }
        }
        self.inner.changed.notify_waiters();
    }

    /// Forget a drained fence after the durable tombstone commits.
    pub fn retire_fence(&self, resource_id: ResourceId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&resource_id)
            .is_some_and(|entry| entry.fenced && entry.count == 0)
        {
            entries.remove(&resource_id);
        }
        self.inner.changed.notify_waiters();
    }

    /// Current active operation count for diagnostics and leak auditing.
    #[must_use]
    pub fn count(&self, resource_id: ResourceId) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&resource_id)
            .map_or(0, |entry| entry.count)
    }
}

/// RAII resource operation pin.
pub struct ResourcePin {
    resource_id: ResourceId,
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ResourcePin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourcePin")
            .field("resource_id", &self.resource_id)
            .finish_non_exhaustive()
    }
}

impl Drop for ResourcePin {
    fn drop(&mut self) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&self.resource_id) {
            entry.count = entry.count.saturating_sub(1);
            if entry.count == 0 && !entry.fenced {
                entries.remove(&self.resource_id);
            }
        }
        drop(entries);
        self.inner.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "resource_pins_tests.rs"]
mod tests;
