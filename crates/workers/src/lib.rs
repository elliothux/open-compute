//! P0.2 Workers control/data-plane contracts.

#![deny(missing_docs)]

pub mod assets;
pub mod bundle;
pub mod d1;
pub mod descriptor;
pub mod durable_objects;
pub mod kv;
pub mod pins;
pub mod pipeline;
pub mod queue_lifecycle;
pub mod r2;
pub mod resource_lifecycle;
pub mod resource_pins;
pub mod runtime_source;
pub mod workflows;
pub use workflows::{
    WorkflowController, WorkflowCreateInput, WorkflowReconcileCursor, WorkflowStatus,
};

pub use assets::{
    AssetEntryV1, AssetHeaderOperation, AssetHeaderRule, AssetManifestV1, AssetRedirectRule,
    AssetRequest, AssetResponsePlan, AssetRoutingConfigV1, DeploymentAssets, HtmlHandling,
    MAX_ASSET_FILE_BYTES, MAX_ASSET_FILES, MAX_ASSET_MANIFEST_BYTES, MAX_ASSET_ROUTING_RULES,
    MAX_ASSET_TOTAL_BYTES, NotFoundHandling, RunWorkerFirst, plan_asset_response,
    validate_asset_path,
};

pub use bundle::{
    BundleLimits, CanonicalBundle, ModuleInput, ModuleManifest, ModuleType, StagedBundle,
    WorkerBundleManifest,
};
pub use d1::D1ResourceDriver;
pub use descriptor::{
    AssetDescriptorV1, BindingDescriptorV1, COMPATIBILITY_DATE_MAX, COMPATIBILITY_DATE_MIN,
    COMPATIBILITY_FLAGS_ALLOWED, GLOBAL_OUTBOUND_POLICY_VERSION, QueueProducerBindingDescriptorV1,
    SYSTEM_MODULE_PREFIX, SecretDescriptor, WorkerCodeDescriptorV1, canonicalize_vars,
    ciphertext_sha256, loader_key, parse_loader_key, validate_compatibility, validate_env_name,
};
pub use durable_objects::DurableObjectResourceDriver;
pub use kv::KvResourceDriver;
pub use pins::{DeploymentPin, DeploymentPins};
pub use pipeline::{
    CreateDeploymentOutcome, CreateDeploymentRequest, CreateDeploymentResult,
    DeploymentBindingInput, DeploymentBundle, DeploymentContent, DeploymentController,
    ProductPromotionCoordinator, ProductPromotionRequest, QueueConsumerInput, RuntimeValidator,
    ValidationCandidate,
};
pub use queue_lifecycle::{
    CreateQueueOutcome, CreateQueueRequest, CreateQueueResult, DeleteQueueResult, QueueController,
};
pub use r2::R2ResourceDriver;
pub use resource_lifecycle::{
    CreateResourceOutcome, CreateResourceRequest, CreateResourceResult, ReconcileOutcome,
    ResourceController, ResourceDriver, ResourceHealth,
};
pub use resource_pins::{ResourcePin, ResourcePins};
pub use runtime_source::{
    DurableObjectFacadeIdentity, RuntimeAssetBinding, RuntimeAssets, RuntimeBinding, RuntimeModule,
    RuntimePayload, RuntimeQueueBinding, RuntimeScope, RuntimeSnapshot, RuntimeSource,
};

#[cfg(test)]
mod tests;
