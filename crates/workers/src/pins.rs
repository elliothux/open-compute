//! In-process deployment dispatch pins and deletion fence.

use open_compute_core::{DeploymentId, ErrorCode, PlatformError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct Entry {
    count: usize,
    fenced: bool,
    retained_until_restart: bool,
}

#[derive(Debug, Default)]
struct Inner {
    entries: Mutex<HashMap<DeploymentId, Entry>>,
    changed: Notify,
}

/// Process-local authority that freezes deployment identity for in-flight work.
#[derive(Clone, Debug, Default)]
pub struct DeploymentPins {
    inner: Arc<Inner>,
}

impl DeploymentPins {
    /// Construct an empty pin registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire a pin unless deletion already fenced the deployment.
    pub fn pin(&self, deployment_id: DeploymentId) -> Result<DeploymentPin, PlatformError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(deployment_id).or_default();
        if entry.fenced {
            return Err(PlatformError::new(
                ErrorCode::DeploymentNotReady,
                "deployment is fenced for deletion",
            ));
        }
        entry.count = entry.count.checked_add(1).ok_or_else(|| {
            PlatformError::new(ErrorCode::Internal, "deployment pin count overflow")
        })?;
        Ok(DeploymentPin {
            deployment_id,
            inner: self.inner.clone(),
            released: false,
        })
    }

    /// Conservatively retain one deployment until this platform process and workerd generation end.
    ///
    /// Stock workerd does not expose an acknowledgement that every tenant `waitUntil()` task
    /// completed. Retaining the immutable deployment for the owning process lifetime prevents
    /// deletion from racing background execution without guessing a time-to-live.
    pub fn retain_until_restart(&self, deployment_id: DeploymentId) -> Result<(), PlatformError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(deployment_id).or_default();
        if entry.fenced {
            return Err(PlatformError::new(
                ErrorCode::DeploymentNotReady,
                "deployment is fenced for deletion",
            ));
        }
        entry.retained_until_restart = true;
        Ok(())
    }

    /// Fence new pins and wait for current work to drain to zero.
    pub async fn fence_and_wait(
        &self,
        deployment_id: DeploymentId,
        deadline: Duration,
    ) -> Result<(), PlatformError> {
        {
            let mut entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.entry(deployment_id).or_default().fenced = true;
        }
        let wait = async {
            loop {
                let notified = self.inner.changed.notified();
                let empty = self
                    .inner
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&deployment_id)
                    .is_none_or(|entry| entry.count == 0 && !entry.retained_until_restart);
                if empty {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(deadline, wait).await.map_err(|_| {
            PlatformError::new(
                ErrorCode::DeploymentReferenced,
                "deployment still has in-flight requests",
            )
        })
    }

    /// Remove a fence when the database transition could not be committed.
    pub fn unfence(&self, deployment_id: DeploymentId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&deployment_id) {
            entry.fenced = false;
            if entry.count == 0 && !entry.retained_until_restart {
                entries.remove(&deployment_id);
            }
        }
        self.inner.changed.notify_waiters();
    }

    /// Forget a drained fence after `SQLite` committed the deployment tombstone.
    ///
    /// A stale route snapshot can no longer reach tenant code because
    /// `RuntimeSource` independently rejects tombstoned deployments.
    pub fn retire_fence(&self, deployment_id: DeploymentId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(&deployment_id)
            .is_some_and(|entry| entry.fenced && entry.count == 0 && !entry.retained_until_restart)
        {
            entries.remove(&deployment_id);
        }
        self.inner.changed.notify_waiters();
    }

    /// Return the current in-flight count for tests and diagnostics.
    #[must_use]
    pub fn count(&self, deployment_id: DeploymentId) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&deployment_id)
            .map_or(0, |entry| {
                entry.count + usize::from(entry.retained_until_restart)
            })
    }
}

/// RAII deployment execution pin.
pub struct DeploymentPin {
    deployment_id: DeploymentId,
    inner: Arc<Inner>,
    released: bool,
}

impl std::fmt::Debug for DeploymentPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentPin")
            .field("deployment_id", &self.deployment_id)
            .finish_non_exhaustive()
    }
}

impl Drop for DeploymentPin {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&self.deployment_id) {
            entry.count = entry.count.saturating_sub(1);
            if entry.count == 0 && !entry.fenced && !entry.retained_until_restart {
                entries.remove(&self.deployment_id);
            }
        }
        drop(entries);
        self.inner.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "pins_tests.rs"]
mod tests;
