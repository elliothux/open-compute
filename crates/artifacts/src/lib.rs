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
mod store;

pub use artifact::{ARTIFACT_KEY_VERSION, ArtifactRef};
pub use cache::{ArtifactCache, PinnedArtifact};
pub use client::S3ArtifactClient;
pub use credentials::{
    CredentialEnv, ProcessEnv, S3Credentials, StaticEnv, resolve_s3_credentials,
    resolve_s3_credentials_with,
};
pub use error::{S3Failure, S3Stage};
pub use inspect::{CacheSample, sample_cache_integrity};
pub use preflight::{PreflightOutcome, preflight_s3};
pub use store::{ArtifactCandidate, ArtifactStore};

#[cfg(any(test, feature = "test-support"))]
pub use credentials::MapEnv;

#[cfg(any(test, feature = "test-support"))]
mod mock_s3;
#[cfg(any(test, feature = "test-support"))]
pub use mock_s3::{Fault, MockS3, Recorded};

#[cfg(test)]
mod tests;
