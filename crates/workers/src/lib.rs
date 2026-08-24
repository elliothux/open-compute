//! P0.2 Workers control/data-plane contracts.

#![deny(missing_docs)]

pub mod bundle;
pub mod descriptor;
pub mod pins;
pub mod pipeline;
pub mod resource_lifecycle;
pub mod resource_pins;
pub mod runtime_source;

pub use bundle::{
    BundleLimits, CanonicalBundle, ModuleInput, ModuleManifest, ModuleType, StagedBundle,
    WorkerBundleManifest,
};
pub use descriptor::{
    BindingDescriptorV1, GLOBAL_OUTBOUND_POLICY_VERSION, SecretDescriptor, WorkerCodeDescriptorV1,
    canonicalize_vars, ciphertext_sha256, loader_key, parse_loader_key, validate_compatibility,
    validate_env_name,
};
pub use pins::{DeploymentPin, DeploymentPins};
pub use pipeline::{
    CreateDeploymentOutcome, CreateDeploymentRequest, CreateDeploymentResult,
    DeploymentBindingInput, DeploymentBundle, DeploymentController, RuntimeValidator,
    ValidationCandidate,
};
pub use resource_lifecycle::{
    CreateResourceOutcome, CreateResourceRequest, CreateResourceResult, ReconcileOutcome,
    ResourceController, ResourceDriver, ResourceHealth,
};
pub use resource_pins::{ResourcePin, ResourcePins};
pub use runtime_source::{
    RuntimeBinding, RuntimeModule, RuntimePayload, RuntimeScope, RuntimeSnapshot, RuntimeSource,
};

#[cfg(test)]
mod tests;
