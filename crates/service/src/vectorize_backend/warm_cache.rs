//! Host-wide immutable Vectorize snapshots with a bounded weighted LRU.

use open_compute_core::PlatformError;
use open_compute_storage::{VectorRecord, VectorizeEngine};
use std::collections::{BTreeMap, VecDeque};
use std::mem::size_of;
use std::sync::{Arc, Mutex, OnceLock};

const WARM_CACHE_BYTES: usize = 512 * 1024 * 1024;

static VECTOR_WARM_CACHE: OnceLock<Mutex<WarmCache>> = OnceLock::new();
static VECTOR_WARM_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WarmCacheKey {
    resource_id: String,
    processed_sequence: u64,
    metadata_generation: u64,
}

#[derive(Debug)]
pub(super) struct WarmSnapshot {
    pub(super) records: Vec<VectorRecord>,
    weight_bytes: usize,
}

#[derive(Debug)]
struct WarmCache {
    entries: BTreeMap<WarmCacheKey, Arc<WarmSnapshot>>,
    order: VecDeque<WarmCacheKey>,
    weight_bytes: usize,
    limit_bytes: usize,
}

impl WarmCache {
    fn new(limit_bytes: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            weight_bytes: 0,
            limit_bytes,
        }
    }

    fn get(&mut self, key: &WarmCacheKey) -> Option<Arc<WarmSnapshot>> {
        let value = self.entries.get(key)?.clone();
        self.order.retain(|candidate| candidate != key);
        self.order.push_back(key.clone());
        Some(value)
    }

    fn insert(&mut self, key: WarmCacheKey, snapshot: Arc<WarmSnapshot>) {
        if snapshot.weight_bytes > self.limit_bytes {
            return;
        }
        let stale = self
            .entries
            .keys()
            .filter(|candidate| candidate.resource_id == key.resource_id)
            .cloned()
            .collect::<Vec<_>>();
        for candidate in stale {
            self.remove(&candidate);
        }
        while self
            .weight_bytes
            .checked_add(snapshot.weight_bytes)
            .is_none_or(|weight| weight > self.limit_bytes)
        {
            let Some(oldest) = self.order.pop_front() else {
                return;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.weight_bytes = self.weight_bytes.saturating_sub(removed.weight_bytes);
            }
        }
        self.weight_bytes = self.weight_bytes.saturating_add(snapshot.weight_bytes);
        self.order.push_back(key.clone());
        self.entries.insert(key, snapshot);
    }

    fn remove(&mut self, key: &WarmCacheKey) {
        self.order.retain(|candidate| candidate != key);
        if let Some(removed) = self.entries.remove(key) {
            self.weight_bytes = self.weight_bytes.saturating_sub(removed.weight_bytes);
        }
    }
}

pub(super) fn load_warm_snapshot(
    engine: &VectorizeEngine,
    resource_id: &str,
) -> Result<Option<Arc<WarmSnapshot>>, PlatformError> {
    let description = engine.describe()?;
    let minimum_weight = usize::try_from(description.vector_count)
        .ok()
        .and_then(|count| {
            usize::try_from(description.dimensions)
                .ok()
                .and_then(|dimensions| count.checked_mul(dimensions)?.checked_mul(4))
        })
        .unwrap_or(usize::MAX);
    if minimum_weight > WARM_CACHE_BYTES {
        return Ok(None);
    }
    let key = WarmCacheKey {
        resource_id: resource_id.to_string(),
        processed_sequence: description.processed_sequence,
        metadata_generation: description.metadata_generation,
    };
    let cache = VECTOR_WARM_CACHE.get_or_init(|| Mutex::new(WarmCache::new(WARM_CACHE_BYTES)));
    if let Some(snapshot) = cache.lock().map_err(|_| super::unavailable())?.get(&key) {
        return Ok(Some(snapshot));
    }

    let _build = VECTOR_WARM_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| super::unavailable())?;
    if let Some(snapshot) = cache.lock().map_err(|_| super::unavailable())?.get(&key) {
        return Ok(Some(snapshot));
    }

    let mut records = Vec::with_capacity(
        usize::try_from(description.vector_count)
            .unwrap_or(0)
            .min(100_000),
    );
    let mut weight_bytes = 0_usize;
    let mut over_budget = false;
    engine.scan_candidates(None, None, |record| {
        if over_budget {
            return Ok(());
        }
        let metadata_bytes = record
            .metadata
            .as_ref()
            .and_then(|value| serde_json::to_vec(value).ok())
            .map_or(0, |bytes| bytes.len());
        let record_weight = record
            .values
            .len()
            .checked_mul(size_of::<f32>())
            .and_then(|bytes| bytes.checked_add(record.id.len()))
            .and_then(|bytes| bytes.checked_add(record.namespace.as_ref().map_or(0, String::len)))
            .and_then(|bytes| bytes.checked_add(metadata_bytes))
            .unwrap_or(usize::MAX);
        let next = weight_bytes.saturating_add(record_weight);
        if next > WARM_CACHE_BYTES {
            records.clear();
            over_budget = true;
        } else {
            weight_bytes = next;
            records.push(record);
        }
        Ok(())
    })?;
    if over_budget {
        return Ok(None);
    }

    let latest = engine.describe()?;
    if latest.processed_sequence != description.processed_sequence
        || latest.metadata_generation != description.metadata_generation
    {
        return Ok(None);
    }
    let snapshot = Arc::new(WarmSnapshot {
        records,
        weight_bytes,
    });
    cache
        .lock()
        .map_err(|_| super::unavailable())?
        .insert(key, snapshot.clone());
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(resource_id: &str, processed_sequence: u64) -> WarmCacheKey {
        WarmCacheKey {
            resource_id: resource_id.to_string(),
            processed_sequence,
            metadata_generation: 0,
        }
    }

    fn snapshot(weight_bytes: usize) -> Arc<WarmSnapshot> {
        Arc::new(WarmSnapshot {
            records: Vec::new(),
            weight_bytes,
        })
    }

    #[test]
    fn weighted_lru_replaces_stale_resource_generations() {
        let mut cache = WarmCache::new(10);
        let first = key("first", 1);
        let second = key("second", 1);
        cache.insert(first.clone(), snapshot(6));
        cache.insert(second.clone(), snapshot(4));
        assert!(cache.get(&first).is_some());

        let third = key("third", 1);
        cache.insert(third.clone(), snapshot(4));
        assert!(cache.get(&second).is_none());
        assert!(cache.get(&first).is_some());
        assert!(cache.get(&third).is_some());

        let next_generation = key("first", 2);
        cache.insert(next_generation.clone(), snapshot(5));
        assert!(cache.get(&first).is_none());
        assert!(cache.get(&next_generation).is_some());
        assert!(cache.weight_bytes <= cache.limit_bytes);
    }

    #[test]
    fn snapshot_larger_than_budget_is_refused() {
        let mut cache = WarmCache::new(10);
        let oversized = key("oversized", 1);
        cache.insert(oversized.clone(), snapshot(11));
        assert!(cache.get(&oversized).is_none());
        assert_eq!(cache.weight_bytes, 0);
    }
}
