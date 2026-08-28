//! Source identities frozen into loaded-isolate deployment descriptors.

use super::*;

const R2_FACADE_SOURCE: &[u8] = include_bytes!("../../../../runtime/system-workers/r2-facade.js");
const D1_FACADE_SOURCE: &[u8] = include_bytes!("../../../../runtime/system-workers/d1-facade.js");
const DO_FACADE_SOURCE: &[u8] = include_bytes!("../../../../runtime/system-workers/do-facade.js");
const DO_ID_CODEC_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/do-id-codec.js");
const DO_ALARM_SHIM_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/do-alarm-shim.js");
const QUEUE_FACADE_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/queue-facade.js");
const LOADED_ISOLATE_WRAPPER_GENERATOR_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/loaded-isolate-wrapper-generator.js");
const WORKFLOW_WRAPPER_GENERATOR_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/loaded-isolate-wrapper-generator-v2.js");
const WORKFLOW_FACADE_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/workflow-facade.js");
const WORKFLOW_V2_WRAPPER_GENERATOR_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/loaded-isolate-wrapper-generator-v3.js");
const WORKFLOW_V2_FACADE_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/workflow-facade-v2.js");
const WORKFLOW_JSON_SOURCE: &[u8] =
    include_bytes!("../../../../runtime/system-workers/workflow-json.js");

/// Exact loaded-isolate source identity frozen into a facade deployment descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoadedIsolateInjectionV1 {
    /// Injection plan schema.
    pub schema_version: u32,
    /// Local R2 facade capability version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r2_facade_capability_version: Option<u32>,
    /// SHA-256 of the exact injected facade module source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r2_facade_sha256: Option<String>,
    /// Local D1 facade capability version when D1 is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d1_facade_capability_version: Option<u32>,
    /// SHA-256 of the exact injected D1 facade source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d1_facade_sha256: Option<String>,
    /// Local Durable Object facade capability version when a namespace is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_facade_capability_version: Option<u32>,
    /// SHA-256 of the exact injected Durable Object facade source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_facade_sha256: Option<String>,
    /// SHA-256 of the exact synchronous Durable Object ID codec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_id_codec_sha256: Option<String>,
    /// SHA-256 of the exact object-local alarm shim source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_alarm_shim_sha256: Option<String>,
    /// Local Queue producer facade capability version when a Queue binding is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_facade_capability_version: Option<u32>,
    /// SHA-256 of the exact injected Queue producer facade source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_facade_sha256: Option<String>,
    /// Local Workflow facade capability when a caller binding is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_facade_capability_version: Option<u32>,
    /// Frozen tenant-local Workflow caller facade source digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_facade_sha256: Option<String>,
    /// Durable Workflow facade capability, independently present in mixed deployments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_v2_facade_capability_version: Option<u32>,
    /// Frozen capability-two facade source digest; never replaces the legacy source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_v2_facade_sha256: Option<String>,
    /// Frozen canonical JSON codec source digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_json_sha256: Option<String>,
    /// SHA-256 of the exact deterministic wrapper generator source.
    pub loaded_isolate_wrapper_generator_sha256: String,
}

impl LoadedIsolateInjectionV1 {
    pub(super) fn for_bindings(
        bindings: &[BindingDescriptorV1],
        queue_bindings: &[QueueProducerBindingDescriptorV1],
        workflow_bindings: &[open_compute_storage::WorkflowBindingDescriptor],
    ) -> Option<Self> {
        let r2 = bindings
            .iter()
            .any(|binding| binding.kind == BindingKind::R2Bucket);
        let d1 = bindings
            .iter()
            .any(|binding| binding.kind == BindingKind::D1Database);
        let durable_objects = bindings
            .iter()
            .any(|binding| binding.kind == BindingKind::DoNamespace);
        let queue = !queue_bindings.is_empty();
        let workflow = !workflow_bindings.is_empty();
        let workflow_v1 = workflow_bindings
            .iter()
            .any(|binding| binding.capability_version == 1);
        let workflow_v2 = workflow_bindings
            .iter()
            .any(|binding| binding.capability_version == 2);
        (r2 || d1 || durable_objects || queue || workflow).then(|| Self {
            schema_version: 1,
            r2_facade_capability_version: r2.then_some(1),
            r2_facade_sha256: r2.then(|| hex::encode(Sha256::digest(R2_FACADE_SOURCE))),
            d1_facade_capability_version: d1.then_some(1),
            d1_facade_sha256: d1.then(|| hex::encode(Sha256::digest(D1_FACADE_SOURCE))),
            do_facade_capability_version: durable_objects.then_some(1),
            do_facade_sha256: durable_objects
                .then(|| hex::encode(Sha256::digest(DO_FACADE_SOURCE))),
            do_id_codec_sha256: durable_objects
                .then(|| hex::encode(Sha256::digest(DO_ID_CODEC_SOURCE))),
            do_alarm_shim_sha256: durable_objects
                .then(|| hex::encode(Sha256::digest(DO_ALARM_SHIM_SOURCE))),
            queue_facade_capability_version: queue.then_some(1),
            queue_facade_sha256: queue.then(|| hex::encode(Sha256::digest(QUEUE_FACADE_SOURCE))),
            workflow_facade_capability_version: workflow_v1.then_some(1),
            workflow_facade_sha256: workflow_v1
                .then(|| hex::encode(Sha256::digest(WORKFLOW_FACADE_SOURCE))),
            workflow_v2_facade_capability_version: workflow_v2.then_some(2),
            workflow_v2_facade_sha256: workflow_v2
                .then(|| hex::encode(Sha256::digest(WORKFLOW_V2_FACADE_SOURCE))),
            workflow_json_sha256: workflow
                .then(|| hex::encode(Sha256::digest(WORKFLOW_JSON_SOURCE))),
            loaded_isolate_wrapper_generator_sha256: hex::encode(Sha256::digest(if workflow_v2 {
                WORKFLOW_V2_WRAPPER_GENERATOR_SOURCE
            } else if workflow {
                WORKFLOW_WRAPPER_GENERATOR_SOURCE
            } else {
                LOADED_ISOLATE_WRAPPER_GENERATOR_SOURCE
            })),
        })
    }
}

#[cfg(test)]
#[path = "injection_tests.rs"]
mod tests;
