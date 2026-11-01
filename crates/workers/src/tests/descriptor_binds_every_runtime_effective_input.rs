use super::*;

#[test]
fn descriptor_binds_every_runtime_effective_input() {
    let account = "019c0000000070008000000000000001"
        .parse::<InstanceId>()
        .unwrap();
    let worker = "019c0000-0000-7000-8000-000000000002"
        .parse::<WorkerId>()
        .unwrap();
    let version = "019c0000-0000-7000-8000-000000000003"
        .parse::<VersionId>()
        .unwrap();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![module("index.js", b"export default {}")],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), serde_json::json!({"b": 2, "a": 1}));
    let (vars, _) = canonicalize_vars(vars).unwrap();
    let mut descriptor = WorkerCodeDescriptorV1::new(
        account,
        worker,
        version,
        0,
        "2026-09-08".into(),
        Vec::new(),
        open_compute_storage::worker_repository::EffectiveResourceLimits::standard_defaults(),
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
    // Freeze the source-manifest input so runtime source edits do not change this fixture.
    descriptor.system_worker_sources_sha256 = "ab".repeat(32);
    assert_eq!(
        hex::encode(descriptor.sha256().unwrap()),
        "e4aad2d2a1b0fc64f283dab3fc5cb84fe0ecc740cd0d82dfefad1a3e95bab8ef"
    );
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
