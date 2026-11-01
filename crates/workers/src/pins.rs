//! In-process version dispatch pins and deletion fence.

use open_compute_core::{DeploymentId, ErrorCode, PlatformError, RequestId, VersionId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct Entry {
    count: usize,
    deployments: HashMap<DeploymentId, usize>,
    fenced: bool,
    retained_until_restart: bool,
    // Separate acknowledgements keep one drained activation from releasing concurrent or cancelled work.
    awaiting_drain: HashSet<RequestId>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: Mutex<HashMap<VersionId, Entry>>,
    changed: Notify,
}

/// Process-local authority that freezes version identity for in-flight work.
#[derive(Clone, Debug, Default)]
pub struct VersionPins {
    inner: Arc<Inner>,
}

impl VersionPins {
    /// Construct an empty pin registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire a pin unless deletion already fenced the version.
    pub fn pin(&self, version_id: VersionId) -> Result<VersionPin, PlatformError> {
        self.pin_inner(version_id, None)
    }

    /// Acquire an active deployment pin for runtime-incident attribution.
    pub fn pin_deployment(
        &self,
        version_id: VersionId,
        deployment_id: DeploymentId,
    ) -> Result<VersionPin, PlatformError> {
        self.pin_inner(version_id, Some(deployment_id))
    }

    fn pin_inner(
        &self,
        version_id: VersionId,
        deployment_id: Option<DeploymentId>,
    ) -> Result<VersionPin, PlatformError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(version_id).or_default();
        if entry.fenced {
            return Err(PlatformError::new(
                ErrorCode::VersionNotReady,
                "version is fenced for deletion",
            ));
        }
        entry.count = entry
            .count
            .checked_add(1)
            .ok_or_else(|| PlatformError::new(ErrorCode::Internal, "version pin count overflow"))?;
        if let Some(deployment_id) = deployment_id {
            let count = entry.deployments.entry(deployment_id).or_default();
            *count = count.checked_add(1).ok_or_else(|| {
                PlatformError::new(ErrorCode::Internal, "deployment pin count overflow")
            })?;
        }
        Ok(VersionPin {
            version_id,
            deployment_id,
            inner: self.inner.clone(),
            released: false,
        })
    }

    /// Return active deployment identities for the current runtime generation.
    #[must_use]
    pub fn active_deployments(&self) -> Vec<DeploymentId> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .flat_map(|entry| entry.deployments.keys().copied())
            .collect()
    }

    /// Conservatively retain one version until this platform process and workerd generation end.
    ///
    /// Stock workerd does not expose an acknowledgement that every tenant `waitUntil()` task
    /// completed. Retaining the immutable version for the owning process lifetime prevents
    /// deletion from racing background execution without guessing a time-to-live.
    pub fn retain_until_restart(&self, version_id: VersionId) -> Result<(), PlatformError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(version_id).or_default();
        if entry.fenced {
            return Err(PlatformError::new(
                ErrorCode::VersionNotReady,
                "version is fenced for deletion",
            ));
        }
        entry.retained_until_restart = true;
        Ok(())
    }

    /// Retain a dispatched activation until its exact trusted drain acknowledgement or generation exit.
    pub fn retain_until_drain(
        &self,
        version_id: VersionId,
        request_id: RequestId,
    ) -> Result<(), PlatformError> {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.entry(version_id).or_default();
        if entry.fenced {
            return Err(PlatformError::new(
                ErrorCode::VersionNotReady,
                "version is fenced for deletion",
            ));
        }
        entry.awaiting_drain.insert(request_id);
        Ok(())
    }

    /// Release only the acknowledged activation; late replies cannot release a later generation's work.
    pub fn release_drained(&self, version_id: VersionId, request_id: RequestId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&version_id) {
            entry.awaiting_drain.remove(&request_id);
            if entry.count == 0
                && !entry.fenced
                && !entry.retained_until_restart
                && entry.awaiting_drain.is_empty()
            {
                entries.remove(&version_id);
            }
        }
        drop(entries);
        self.inner.changed.notify_waiters();
    }

    /// Release conservative background-work holds after the supervised workerd generation exits.
    ///
    /// Ordinary RAII pins remain intact because their Rust owners can outlive the child long
    /// enough to drain already-buffered response bodies. Only the generation-scoped fallback
    /// holds are cleared once process supervision has proved that tenant code can no longer run.
    pub fn clear_generation_retentions(&self) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| {
            entry.retained_until_restart = false;
            entry.awaiting_drain.clear();
            entry.count > 0 || entry.fenced
        });
        self.inner.changed.notify_waiters();
    }

    /// Fence new pins and wait for current work to drain to zero.
    pub async fn fence_and_wait(
        &self,
        version_id: VersionId,
        deadline: Duration,
    ) -> Result<(), PlatformError> {
        self.fence_many_and_wait(&[version_id], deadline).await
    }

    /// Atomically fence a Worker version set, then wait for every current pin to drain.
    pub async fn fence_many_and_wait(
        &self,
        version_ids: &[VersionId],
        deadline: Duration,
    ) -> Result<(), PlatformError> {
        {
            let mut entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for version_id in version_ids {
                entries.entry(*version_id).or_default().fenced = true;
            }
        }
        let wait = async {
            loop {
                let notified = self.inner.changed.notified();
                let empty = self
                    .inner
                    .entries
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .filter(|(id, _)| version_ids.contains(id))
                    .all(|(_, entry)| {
                        entry.count == 0
                            && !entry.retained_until_restart
                            && entry.awaiting_drain.is_empty()
                    });
                if empty {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(deadline, wait).await.map_err(|_| {
            PlatformError::new(
                ErrorCode::VersionReferenced,
                "version set still has in-flight requests",
            )
        })
    }

    /// Remove a fence when the database transition could not be committed.
    pub fn unfence(&self, version_id: VersionId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&version_id) {
            entry.fenced = false;
            if entry.count == 0 && !entry.retained_until_restart && entry.awaiting_drain.is_empty()
            {
                entries.remove(&version_id);
            }
        }
        self.inner.changed.notify_waiters();
    }

    /// Forget a drained fence after `SQLite` committed the version tombstone.
    ///
    /// A stale route snapshot can no longer reach tenant code because
    /// `RuntimeSource` independently rejects tombstoned versions.
    pub fn retire_fence(&self, version_id: VersionId) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.get(&version_id).is_some_and(|entry| {
            entry.fenced
                && entry.count == 0
                && !entry.retained_until_restart
                && entry.awaiting_drain.is_empty()
        }) {
            entries.remove(&version_id);
        }
        self.inner.changed.notify_waiters();
    }

    /// Return the current in-flight count for tests and diagnostics.
    #[must_use]
    pub fn count(&self, version_id: VersionId) -> usize {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&version_id)
            .map_or(0, |entry| {
                entry.count + usize::from(entry.retained_until_restart) + entry.awaiting_drain.len()
            })
    }
}

/// RAII version execution pin.
pub struct VersionPin {
    version_id: VersionId,
    deployment_id: Option<DeploymentId>,
    inner: Arc<Inner>,
    released: bool,
}

impl std::fmt::Debug for VersionPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersionPin")
            .field("version_id", &self.version_id)
            .finish_non_exhaustive()
    }
}

impl Drop for VersionPin {
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
        if let Some(entry) = entries.get_mut(&self.version_id) {
            entry.count = entry.count.saturating_sub(1);
            if let Some(deployment_id) = self.deployment_id
                && let Some(count) = entry.deployments.get_mut(&deployment_id)
            {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    entry.deployments.remove(&deployment_id);
                }
            }
            if entry.count == 0
                && !entry.fenced
                && !entry.retained_until_restart
                && entry.awaiting_drain.is_empty()
            {
                entries.remove(&self.version_id);
            }
        }
        drop(entries);
        self.inner.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "pins_tests.rs"]
mod tests;
