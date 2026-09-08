//! Read-only operator upgrade availability contract for Dashboard and CLI.

use crate::install_receipt::cmp_stable_semver;
use serde::{Deserialize, Serialize};

/// Result of a version check against the formal release authority or local cache.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpgradeCheckResult {
    /// Schema version for this payload.
    pub schema_version: u32,
    /// Currently running release version.
    pub current_version: String,
    /// Newest stable version known from cache or a fresh check, if any.
    pub available_version: Option<String>,
    /// Whether `available_version` is strictly newer than `current_version`.
    pub update_available: bool,
    /// Whether this install may be upgraded by the local `ocd upgrade` command.
    pub upgrade_allowed: bool,
    /// Human-safe reason when upgrade is blocked, for example package-manager ownership.
    pub blocked_reason: Option<String>,
}

/// Build a check result from the current binary and an optional cached version.
#[must_use]
pub fn check_result(
    current_version: &str,
    available_version: Option<&str>,
    upgrade_allowed: bool,
    blocked_reason: Option<&str>,
) -> UpgradeCheckResult {
    let available_version = available_version.filter(|available| {
        cmp_stable_semver(available, current_version) == Some(std::cmp::Ordering::Greater)
    });
    let update_available = available_version.is_some();
    UpgradeCheckResult {
        schema_version: 1,
        current_version: current_version.to_owned(),
        available_version: available_version.map(str::to_owned),
        update_available,
        upgrade_allowed: upgrade_allowed && update_available,
        blocked_reason: blocked_reason.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_result_only_marks_strictly_newer_stable_versions() {
        let newer = check_result("0.1.0", Some("0.1.1"), true, None);
        assert!(newer.update_available);
        assert!(newer.upgrade_allowed);

        for version in ["0.1.0", "0.0.9", "not-semver"] {
            let result = check_result("0.1.0", Some(version), true, None);
            assert!(!result.update_available);
            assert!(!result.upgrade_allowed);
            assert!(result.available_version.is_none());
        }

        let blocked = check_result("0.1.0", Some("0.1.1"), false, Some("package-manager-owned"));
        assert!(!blocked.upgrade_allowed);
    }
}
