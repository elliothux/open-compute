//! Versioned P1 capability registry types.

use crate::PlatformReleaseIdentityV1;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Product support verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// Production behavior and its Gate are implemented.
    Supported,
    /// The product or optional API is intentionally absent.
    Unsupported,
    /// A pinned-runtime hard Gate has not yet produced a stable Go verdict.
    Conditional,
}

/// One product entry in [`PlatformCapabilitiesV1`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
        const REQUIRED: [&str; 10] = [
            "workers",
            "kv",
            "r2",
            "d1",
            "durable_objects",
            "alarms",
            "queues",
            "cron",
            "workflows",
            "websocket_hibernation",
        ];
        self.schema_version == 1
            && self.release.validate()
            && self.runtime.compatibility_date_min <= self.runtime.compatibility_date_max
            && REQUIRED
                .iter()
                .all(|name| self.products.contains_key(*name))
            && self.products.values().all(|product| match product.status {
                CapabilityStatus::Supported => product.capability_version.is_some(),
                CapabilityStatus::Unsupported | CapabilityStatus::Conditional => {
                    product.capability_version.is_none()
                }
            })
    }
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;
