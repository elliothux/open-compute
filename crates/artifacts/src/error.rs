//! Backend-neutral, secret-safe object failure mapping.

use crate::BackendError;
use open_compute_core::{ErrorCode, PlatformError};

pub(crate) const fn integrity_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::ArtifactIntegrityError,
        "artifact failed integrity verification",
    )
}

impl From<BackendError> for PlatformError {
    fn from(error: BackendError) -> Self {
        let (code, message) = match error {
            BackendError::NotFound => (
                ErrorCode::ObjectStorageUnavailable,
                "object storage object was not found",
            ),
            BackendError::PreconditionFailed => (
                ErrorCode::ObjectStorageUnavailable,
                "object storage precondition failed",
            ),
            BackendError::InvalidRange => (
                ErrorCode::ObjectStorageIntegrityError,
                "object storage range is invalid",
            ),
            BackendError::Corrupt => (
                ErrorCode::ObjectStorageIntegrityError,
                "object storage integrity verification failed",
            ),
            BackendError::Unavailable => (
                ErrorCode::ObjectStorageUnavailable,
                "object storage is unavailable",
            ),
            BackendError::Capacity => (
                ErrorCode::ObjectStorageCapacity,
                "object storage capacity is exhausted",
            ),
            BackendError::InvalidKey => (ErrorCode::ConfigInvalid, "object storage key is invalid"),
            BackendError::CustomerKeyInvalid => (
                ErrorCode::ObjectStorageIntegrityError,
                "object storage customer key is invalid",
            ),
            BackendError::MultipartInvalid => (
                ErrorCode::ObjectStorageIntegrityError,
                "object storage multipart state is invalid",
            ),
            BackendError::AuthorityMismatch => (
                ErrorCode::ObjectStorageAuthorityMismatch,
                "object storage authority does not match this platform",
            ),
        };
        PlatformError::new(code, message)
    }
}

pub(crate) fn from_backend(error: BackendError) -> PlatformError {
    error.into()
}

pub(crate) fn is_not_found(error: &PlatformError) -> bool {
    error.message() == "object storage object was not found"
}
