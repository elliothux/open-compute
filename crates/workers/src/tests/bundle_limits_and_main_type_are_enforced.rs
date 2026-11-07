use super::*;

#[test]
fn bundle_limits_and_main_type_are_enforced() {
    let limits = BundleLimits {
        max_total_module_bytes: 3,
        ..BundleLimits::default()
    };
    assert_eq!(
        CanonicalBundle::build("index.js", vec![module("index.js", b"1234")], limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleTooLarge
    );
    let mut main = module("index.js", b"hello");
    main.module_type = ModuleType::Text;
    assert_eq!(
        CanonicalBundle::build("index.js", vec![main], BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
}

#[test]
fn default_bundle_limits_admit_a_full_bounded_module_set_and_reject_overflow() {
    let limits = BundleLimits::default();
    assert_eq!(limits.max_modules, 4096);
    assert_eq!(limits.max_total_module_bytes, 32 * 1024 * 1024);
    assert_eq!(limits.max_manifest_bytes, 1024 * 1024);
    assert_eq!(
        limits.max_artifact_bytes as u64,
        open_compute_core::WorkersConfig::default().max_bundle_bytes
    );
    let mut modules = (0..limits.max_modules)
        .map(|index| ModuleInput {
            name: format!("packages/resource_{index:04}.bin"),
            module_type: ModuleType::Data,
            bytes: vec![0],
        })
        .collect::<Vec<_>>();
    modules[0].name = "index.js".to_owned();
    modules[0].module_type = ModuleType::EsModule;
    // One module may take the whole module budget; Cloudflare limits only the Worker total.
    modules[0].bytes = vec![b' '; 9 * 1024 * 1024];
    modules[3].bytes =
        vec![0; limits.max_total_module_bytes - 9 * 1024 * 1024 - (limits.max_modules - 2)];
    let bundle = CanonicalBundle::build("index.js", modules.clone(), limits).unwrap();
    assert_eq!(bundle.manifest().modules.len(), limits.max_modules);
    assert!(bundle.bytes().len() > limits.max_total_module_bytes);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bundle");
    fs::write(&path, bundle.bytes()).unwrap();
    let staged = StagedBundle::open(path, limits).unwrap();
    assert_eq!(staged.sha256(), bundle.sha256());
    assert_eq!(staged.manifest(), bundle.manifest());
    assert_eq!(
        CanonicalBundle::parse(bundle.bytes().to_vec(), limits).unwrap(),
        bundle
    );

    modules[3].bytes.push(0);
    assert_eq!(
        CanonicalBundle::build("index.js", modules, limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleTooLarge
    );
    let mut modules = (0..=limits.max_modules)
        .map(|index| module(&format!("part{index}.js"), b""))
        .collect::<Vec<_>>();
    modules[0].name = "index.js".to_owned();
    assert_eq!(
        CanonicalBundle::build("index.js", modules, limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleTooLarge
    );
}
