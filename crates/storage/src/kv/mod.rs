//! Workers KV catalog, secure paths, and namespace-local SQLite engine.

mod catalog;
mod engine;
mod paths;

pub use catalog::{KvBackupRecord, KvBackupState, KvNamespaceRecord, KvNamespaceRepository};
pub use engine::{
    KV_CAPABILITY_VERSION, KV_DEFAULT_LIST_LIMIT, KV_MAX_KEY_BYTES, KV_MAX_LIST_LIMIT,
    KV_MAX_METADATA_BYTES, KV_MAX_MULTI_GET_KEYS, KV_MAX_MULTI_GET_RESPONSE_BYTES,
    KV_MAX_VALUE_BYTES, KV_MIN_CACHE_TTL_SECONDS, KV_MIN_EXPIRATION_TTL_SECONDS, KV_SCHEMA_VERSION,
    KvEngine, KvEntry, KvEntryInfo, KvListPage, KvListRow, KvPutOptions, canonical_metadata,
    validate_key,
};
pub use paths::KvPaths;
