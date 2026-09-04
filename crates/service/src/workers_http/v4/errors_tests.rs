use super::{invalid, invariant, unsupported};
use open_compute_core::ErrorCode;

#[test]
fn stable_worker_domain_errors_keep_their_owned_codes() {
    assert_eq!(invalid("invalid").code(), ErrorCode::BundleInvalid);
    assert_eq!(
        unsupported("unsupported").code(),
        ErrorCode::BindingCapabilityUnsupported
    );
    assert_eq!(invariant().code(), ErrorCode::VersionInvariantViolation);
}
