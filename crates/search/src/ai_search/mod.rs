//! AI Search chunking, keyword-query, and retrieval algorithms.

mod chunk;
mod fts;
mod model;
mod retrieval;

pub use chunk::{ChunkConfig, ChunkError, TextChunk, chunk_text};
pub use fts::{KeywordMatchMode, KeywordQueryError, build_fts_query};
pub use model::{FusionMethod, RankedCandidate, ScoredCandidate};
pub use retrieval::{RetrievalError, cosine_similarity, fuse_candidates};

#[cfg(test)]
mod tests;
