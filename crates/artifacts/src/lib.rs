//! S3-compatible artifact authority, preflight, and verified local cache.
//!
//! This crate talks to object storage over the real `SigV4` S3 protocol. It
//! never depends on `open-compute-storage` or the control database.

#![deny(missing_docs)]

mod artifact;
mod cache;
mod client;
mod credentials;
mod error;
mod inspect;
mod preflight;
mod r2;
mod r2_model;
mod r2_preflight;
mod snapshot;
mod store;

pub use artifact::{ARTIFACT_KEY_VERSION, ArtifactRef};
pub use cache::{ArtifactCache, PinnedArtifact, PinnedArtifactReader};
pub use client::S3ArtifactClient;
pub use credentials::{
    CredentialEnv, ProcessEnv, S3Credentials, StaticEnv, resolve_s3_credentials,
    resolve_s3_credentials_with,
};
pub use error::{S3Failure, S3Stage};
pub use inspect::{CacheSample, sample_cache_integrity};
pub use preflight::{PreflightOutcome, preflight_s3};
pub use r2::R2ObjectStore;
pub use r2::md5_file;
pub use r2_model::{
    R2_MAX_CUSTOM_METADATA_JSON_BYTES, R2_MAX_DELETE_KEYS, R2_MAX_LIST_LIMIT,
    R2_PROVIDER_KEY_MAX_BYTES, R2BucketIdentity, R2BucketLocator, R2Condition, R2Download,
    R2GetResult, R2HttpMetadata, R2ListPage, R2ListedObject, R2ObjectMetadata, R2PutOptions,
    R2Range, R2UploadSource, UserObjectKey,
};
pub use r2_preflight::{R2PreflightOutcome, preflight_r2};
pub use snapshot::{CommittedSnapshot, IncompleteSnapshotCleanup, SnapshotObjectStore};
pub use store::{ArtifactCandidate, ArtifactDeploymentReservation, ArtifactGcFence, ArtifactStore};

#[cfg(any(test, feature = "test-support"))]
pub use credentials::MapEnv;

#[cfg(any(test, feature = "test-support"))]
mod mock_s3;
#[cfg(any(test, feature = "test-support"))]
pub use mock_s3::{Fault, MockS3, Recorded};

#[cfg(test)]
mod tests;
