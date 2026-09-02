use crate::{DistanceMetric, MAX_TOP_K, PreparedQuery, SearchError};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// One applied vector candidate selected after namespace and metadata pre-filtering.
#[derive(Clone, Copy, Debug)]
pub struct ExactCandidate<'a> {
    /// Stable vector identifier used for deterministic ties.
    pub id: &'a str,
    /// Authoritative f32 values.
    pub values: &'a [f32],
}

/// One deterministic exact search result.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoredVector {
    /// Stable vector identifier.
    pub id: String,
    /// Public metric score.
    pub score: f64,
}

#[derive(Clone, Debug)]
struct HeapEntry {
    id: String,
    score: f64,
    rank_score: f64,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.score.total_cmp(&other.score) == Ordering::Equal && self.id == other.id
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .rank_score
            .total_cmp(&self.rank_score)
            .then_with(|| self.id.cmp(&other.id))
    }
}

/// Score a pre-filtered iterator with fixed `O(topK)` heap memory.
pub fn exact_top_k<'a>(
    metric: DistanceMetric,
    query: &[f32],
    candidates: impl IntoIterator<Item = ExactCandidate<'a>>,
    top_k: usize,
) -> Result<Vec<ScoredVector>, SearchError> {
    let mut accumulator = ExactTopK::new(metric, query, top_k)?;
    for candidate in candidates {
        accumulator.push(candidate)?;
    }
    Ok(accumulator.finish())
}

/// Incremental exact top-k accumulator with fixed `O(topK)` memory.
#[derive(Clone, Debug)]
pub struct ExactTopK {
    metric: DistanceMetric,
    prepared: PreparedQuery,
    top_k: usize,
    heap: BinaryHeap<HeapEntry>,
}

impl ExactTopK {
    /// Validate one query and allocate only the requested result heap.
    pub fn new(metric: DistanceMetric, query: &[f32], top_k: usize) -> Result<Self, SearchError> {
        if top_k == 0 || top_k > MAX_TOP_K {
            return Err(SearchError::InvalidTopK);
        }
        Ok(Self {
            metric,
            prepared: PreparedQuery::new(metric, query, query.len())?,
            top_k,
            heap: BinaryHeap::with_capacity(top_k),
        })
    }

    /// Score one already pre-filtered candidate.
    pub fn push(&mut self, candidate: ExactCandidate<'_>) -> Result<(), SearchError> {
        let score = self.prepared.score(candidate.values)?;
        let rank_score = match self.metric {
            DistanceMetric::Cosine | DistanceMetric::DotProduct => score,
            DistanceMetric::Euclidean => -score,
        };
        let should_insert = self.heap.len() < self.top_k
            || self
                .heap
                .peek()
                .is_some_and(|worst| better(rank_score, candidate.id, worst));
        if should_insert {
            let entry = HeapEntry {
                id: candidate.id.to_string(),
                score,
                rank_score,
            };
            if self.heap.len() == self.top_k {
                let _ = self.heap.pop();
            }
            self.heap.push(entry);
        }
        Ok(())
    }

    /// Return results in metric order with stable ID ties.
    #[must_use]
    pub fn finish(self) -> Vec<ScoredVector> {
        let mut results = self
            .heap
            .into_iter()
            .map(|entry| ScoredVector {
                id: entry.id,
                score: entry.score,
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| {
            self.metric
                .compare_scores(right.score, left.score)
                .then_with(|| left.id.cmp(&right.id))
        });
        results
    }
}

fn better(rank_score: f64, id: &str, right: &HeapEntry) -> bool {
    rank_score > right.rank_score || (rank_score == right.rank_score && id < right.id.as_str())
}
