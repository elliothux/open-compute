//! Exact vector scoring and deterministic hybrid candidate fusion.

use super::{FusionMethod, RankedCandidate, ScoredCandidate};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

const RRF_K: f32 = 60.0;

/// Invalid vector or ranked-candidate input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetrievalError {
    /// Query and candidate vector dimensions differ or are empty.
    DimensionMismatch,
    /// A vector value or score was NaN, infinite, or outside its declared range.
    NonFiniteValue,
    /// A cosine vector had zero norm.
    ZeroNorm,
    /// A branch contained the same chunk identity more than once.
    DuplicateCandidate,
    /// The requested result count was zero or above the public maximum.
    InvalidLimit,
}

impl Display for RetrievalError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DimensionMismatch => formatter.write_str("vector dimensions do not match"),
            Self::NonFiniteValue => formatter.write_str("retrieval input is not finite"),
            Self::ZeroNorm => formatter.write_str("cosine vector has zero norm"),
            Self::DuplicateCandidate => formatter.write_str("retrieval branch has a duplicate"),
            Self::InvalidLimit => formatter.write_str("retrieval result limit is invalid"),
        }
    }
}

impl std::error::Error for RetrievalError {}

/// Compute exact cosine similarity without accepting malformed or zero vectors.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f32, RetrievalError> {
    if left.is_empty() || left.len() != right.len() {
        return Err(RetrievalError::DimensionMismatch);
    }
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (&left_value, &right_value) in left.iter().zip(right) {
        if !left_value.is_finite() || !right_value.is_finite() {
            return Err(RetrievalError::NonFiniteValue);
        }
        let left_value = f64::from(left_value);
        let right_value = f64::from(right_value);
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return Err(RetrievalError::ZeroNorm);
    }
    let score = dot / (left_norm.sqrt() * right_norm.sqrt());
    if !score.is_finite() {
        return Err(RetrievalError::NonFiniteValue);
    }
    Ok(score.clamp(-1.0, 1.0) as f32)
}

#[derive(Default)]
struct Branches {
    vector: Option<(usize, f32)>,
    keyword: Option<(usize, f32)>,
}

/// Fuse two already ranked branches, rejecting invalid scores and duplicate
/// identities within a branch. Results are ordered by descending score and
/// stable chunk identity.
pub fn fuse_candidates(
    vector: &[RankedCandidate],
    keyword: &[RankedCandidate],
    method: FusionMethod,
    max_results: usize,
    score_threshold: f32,
) -> Result<Vec<ScoredCandidate>, RetrievalError> {
    if max_results == 0 || max_results > 50 {
        return Err(RetrievalError::InvalidLimit);
    }
    if !score_threshold.is_finite() || !(0.0..=1.0).contains(&score_threshold) {
        return Err(RetrievalError::NonFiniteValue);
    }
    let mut joined = BTreeMap::<String, Branches>::new();
    insert_branch(&mut joined, vector, true)?;
    insert_branch(&mut joined, keyword, false)?;
    let mut result = joined
        .into_iter()
        .map(|(chunk_id, branches)| {
            let score = match method {
                FusionMethod::ReciprocalRank => {
                    let raw = branches
                        .vector
                        .map_or(0.0, |(rank, _)| 1.0 / (RRF_K + rank as f32))
                        + branches
                            .keyword
                            .map_or(0.0, |(rank, _)| 1.0 / (RRF_K + rank as f32));
                    raw / (2.0 / (RRF_K + 1.0))
                }
                FusionMethod::Maximum => branches
                    .vector
                    .map(|(_, score)| score)
                    .into_iter()
                    .chain(branches.keyword.map(|(_, score)| score))
                    .fold(0.0_f32, f32::max),
            };
            ScoredCandidate {
                chunk_id,
                score,
                vector_rank: branches.vector.map(|value| value.0),
                vector_score: branches.vector.map(|value| value.1),
                keyword_rank: branches.keyword.map(|value| value.0),
                keyword_score: branches.keyword.map(|value| value.1),
            }
        })
        .filter(|candidate| candidate.score >= score_threshold)
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    result.truncate(max_results);
    Ok(result)
}

fn insert_branch(
    joined: &mut BTreeMap<String, Branches>,
    candidates: &[RankedCandidate],
    vector: bool,
) -> Result<(), RetrievalError> {
    for (index, candidate) in candidates.iter().enumerate() {
        if !candidate.score.is_finite() || !(0.0..=1.0).contains(&candidate.score) {
            return Err(RetrievalError::NonFiniteValue);
        }
        let branches = joined.entry(candidate.chunk_id.clone()).or_default();
        let slot = if vector {
            &mut branches.vector
        } else {
            &mut branches.keyword
        };
        if slot.replace((index + 1, candidate.score)).is_some() {
            return Err(RetrievalError::DuplicateCandidate);
        }
    }
    Ok(())
}
