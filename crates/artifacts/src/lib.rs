//! Local or S3-compatible object authority, preflight, and verified cache.

#![deny(missing_docs)]

mod ai_search;
mod artifact;
mod backend;
mod cache;
mod client;
mod credentials;
mod error;
mod inspect;
mod local;
mod preflight;
mod r2;
mod r2_codec;
mod r2_model;
mod r2_multipart;
mod r2_preflight;
mod snapshot;
mod store;

pub use ai_search::{AiSearchObjectDownload, AiSearchObjectRef, AiSearchObjectStore};
pub use artifact::{ARTIFACT_KEY_VERSION, ArtifactRef};
pub use backend::{
    AggregatedObjectBytes, BackendError, CustomerKey, GetOptions, HeadOptions, ListPage,
    ListedObject, ObjectBackend, ObjectBody, ObjectBodyReader, ObjectGet, ObjectHttpMetadata,
    ObjectKey, ObjectMetadata, ObjectRange, ObjectSource, ObjectStorageClass, PutMode, PutOptions,
    UploadedPart,
};
pub use cache::{ArtifactCache, PinnedArtifact, PinnedArtifactReader};
pub use credentials::{
    CredentialEnv, ProcessEnv, S3Credentials, StaticEnv, resolve_s3_credentials,
    resolve_s3_credentials_with,
};
pub use inspect::{CacheSample, probe_object_storage, sample_cache_integrity};
pub use preflight::{PreflightOutcome, preflight_object_storage, verify_object_authority};
pub use r2::R2ObjectStore;
pub use r2::{hash_bytes, hash_file, md5_file};
pub use r2_model::{
    R2_MAX_CUSTOM_METADATA_JSON_BYTES, R2_MAX_DELETE_KEYS, R2_MAX_KEY_BYTES, R2_MAX_LIST_LIMIT,
    R2_MAX_MULTIPART_OBJECT_BYTES, R2_MAX_MULTIPART_PART_BYTES, R2_MAX_MULTIPART_PARTS,
    R2_MIN_MULTIPART_PART_BYTES, R2BucketIdentity, R2BucketLocator, R2ChecksumAlgorithm,
    R2Checksums, R2ComputedChecksums, R2Condition, R2Download, R2EtagMatch, R2GetResult,
    R2HttpMetadata, R2MultipartCreateOptions, R2ObjectMetadata, R2PartSource, R2PutOptions,
    R2Range, R2SsecKey, R2StorageClass, R2UploadSource, R2UploadedPart, UserObjectKey,
};
pub use r2_preflight::{R2PreflightOutcome, preflight_r2};
pub use snapshot::{CommittedSnapshot, IncompleteSnapshotCleanup, SnapshotObjectStore};
pub use store::{ArtifactCandidate, ArtifactGcFence, ArtifactStore, ArtifactVersionReservation};

#[cfg(any(test, feature = "test-support"))]
pub use credentials::MapEnv;

#[cfg(any(test, feature = "test-support"))]
mod mock_s3;
#[cfg(any(test, feature = "test-support"))]
pub use mock_s3::{Fault, MockS3, Recorded};

#[cfg(test)]
mod local_tests;
#[cfg(test)]
mod tests;
