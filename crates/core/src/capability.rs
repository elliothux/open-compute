//! Versioned P1 capability registry types.

use crate::PlatformReleaseIdentityV1;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

/// How a product relates to the Cloudflare target inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductKind {
    /// Cloudflare target surface inventoried from the pinned stable AST.
    Target,
    /// Platform-owned product without an upstream type inventory.
    Platform,
    /// Explicitly out-of-scope Cloudflare product.
    NonTarget,
}

/// One inventoried upstream symbol member or overload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityMemberV1 {
    /// Stable member/overload identity.
    pub id: String,
    /// Public product that owns this member.
    pub product: String,
    /// Qualified TypeScript symbol name.
    pub symbol: String,
    /// Member name, including synthetic `()`, `new`, `[]`, and `constructor`.
    pub member: String,
    /// Member kind: method, property, constructor, call, construct, index, get, set, function, or var.
    pub kind: String,
    /// Zero-based overload index among identical symbol/member/kind records.
    pub overload: u32,
    /// Whether the member is readonly.
    pub readonly: bool,
    /// Whether the member is optional.
    pub optional: bool,
    /// Whether the member is static.
    #[serde(rename = "static")]
    pub is_static: bool,
    /// Whitespace-normalized source signature.
    pub signature: String,
    /// SHA-256 of the canonical member AST JSON.
    pub signature_sha256: String,
    /// Member support state.
    pub status: CapabilityStatus,
    /// Compile-fixture case IDs required for supported members.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compile_cases: Vec<String>,
    /// Real-runtime case IDs required for supported members.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_cases: Vec<String>,
    /// Stable documented deviation identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deviations: Vec<String>,
}

impl CapabilityMemberV1 {
    /// Validate one member record independently of product aggregation.
    pub fn validate(&self) -> bool {
        !self.id.is_empty()
            && !self.product.is_empty()
            && !self.symbol.is_empty()
            && !self.member.is_empty()
            && !self.kind.is_empty()
            && !self.signature.is_empty()
            && is_sha256(&self.signature_sha256)
            && unique_nonempty(&self.compile_cases)
            && unique_nonempty(&self.runtime_cases)
            && unique_nonempty(&self.deviations)
            && match self.status {
                CapabilityStatus::Supported => {
                    !self.compile_cases.is_empty()
                        && !self.runtime_cases.is_empty()
                        && self.deviations.is_empty()
                }
                CapabilityStatus::SupportedWithDeviation => {
                    !self.compile_cases.is_empty()
                        && !self.runtime_cases.is_empty()
                        && !self.deviations.is_empty()
                }
                CapabilityStatus::Blocked => {
                    self.compile_cases.is_empty() && self.runtime_cases.is_empty()
                }
                CapabilityStatus::Unsupported => false,
            }
    }
}

/// One product entry in [`PlatformCapabilitiesV1`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductCapabilityV1 {
    /// Product support state.
    pub status: CapabilityStatus,
    /// Whether this product is target inventory, platform-owned, or non-target.
    pub kind: ProductKind,
    /// Static facade contract version when supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_version: Option<u32>,
    /// Inventoried upstream members in canonical order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<CapabilityMemberV1>,
    /// Stable documented deviation identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deviations: Vec<String>,
}

impl ProductCapabilityV1 {
    /// Validate one product entry independently of release identity and configured limits.
    pub fn validate(&self) -> bool {
        let deviations_are_unique = unique_nonempty(&self.deviations);
        let member_ids_are_unique = unique_nonempty(
            &self
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect::<Vec<_>>(),
        );
        let members_are_valid = self.members.iter().all(CapabilityMemberV1::validate)
            && self
                .members
                .iter()
                .all(|member| member.status != CapabilityStatus::Unsupported);
        deviations_are_unique
            && member_ids_are_unique
            && members_are_valid
            && match (self.kind, self.status) {
                (ProductKind::NonTarget, CapabilityStatus::Unsupported) => {
                    self.capability_version.is_none()
                        && self.members.is_empty()
                        && self.deviations.is_empty()
                }
                (ProductKind::Platform, CapabilityStatus::Supported) => {
                    self.capability_version.is_some()
                        && self.members.is_empty()
                        && self.deviations.is_empty()
                }
                (ProductKind::Platform, CapabilityStatus::SupportedWithDeviation) => {
                    self.capability_version.is_some()
                        && self.members.is_empty()
                        && !self.deviations.is_empty()
                }
                (ProductKind::Target, CapabilityStatus::Blocked) => {
                    self.capability_version.is_none() && !self.members.is_empty()
                }
                (ProductKind::Target, CapabilityStatus::Supported) => {
                    self.capability_version.is_some()
                        && self.deviations.is_empty()
                        && !self.members.is_empty()
                        && self
                            .members
                            .iter()
                            .all(|member| member.status == CapabilityStatus::Supported)
                }
                (ProductKind::Target, CapabilityStatus::SupportedWithDeviation) => {
                    self.capability_version.is_some()
                        && !self.deviations.is_empty()
                        && !self.members.is_empty()
                        && self.members.iter().all(|member| {
                            matches!(
                                member.status,
                                CapabilityStatus::Supported
                                    | CapabilityStatus::SupportedWithDeviation
                            )
                        })
                }
                _ => false,
            }
    }
}

