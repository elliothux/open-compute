//! Vectorize control authority, secure paths, and per-index SQLite engine.

mod catalog;
mod engine;
mod paths;

pub use catalog::{VectorizeIndexRecord, VectorizeIndexRepository};
pub use engine::{
    VECTORIZE_SCHEMA_VERSION, VectorMutation, VectorMutationInput, VectorMutationKind,
    VectorMutationState, VectorRecord, VectorizeDescription, VectorizeEngine,
    VectorizeReadSnapshot,
};
pub use paths::VectorizePaths;

#[cfg(test)]
mod tests;
