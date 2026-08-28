use super::*;
use crate::pipeline::{DeploymentBindingInput, validate_binding_set};
use open_compute_core::WorkflowId;
use open_compute_storage::WorkflowBindingDescriptor;
use serde_json::json;

fn binding(capability: u32) -> WorkflowBindingDescriptor {
    WorkflowBindingDescriptor {
        kind: BindingKind::Workflow,
        schema_version: 1,
        binding_id: BindingId::generate(),
        name: format!("FLOW_{capability}"),
        definition_id: WorkflowId::generate(),
        definition_lifecycle_generation: 1,
        capability_version: capability,
    }
}

#[test]
fn legacy_and_mixed_workflow_injections_freeze_independent_source_identities() {
    let legacy = binding(1);
    let durable = binding(2);
    let one =
        LoadedIsolateInjectionV1::for_bindings(&[], &[], std::slice::from_ref(&legacy)).unwrap();
    let wire = serde_json::to_value(&one).unwrap();
    assert_eq!(
        wire,
        json!({
            "schemaVersion":1,
            "workflowFacadeCapabilityVersion":1,
            "workflowFacadeSha256":"3d7cb193b3636b70bc635d4d53054f5c81e991e52012bb9eaa6532892909c94c",
            "workflowJsonSha256":"e40180928eb3f4611039a63960c610d8b21e358db0058b53805347cca6c81a66",
            "loadedIsolateWrapperGeneratorSha256":"e74c516c4e9ef2c4ef858d91f232de59810c8e33696e28fd4f9e0963e7c990a5"
        })
    );
    assert_eq!(
        serde_json::from_value::<LoadedIsolateInjectionV1>(wire).unwrap(),
        one
    );
    let two =
        LoadedIsolateInjectionV1::for_bindings(&[], &[], std::slice::from_ref(&durable)).unwrap();
    assert_eq!(two.workflow_facade_sha256, None);
    assert_eq!(two.workflow_v2_facade_capability_version, Some(2));
    assert_eq!(
        two.workflow_v2_facade_sha256,
        Some(hex::encode(Sha256::digest(WORKFLOW_V2_FACADE_SOURCE)))
    );
    assert_ne!(
        two.loaded_isolate_wrapper_generator_sha256,
        one.loaded_isolate_wrapper_generator_sha256
    );
    let mixed =
        LoadedIsolateInjectionV1::for_bindings(&[], &[], &[legacy.clone(), durable]).unwrap();
    assert_eq!(mixed.workflow_facade_sha256, one.workflow_facade_sha256);
    assert_eq!(
        mixed.workflow_v2_facade_sha256,
        two.workflow_v2_facade_sha256
    );
    assert_eq!(
        mixed.loaded_isolate_wrapper_generator_sha256,
        two.loaded_isolate_wrapper_generator_sha256
    );
    let mut changed = legacy.clone();
    changed.capability_version = 2;
    assert_ne!(changed.sha256().unwrap(), legacy.sha256().unwrap());
}

#[test]
fn caller_declaration_preserves_legacy_request_bytes_and_rejects_unsupported_products() {
    let old = json!({"type":"workflow","id":ResourceId::generate(),"permissions":{"read":true,"write":true},"config":{}});
    let mut declaration: DeploymentBindingInput = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(declaration.capability_version, 1);
    assert_eq!(serde_json::to_value(&declaration).unwrap(), old);
    let valid = |value: &DeploymentBindingInput| {
        validate_binding_set(
            &BTreeMap::from([("FLOW".into(), value.clone())]),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
    };
    declaration.capability_version = 2;
    assert_eq!(
        serde_json::to_value(&declaration).unwrap()["capabilityVersion"],
        2
    );
    valid(&declaration).unwrap();
    declaration.kind = BindingKind::KvNamespace;
    assert_eq!(
        valid(&declaration).unwrap_err().code(),
        ErrorCode::BindingCapabilityUnsupported
    );
    declaration.kind = BindingKind::Workflow;
    for capability in [0, 3, u32::MAX] {
        declaration.capability_version = capability;
        assert_eq!(
            valid(&declaration).unwrap_err().code(),
            ErrorCode::BindingCapabilityUnsupported
        );
    }
    for capability in [json!(null), json!("2"), json!(2.5), json!(-1)] {
        let mut body = old.clone();
        body["capabilityVersion"] = capability;
        assert!(serde_json::from_value::<DeploymentBindingInput>(body).is_err());
    }
}

#[test]
fn new_reserved_module_does_not_reject_a_legacy_bundle() {
    use crate::{BundleLimits, CanonicalBundle, ModuleInput, ModuleType};
    let bundle = CanonicalBundle::build(
        "index.js",
        ["index.js", WORKFLOW_V2_FACADE_MODULE_NAME]
            .into_iter()
            .map(|name| ModuleInput {
                name: name.into(),
                module_type: ModuleType::EsModule,
                bytes: b"export default {}".to_vec(),
            })
            .collect(),
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::from([(
        "FLOW".into(),
        DeploymentBindingInput {
            kind: BindingKind::Workflow,
            id: ResourceId::generate(),
            capability_version: 1,
            permissions: Default::default(),
            config: Default::default(),
        },
    )]);
    crate::pipeline::validate_injection_module_collisions(bundle.manifest(), &bindings).unwrap();
    bindings.get_mut("FLOW").unwrap().capability_version = 2;
    assert_eq!(
        crate::pipeline::validate_injection_module_collisions(bundle.manifest(), &bindings)
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
}
