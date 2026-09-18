//! Generation-scoped inherited socket handoff for the host-extension broker.

use open_compute_core::{ErrorCode, PlatformError, StartupId};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
struct State {
    pending: Option<(StartupId, UnixStream)>,
}

/// Publishes exactly one private broker socket for each workerd generation.
#[derive(Clone, Default)]
pub struct HostExtensionBrokerRegistry {
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
}

impl std::fmt::Debug for HostExtensionBrokerRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostExtensionBrokerRegistry")
            .field(
                "pending_generation",
                &self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pending
                    .as_ref()
                    .map(|(generation, _)| generation),
            )
            .finish()
    }
}

impl HostExtensionBrokerRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn prepare(&self, generation: StartupId) -> Result<OwnedFd, PlatformError> {
        let (parent, child) = UnixStream::pair().map_err(|_| {
            PlatformError::new(
                ErrorCode::RuntimeInvalid,
                "failed to create host-extension broker socket",
            )
        })?;
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending = Some((generation, parent));
        self.changed.notify_waiters();
        Ok(child.into())
    }

    /// Take the next workerd generation's broker socket.
    pub async fn take(&self) -> (StartupId, UnixStream) {
        loop {
            let changed = self.changed.notified();
            if let Some(value) = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pending
                .take()
            {
                return value;
            }
            changed.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[tokio::test]
    async fn publishes_one_connected_socket_for_the_generation() {
        let registry = HostExtensionBrokerRegistry::new();
        let generation = StartupId::generate();
        let child = registry.prepare(generation).unwrap();
        let (published, mut parent) = registry.take().await;
        let mut child = UnixStream::from(child);
        child.write_all(b"x").unwrap();
        let mut byte = [0];
        parent.read_exact(&mut byte).unwrap();
        assert_eq!(published, generation);
        assert_eq!(byte, *b"x");
    }
}
