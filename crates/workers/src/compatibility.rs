//! Certified Worker compatibility metadata for the formally pinned runtime.

use std::collections::BTreeSet;

/// The only tenant-visible compatibility date accepted by the current runtime pin.
pub const WORKER_COMPATIBILITY_DATE: &str = "2026-08-30";

/// Tenant compatibility flags proven equivalent under the current runtime pin.
pub const ALLOWED_WORKER_COMPATIBILITY_FLAGS: &[&str] = &["nodejs_compat"];

/// Return whether one immutable Version selects the certified date and a duplicate-free flag set.
#[must_use]
pub fn supports_worker_compatibility(date: &str, flags: &[String]) -> bool {
    if date != WORKER_COMPATIBILITY_DATE || flags.len() > ALLOWED_WORKER_COMPATIBILITY_FLAGS.len() {
        return false;
    }
    let mut seen = BTreeSet::new();
    flags.iter().all(|flag| {
        ALLOWED_WORKER_COMPATIBILITY_FLAGS.contains(&flag.as_str()) && seen.insert(flag.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certified_metadata_accepts_only_the_advertised_set() {
        assert!(supports_worker_compatibility(
            WORKER_COMPATIBILITY_DATE,
            &[]
        ));
        assert!(supports_worker_compatibility(
            WORKER_COMPATIBILITY_DATE,
            &["nodejs_compat".to_owned()]
        ));
        assert!(!supports_worker_compatibility(
            WORKER_COMPATIBILITY_DATE,
            &["nodejs_compat".to_owned(), "nodejs_compat".to_owned()]
        ));
        assert!(!supports_worker_compatibility(
            "2026-08-29",
            &["nodejs_compat".to_owned()]
        ));
    }
}
