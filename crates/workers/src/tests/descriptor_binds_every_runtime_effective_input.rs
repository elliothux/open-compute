use super::*;

#[test]
fn descriptor_binds_every_runtime_effective_input() {
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let version = VersionId::generate();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![module("index.js", b"export default {}")],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), serde_json::json!({"b": 2, "a": 1}));
    let (vars, _) = canonicalize_vars(vars).unwrap();
    let descriptor = WorkerCodeDescriptorV1::new(
        account,
        worker,
        version,
        0,
        "2026-09-08".into(),
        Vec::new(),
        Some((bundle.sha256(), bundle.manifest())),
        None,
        vars,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        CachePolicyDescriptorV1::default(),
        Vec::new(),
        1,
    )
    .unwrap();
    let encoded = serde_json::to_value(&descriptor).unwrap();
    assert_eq!(encoded["compatibilityDate"], "2026-09-08");
    assert_eq!(encoded["compatibilityFlags"], serde_json::json!([]));
    assert_eq!(
        parse_loader_key(&descriptor.loader_key).unwrap(),
        (account, worker, version)
    );
    let first = descriptor.sha256().unwrap();
    let mut changed = descriptor.clone();
    changed.loader_schema_version += 1;
    assert_ne!(first, changed.sha256().unwrap());
}