/// Pinned workers-types identity that produced the member inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeSourceIdentityV1 {
    /// npm `@cloudflare/workers-types` version.
    pub workers_types_version: String,
    /// Upstream git revision for the pinned types package.
    pub git_head: String,
    /// SHA-256 of the pinned types package tarball or lock digest input.
    pub package_sha256: String,
    /// SHA-256 of the pinned stable `index.d.ts` bytes.
    pub index_sha256: String,
    /// SHA-256 of the canonical TypeScript AST JSON.
    pub ast_sha256: String,
}

impl TypeSourceIdentityV1 {
    /// Validate immutable type-source identity fields.
    pub fn validate(&self) -> bool {
        !self.workers_types_version.is_empty()
            && !self.git_head.is_empty()
            && is_sha256(&self.package_sha256)
            && is_sha256(&self.index_sha256)
            && is_sha256(&self.ast_sha256)
    }
}

/// Checked-in machine inventory deserialized by `ocd`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityInventoryV1 {
    /// JSON format version.
    pub schema_version: u32,
    /// Pinned workers-types identity.
    pub source: TypeSourceIdentityV1,
    /// Product entries keyed by stable product name.
    pub products: BTreeMap<String, ProductCapabilityV1>,
}

impl CapabilityInventoryV1 {
    /// Validate inventory schema, source identity, and product coverage.
    pub fn validate(&self) -> bool {
        self.schema_version == 1
            && self.source.validate()
            && required_products_present(&self.products)
            && self.products.values().all(ProductCapabilityV1::validate)
            && unique_member_ids(&self.products)
    }
}

/// Runtime identity exposed by the production binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCapabilityV1 {
    /// Effective compatibility date from the formal runtime lock.
    pub effective_compatibility_date: String,
    /// SHA-256 of the formal runtime lock bytes.
    pub workerd_lock_sha256: String,
    /// npm `@cloudflare/workers-types` version.
    pub workers_types_version: String,
    /// Upstream git revision for the pinned types package.
    pub workers_types_git_head: String,
    /// SHA-256 of the pinned types package.
    pub workers_types_package_sha256: String,
    /// SHA-256 of the pinned stable `index.d.ts` bytes.
    pub workers_types_index_sha256: String,
    /// SHA-256 of the canonical TypeScript AST JSON.
    pub workers_types_ast_sha256: String,
}

impl RuntimeCapabilityV1 {
    /// Validate immutable runtime and type-source identity.
    pub fn validate(&self) -> bool {
        !self.effective_compatibility_date.is_empty()
            && is_sha256(&self.workerd_lock_sha256)
            && !self.workers_types_version.is_empty()
            && !self.workers_types_git_head.is_empty()
            && is_sha256(&self.workers_types_package_sha256)
            && is_sha256(&self.workers_types_index_sha256)
            && is_sha256(&self.workers_types_ast_sha256)
    }
}

/// Queryable P1 product and release contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlatformCapabilitiesV1 {
    /// JSON format version.
    pub schema_version: u32,
    /// Exact release and persisted-format identity.
    pub release: PlatformReleaseIdentityV1,
    /// Pinned runtime lock and type-source identity.
    pub runtime: RuntimeCapabilityV1,
    /// Product entries keyed by stable product name.
    pub products: BTreeMap<String, ProductCapabilityV1>,
    /// Frozen configured limit names and values; never contains secret values.
    pub limits: BTreeMap<String, u64>,
}

impl PlatformCapabilitiesV1 {
    /// Validate stable product coverage and the linked release identity.
    pub fn validate(&self) -> bool {
        self.schema_version == 1
            && self.release.validate()
            && self.runtime.validate()
            && required_products_present(&self.products)
            && self.products.values().all(ProductCapabilityV1::validate)
            && unique_member_ids(&self.products)
    }
}

fn required_products_present(products: &BTreeMap<String, ProductCapabilityV1>) -> bool {
    const REQUIRED: [&str; 25] = [
        "workers",
        "versions",
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
    REQUIRED.iter().all(|name| products.contains_key(*name))
}

fn unique_member_ids(products: &BTreeMap<String, ProductCapabilityV1>) -> bool {
    let mut ids = BTreeSet::new();
    products
        .values()
        .flat_map(|product| &product.members)
        .all(|member| ids.insert(&member.id))
}

fn unique_nonempty(values: &[String]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !value.is_empty() && !values[..index].contains(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;
