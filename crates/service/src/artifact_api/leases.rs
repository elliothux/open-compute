//! In-process repository leases coordinating reads, pushes, forks, and deletion.

use open_compute_core::{ArtifactRepoId, ErrorCode, PlatformError};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub(super) struct RepositoryLeases {
    active: Mutex<HashMap<ArtifactRepoId, usize>>,
    changed: Condvar,
}

#[derive(Debug)]
pub(crate) struct RepositoryLease {
    registry: Arc<RepositoryLeases>,
    repository: ArtifactRepoId,
}

impl RepositoryLeases {
    pub(super) fn acquire(
        self: &Arc<Self>,
        repository: ArtifactRepoId,
    ) -> Result<RepositoryLease, PlatformError> {
        let mut active = self.active.lock().map_err(|_| unavailable())?;
        let count = active.entry(repository).or_default();
        *count = count.checked_add(1).ok_or_else(unavailable)?;
        Ok(RepositoryLease {
            registry: Arc::clone(self),
            repository,
        })
    }

    pub(super) fn drain(
        &self,
        repository: ArtifactRepoId,
        timeout: Duration,
    ) -> Result<(), PlatformError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(unavailable)?;
        let mut active = self.active.lock().map_err(|_| unavailable())?;
        while active.get(&repository).copied().unwrap_or_default() != 0 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(unavailable)?;
            let (next, wait) = self
                .changed
                .wait_timeout(active, remaining)
                .map_err(|_| unavailable())?;
            active = next;
            if wait.timed_out() && active.get(&repository).copied().unwrap_or_default() != 0 {
                return Err(PlatformError::new(
                    ErrorCode::ResourceUnavailable,
                    "Artifact repository still has active operations",
                ));
            }
        }
        Ok(())
    }
}

impl Drop for RepositoryLease {
    fn drop(&mut self) {
        let Ok(mut active) = self.registry.active.lock() else {
            return;
        };
        let Some(count) = active.get_mut(&self.repository) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            active.remove(&self.repository);
            self.registry.changed.notify_all();
        }
    }
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceUnavailable,
        "Artifact repository lease service is unavailable",
    )
}
