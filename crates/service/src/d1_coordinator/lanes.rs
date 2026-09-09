use super::*;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

#[derive(Clone)]
pub(crate) struct D1HandleManager {
    max_open: usize,
    queue_limit: usize,
    idle_ttl: Duration,
    lanes: Arc<Mutex<HashMap<ResourceId, Arc<D1Lane>>>>,
    metrics: Arc<Mutex<Option<Arc<MetricsRegistry>>>>,
}

impl D1HandleManager {
    pub(crate) fn new(global: u32, queue_limit: u32, idle_ttl: Duration) -> Self {
        Self {
            max_open: global.max(1) as usize,
            queue_limit: queue_limit.max(1) as usize,
            idle_ttl,
            lanes: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn set_metrics(&self, metrics: Arc<MetricsRegistry>) {
        *self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(metrics);
    }

    pub(crate) async fn acquire(
        &self,
        resource: ResourceId,
        timeout: Duration,
    ) -> Result<D1LaneLease, PlatformError> {
        let (lane, open_databases) = {
            let mut lanes = self
                .lanes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(lane) = lanes.get(&resource) {
                (lane.clone(), lanes.len())
            } else {
                let now = Instant::now();
                lanes.retain(|_, lane| {
                    lane.queued.load(Ordering::Acquire) > 0
                        || lane.semaphore.available_permits() == 0
                        || now.duration_since(lane.last_used()) < self.idle_ttl
                });
                if lanes.len() >= self.max_open {
                    let candidate = lanes
                        .iter()
                        .filter(|(_, lane)| {
                            lane.queued.load(Ordering::Acquire) == 0
                                && lane.semaphore.available_permits() == 1
                        })
                        .min_by_key(|(_, lane)| lane.last_used())
                        .map(|(id, _)| *id);
                    let Some(candidate) = candidate else {
                        return Err(overloaded());
                    };
                    lanes.remove(&candidate);
                }
                let lane = Arc::new(D1Lane {
                    semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
                    queued: AtomicUsize::new(0),
                    last_used: Mutex::new(now),
                });
                lanes.insert(resource, lane.clone());
                (lane, lanes.len())
            }
        };
        let prior = lane.queued.fetch_add(1, Ordering::AcqRel);
        if let Some(metrics) = self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            metrics.set_d1_open_databases(open_databases as u64);
            metrics.observe_d1_queue_depth(prior.saturating_add(1) as u64);
        }
        if prior >= self.queue_limit {
            lane.queued.fetch_sub(1, Ordering::AcqRel);
            return Err(overloaded());
        }
        let permit = tokio::time::timeout(timeout, lane.semaphore.clone().acquire_owned())
            .await
            .map_err(|_| overloaded())?
            .map_err(|_| overloaded());
        lane.queued.fetch_sub(1, Ordering::AcqRel);
        Ok(D1LaneLease {
            _resource: permit?,
            lane,
        })
    }
}

struct D1Lane {
    semaphore: Arc<tokio::sync::Semaphore>,
    queued: AtomicUsize,
    last_used: Mutex<Instant>,
}

impl D1Lane {
    fn last_used(&self) -> Instant {
        *self
            .last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(crate) struct D1LaneLease {
    _resource: tokio::sync::OwnedSemaphorePermit,
    lane: Arc<D1Lane>,
}

impl Drop for D1LaneLease {
    fn drop(&mut self) {
        *self
            .lane
            .last_used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }
}

fn overloaded() -> PlatformError {
    PlatformError::new(ErrorCode::D1Overloaded, "D1 operation queue is saturated")
}
