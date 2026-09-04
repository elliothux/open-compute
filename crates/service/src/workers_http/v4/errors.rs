//! Stable platform errors emitted by the Worker v4 domain adapter.

use open_compute_core::{ErrorCode, PlatformError};

pub(super) fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::BundleInvalid, message)
}

pub(super) fn unsupported(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::BindingCapabilityUnsupported, message)
}

pub(super) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "persisted Version authority is inconsistent",
    )
}

#[cfg(test)]
#[path = "errors_tests.rs"]
mod tests;
