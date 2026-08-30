//! Versioned P1 capability registry types.

use crate::PlatformReleaseIdentityV1;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Product support verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// Production behavior matches the declared Cloudflare contract.
    Supported,
    /// Production behavior is supported with stable, documented deviations.
    SupportedWithDeviation,
    /// The product or optional API is intentionally absent.
    Unsupported,
    /// Support is intended but required implementation or evidence is missing.
    Blocked,
}

/// One product entry in [`PlatformCapabilitiesV1`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductCapabilityV1 {
    /// Product support state.
    pub status: CapabilityStatus,
    /// Static facade contract version when supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_version: Option<u32>,
    /// Supported method names in canonical order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    /// Stable documented deviation identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deviations: Vec<String>,
    /// Optional basic WebSocket support state for Durable Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basic_websocket: Option<CapabilityStatus>,
    /// Optional hibernatable WebSocket support state for Durable Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hibernatable_websocket: Option<CapabilityStatus>,
}

impl ProductCapabilityV1 {
    /// Validate one product entry independently of release identity and configured limits.
    pub fn validate(&self) -> bool {
        let methods_are_unique =
            self.methods.iter().enumerate().all(|(index, method)| {
                !method.is_empty() && !self.methods[..index].contains(method)
            });
        let deviations_are_unique = self
            .deviations
            .iter()
            .enumerate()
            .all(|(index, deviation)| {
                !deviation.is_empty() && !self.deviations[..index].contains(deviation)
            });
        methods_are_unique
            && deviations_are_unique
            && match self.status {
                CapabilityStatus::Supported => {
                    self.capability_version.is_some()
                        && !self.methods.is_empty()
                        && self.deviations.is_empty()
                }
                CapabilityStatus::SupportedWithDeviation => {
                    self.capability_version.is_some()
                        && !self.methods.is_empty()
                        && !self.deviations.is_empty()
                }
                CapabilityStatus::Unsupported | CapabilityStatus::Blocked => {
                    self.capability_version.is_none()
                        && self.methods.is_empty()
                        && self.deviations.is_empty()
                        && self.basic_websocket.is_none()
                        && self.hibernatable_websocket.is_none()
                }
            }
    }
}

/// Runtime compatibility policy exposed by the production binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCapabilityV1 {
    /// Minimum supported Worker compatibility date.
    pub compatibility_date_min: String,
    /// Maximum supported Worker compatibility date.
    pub compatibility_date_max: String,
    /// Allowlisted compatibility flags.
    pub allowed_flags: Vec<String>,
    /// Explicitly denied compatibility flags.
    pub denied_flags: Vec<String>,
    /// SHA-256 of the formal runtime lock bytes.
    pub workerd_lock_sha256: String,
}

/// Queryable P1 product and release contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlatformCapabilitiesV1 {
    /// JSON format version.
    pub schema_version: u32,
    /// Exact release and persisted-format identity.
    pub release: PlatformReleaseIdentityV1,
    /// Pinned runtime compatibility policy.
    pub runtime: RuntimeCapabilityV1,
    /// Product entries keyed by stable product name.
    pub products: BTreeMap<String, ProductCapabilityV1>,
    /// Frozen configured limit names and values; never contains secret values.
    pub limits: BTreeMap<String, u64>,
}

impl PlatformCapabilitiesV1 {
    /// Validate stable product coverage and the linked release identity.
    pub fn validate(&self) -> bool {
        const REQUIRED: [&str; 25] = [
            "workers",
            "deployments",
            "static_assets",
            "service_bindings",
            "kv",
            "r2",
            "d1",
            "durable_objects",
            "alarms",
            "queues",
            "cron",
            "workflows",
            "workers_cache",
            "cache_api",
            "images",
            "version_metadata",
            "websocket_hibernation",
            "analytics_engine",
            "ai",
            "browser_rendering",
            "vectorize",
            "hyperdrive",
            "mtls",
            "rate_limiting",
            "workers_for_platforms",
        ];
        self.schema_version == 1
            && self.release.validate()
            && self.runtime.compatibility_date_min <= self.runtime.compatibility_date_max
            && REQUIRED
                .iter()
                .all(|name| self.products.contains_key(*name))
            && self.products.values().all(ProductCapabilityV1::validate)
    }
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;
