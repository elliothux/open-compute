use super::*;
use crate::pipeline::{DeploymentBindingInput, validate_binding_set};
use serde_json::json;
use sha2::Digest;

#[test]
fn every_deployment_binds_the_complete_system_source_identity() {
    let bundle = crate::CanonicalBundle::build(
        "index.js",
        vec![crate::ModuleInput {
            name: "index.js".into(),
            module_type: crate::ModuleType::EsModule,
            bytes: b"export default {}".to_vec(),
        }],
        crate::BundleLimits::default(),
    )
    .unwrap();
    let descriptor = WorkerCodeDescriptorV1::new(
        AccountId::generate(),
        WorkerId::generate(),
        DeploymentId::generate(),
        Some((bundle.sha256(), bundle.manifest())),
        None,
        "2026-08-22".into(),
        Vec::new(),
        BTreeMap::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        json!({"profile":"default"}),
        1,
    )
    .unwrap();
    assert_eq!(
        descriptor.system_worker_sources_sha256,
        hex::encode(Sha256::digest(SYSTEM_WORKER_MANIFEST))
    );
    let digest = descriptor.sha256().unwrap();
    let mut changed = descriptor.clone();
    changed.system_worker_sources_sha256 = "00".repeat(32);
    assert_ne!(digest, changed.sha256().unwrap());
    let mut wire = serde_json::to_value(&descriptor).unwrap();
    assert_eq!(
        serde_json::from_value::<WorkerCodeDescriptorV1>(wire.clone()).unwrap(),
        descriptor
    );
    wire.as_object_mut()
        .unwrap()
        .remove("systemWorkerSourcesSha256");
    assert!(serde_json::from_value::<WorkerCodeDescriptorV1>(wire).is_err());
}

#[test]
fn generated_source_manifest_matches_every_system_worker() {
    fn sources(
        directory: &std::path::Path,
        root: &std::path::Path,
        found: &mut BTreeMap<String, String>,
    ) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, root, found);
            } else if path.extension().is_some_and(|value| value == "js") {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                found.insert(
                    name,
                    hex::encode(Sha256::digest(std::fs::read(path).unwrap())),
                );
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/runtime/dist");
    let mut found = BTreeMap::new();
    sources(&root, &root, &mut found);
    assert!(!found.is_empty());
    let manifest: serde_json::Value = serde_json::from_slice(SYSTEM_WORKER_MANIFEST).unwrap();
    assert_eq!(manifest["schemaVersion"], 1);
    assert_eq!(manifest["sources"], json!(found));
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name, expected) in manifest["inputs"].as_object().unwrap() {
        let bytes = std::fs::read(repo.join(name)).unwrap();
        assert_eq!(
            expected,
            &json!(hex::encode(Sha256::digest(bytes))),
            "{name}"
        );
    }
}

#[test]
fn all_system_module_paths_are_reserved_without_binding_exceptions() {
    for name in [
        "__open_compute__/entry.js",
        "__open_compute__/d1/facade.js",
        "__open_compute__/workflows/json.js",
        "__open_compute__/other.js",
    ] {
        let bundle = crate::CanonicalBundle::build(
            name,
            vec![crate::ModuleInput {
                name: name.into(),
                module_type: crate::ModuleType::EsModule,
                bytes: b"export default {}".to_vec(),
            }],
            crate::BundleLimits::default(),
        )
        .unwrap();
        assert_eq!(
            crate::pipeline::validate_injection_module_collisions(bundle.manifest())
                .unwrap_err()
                .code(),
            ErrorCode::BundleInvalid
        );
    }
    let bundle = crate::CanonicalBundle::build(
        "app/index.js",
        vec![crate::ModuleInput {
            name: "app/index.js".into(),
            module_type: crate::ModuleType::EsModule,
            bytes: b"export default {}".to_vec(),
        }],
        crate::BundleLimits::default(),
    )
    .unwrap();
    crate::pipeline::validate_injection_module_collisions(bundle.manifest()).unwrap();
}

#[test]
fn caller_declaration_uses_one_current_capability_and_rejects_selectors() {
    let body = json!({"type":"workflow","id":ResourceId::generate(),"permissions":{"read":true,"write":true},"config":{}});
    let declaration: DeploymentBindingInput = serde_json::from_value(body.clone()).unwrap();
    let valid = |value: &DeploymentBindingInput| {
        validate_binding_set(
            &BTreeMap::from([("FLOW".into(), value.clone())]),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
    };
    valid(&declaration).unwrap();
    assert!(serde_json::to_value(&declaration).unwrap()["capabilityVersion"].is_null());
    for capability in [
        json!(null),
        json!(1),
        json!(2),
        json!("2"),
        json!(2.5),
        json!(-1),
    ] {
        let mut invalid = body.clone();
        invalid["capabilityVersion"] = capability;
        assert!(serde_json::from_value::<DeploymentBindingInput>(invalid).is_err());
    }
}
