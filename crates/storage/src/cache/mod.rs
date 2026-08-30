//! Per-Worker response-cache metadata authority.

mod authority;
mod engine;
mod model;
mod paths;

pub use engine::{CACHE_DATABASE_SCHEMA_VERSION, CacheEngine, CacheManager, CacheStats};
pub use model::{
    CacheBodyRef, CacheHeader, CacheIdentity, CacheLookup, CacheLookupStatus, CacheMethod,
    CachePurge, CachePut, CacheStoredResponse, CacheSurface,
};
pub use paths::CachePaths;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
