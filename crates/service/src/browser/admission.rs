//! Browser session admission retains capacity until native allocation and cleanup finish.

use super::*;

impl BrowserService {
    /// Publish an opaque ready session only after native CDP readiness under the acquire deadline.
    pub async fn acquire(self: &Arc<Self>, keep_alive_ms: u64) -> Result<String, PlatformError> {
        let observation = self.metrics.browser_operation(BrowserOperation::Acquire);
        let result = async {
            if !self.is_available() {
                return Err(backend::unavailable());
            }
            if !(10_000..=1_200_000).contains(&keep_alive_ms) {
                return Err(invalid());
            }
            let pending = self
                .pending
                .clone()
                .try_acquire_owned()
                .map_err(|_| limit())?;
            let (mut reply, response) = tokio::sync::oneshot::channel();
            let service = self.clone();
            // Cancellation retains admission capacity until native allocation and cleanup complete.
            tokio::spawn(async move {
                let _pending = pending;
                let result = service.acquire_inner(keep_alive_ms, &mut reply).await;
                if let Err(Ok(id)) = reply.send(result) {
                    let _ = service.close(&id, false).await;
                }
            });
            tokio::time::timeout(
                Duration::from_millis(self.config.acquire_timeout_ms),
                response,
            )
            .await
            .map_err(|_| timeout())?
            .map_err(|_| backend::unavailable())?
        }
        .await;
        observation.finish(BrowserOutcome::result(&result));
        result
    }

    async fn acquire_inner(
        &self,
        keep_alive_ms: u64,
        reply: &mut tokio::sync::oneshot::Sender<Result<String, PlatformError>>,
    ) -> Result<String, PlatformError> {
        let capacity = tokio::select! {
            capacity = self.capacity.clone().acquire_owned() => capacity.map_err(|_| backend::unavailable())?,
            _ = reply.closed() => return Err(timeout()),
        };
        if self.stopped.load(Ordering::Acquire) {
            return Err(backend::unavailable());
        }
        let (cdp, contract, generation, managed) = if let Some(manager) = &self.manager {
            let generation = manager.acquire().await?;
            let managed = ManagedBrowserSession::open(generation.clone()).await?;
            (
                managed.connect(None).await?,
                generation.contract(),
                generation.id().to_owned(),
                Some(managed),
            )
        } else {
            let backend = BrowserBackend::connect(&self.config, None).await?;
            (backend.cdp, backend.contract, self.generation.clone(), None)
        };

        if reply.is_closed() {
            if let Some(managed) = managed {
                managed.close().await?;
            }
            return Err(timeout());
        }
        let id = uuid::Uuid::now_v7().to_string();
        let record = BrowserSessionRecord {
            id: id.clone(),
            instance_id: self.instance,
            generation: generation.clone(),
            contract_sha256: contract,
            state: BrowserSessionState::Ready,
            keep_alive_ms,
            connections: 0,
            created_at_ms: now_ms(),
            last_activity_at_ms: now_ms(),
            connected_at_ms: None,
            closed_at_ms: None,
            close_reason: None,
        };
        let mut sessions = self.sessions.lock().map_err(|_| backend::unavailable())?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(backend::unavailable());
        }
        BrowserSessions::new(self.storage.db()).create(&record, self.config.max_sessions)?;
        sessions.insert(
            id.clone(),
            Arc::new(Session {
                generation,
                cdp,
                managed,
                activity: Mutex::new(Instant::now()),
                connections: Mutex::new(BTreeMap::new()),
                contract,
                inflight: AtomicUsize::new(0),
                is_action: AtomicBool::new(false),
                keep_alive: Duration::from_millis(keep_alive_ms),
                _capacity: capacity,
            }),
        );
        self.metrics.set_browser_sessions(sessions.len() as u64);
        Ok(id)
    }
}
