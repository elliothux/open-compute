//! Retrieval value types shared by the AI Search algorithms.

/// Candidate-fusion method exposed by AI Search.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FusionMethod {
    /// Reciprocal-rank fusion with the local fixed constant `60`.
    #[default]
    ReciprocalRank,
    /// Maximum of normalized branch scores.
    Maximum,
}

/// One branch-local ranked candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedCandidate {
    /// Stable chunk identity used for joins and deterministic tie-breaking.
    pub chunk_id: String,
    /// Normalized finite branch score in the inclusive range `0..=1`.
    pub score: f32,
}

/// One fused candidate with branch diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoredCandidate {
    /// Stable chunk identity.
    pub chunk_id: String,
    /// Final finite fusion score.
    pub score: f32,
    /// One-based position in the vector branch, if present.
    pub vector_rank: Option<usize>,
    /// Vector branch score, if present.
    pub vector_score: Option<f32>,
    /// One-based position in the keyword branch, if present.
    pub keyword_rank: Option<usize>,
    /// Keyword branch score, if present.
    pub keyword_score: Option<f32>,
}
