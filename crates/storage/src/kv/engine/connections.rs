use super::*;
use std::os::unix::fs::MetadataExt as _;
use std::sync::{Condvar, Mutex};

#[derive(Debug)]
struct CachedConnection {
    path: PathBuf,
    identity: (u64, u64),
    connection: Arc<Mutex<Connection>>,
    active: bool,
}

/// Bounded SQLite connections shared by live KV namespace engines.
#[derive(Debug)]
pub struct KvConnectionPool {
    limit: usize,
    cached: Mutex<Vec<CachedConnection>>,
    available: Condvar,
}

impl KvConnectionPool {
    /// Set the maximum total open connections, including idle connections.
    #[must_use]
    pub fn new(limit: u32) -> Self {
        Self {
            limit: usize::try_from(limit.max(1)).unwrap_or(1),
            cached: Mutex::new(Vec::new()),
            available: Condvar::new(),
        }
    }

    fn acquire(&self, engine: &KvEngine, identity: (u64, u64)) -> Result<Lease<'_>, PlatformError> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut cached = self.cached.lock().map_err(|_| corrupt())?;
        loop {
            if cached
                .iter()
                .any(|entry| entry.path == engine.path && entry.identity != identity)
            {
                return Err(corrupt());
            }
            if let Some(entry) = cached
                .iter_mut()
                .find(|entry| !entry.active && entry.path == engine.path)
            {
                entry.active = true;
                return Ok(Lease {
                    pool: self,
                    connection: Some(entry.connection.clone()),
                });
            }
            if cached.len() >= self.limit {
                if let Some(index) = cached.iter().position(|entry| !entry.active) {
                    cached.remove(index);
                } else {
                    let (next, timeout) = self
                        .available
                        .wait_timeout(
                            cached,
                            deadline.saturating_duration_since(std::time::Instant::now()),
                        )
                        .map_err(|_| corrupt())?;
                    cached = next;
                    if timeout.timed_out() {
                        return Err(PlatformError::new(
                            ErrorCode::KvBusy,
                            "KV connection limit is temporarily saturated",
                        ));
                    }
                    continue;
                }
            }
            let connection = Arc::new(Mutex::new(engine.open_connection()?));
            cached.push(CachedConnection {
                path: engine.path.clone(),
                identity,
                connection: connection.clone(),
                active: true,
            });
            return Ok(Lease {
                pool: self,
                connection: Some(connection),
            });
        }
    }

    fn evict_idle(&self, path: &Path) {
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cached.retain(|entry| entry.active || entry.path != path);
        self.available.notify_all();
    }
}

struct Lease<'a> {
    pool: &'a KvConnectionPool,
    connection: Option<Arc<Mutex<Connection>>>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let mut cached = self
            .pool
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(connection) = self.connection.take() {
            if let Some(entry) = cached
                .iter_mut()
                .find(|entry| Arc::ptr_eq(&entry.connection, &connection))
            {
                entry.active = false;
            }
            // Release the lease's reference before another caller can evict this slot.
            drop(connection);
        }
        self.pool.available.notify_all();
    }
}

impl KvEngine {
    pub(super) fn with_connection<T>(
        &self,
        write: bool,
        operation: impl FnOnce(&mut Connection) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        fs::validate_owned_file(&self.path, true)?;
        let fd = fs::open_nofollow(&self.path, false, write)?;
        fs::validate_authority_fd(&fd)?;
        let metadata = fd.metadata().map_err(|_| storage_unavailable())?;
        let identity = (metadata.dev(), metadata.ino());
        let lease = self.owner.pool.acquire(self, identity)?;
        let connection = lease.connection.as_ref().ok_or_else(invariant)?;
        let mut connection = connection.lock().map_err(|_| corrupt())?;
        apply_quota(&connection, self.quota_bytes)?;
        verify_identity(&connection, self.instance_id, self.resource_id)?;
        verify_schema(&connection)?;
        operation(&mut connection)
    }
}

#[derive(Debug)]
pub(super) struct ConnectionOwner {
    pub(super) path: PathBuf,
    pub(super) pool: Arc<KvConnectionPool>,
}

impl Drop for ConnectionOwner {
    fn drop(&mut self) {
        self.pool.evict_idle(&self.path);
    }
}
