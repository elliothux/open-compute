//! Pure, bounded search algorithms shared by Vectorize and AI Search.
//!
//! This crate owns no database, filesystem, network, process, or tenant authority.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod ai_search;
mod metadata;
mod metric;
mod top_k;
mod vector;

pub use metadata::{
    FilterExpr, FilterOperator, FilterPredicate, IndexedMetadata, MetadataScalar, MetadataValue,
    compile_filter, validate_metadata,
};
pub use metric::{DistanceMetric, PreparedQuery, normalize_public_score, raw_score};
pub use top_k::{ExactCandidate, ExactTopK, ScoredVector, exact_top_k};
pub use vector::{decode_f32le, encode_f32le, validate_vector};

use std::fmt::{Display, Formatter};

/// Maximum dimensions accepted by the current Vectorize contract.
pub const MAX_VECTOR_DIMENSIONS: usize = 1_536;
/// Maximum UTF-8 bytes in one vector identifier.
pub const MAX_VECTOR_ID_BYTES: usize = 64;
/// Maximum UTF-8 bytes in one namespace.
pub const MAX_NAMESPACE_BYTES: usize = 64;
/// Maximum canonical JSON bytes stored as metadata for one vector.
pub const MAX_METADATA_BYTES: usize = 10 * 1_024;
/// Maximum metadata indexes and filter predicates per index.
pub const MAX_METADATA_PREDICATES: usize = 10;
/// Maximum public query result count.
pub const MAX_TOP_K: usize = 100;
/// Maximum public query result count when values or all metadata are returned.
pub const MAX_TOP_K_WITH_VALUES: usize = 50;

/// A stable, content-free validation or algorithm error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchError {
    /// A vector dimension count is zero, too large, or differs from the index.
    DimensionMismatch,
    /// One vector component is NaN or infinite.
    NonFiniteVector,
    /// Persisted little-endian vector bytes have an invalid length.
    InvalidVectorEncoding,
    /// A distance metric token is not part of the closed contract.
    InvalidMetric,
    /// A vector identifier or namespace is outside the public UTF-8 limits.
    InvalidIdentity,
    /// Metadata is not a bounded supported object.
    InvalidMetadata,
    /// A metadata filter is malformed, unindexed, or outside its hard limits.
    InvalidFilter,
    /// `topK` is zero or above the active public response limit.
    InvalidTopK,
}

impl Display for SearchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DimensionMismatch => "vector dimensions do not match the index",
            Self::NonFiniteVector => "vector contains a non-finite component",
            Self::InvalidVectorEncoding => "persisted vector encoding is invalid",
            Self::InvalidMetric => "vector distance metric is invalid",
            Self::InvalidIdentity => "vector identity is invalid",
            Self::InvalidMetadata => "vector metadata is invalid",
            Self::InvalidFilter => "vector metadata filter is invalid",
            Self::InvalidTopK => "vector query topK is invalid",
        })
    }
}

impl std::error::Error for SearchError {}

#[cfg(test)]
mod tests;
