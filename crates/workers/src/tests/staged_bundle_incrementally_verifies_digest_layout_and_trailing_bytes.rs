use super::*;

#[test]
fn staged_bundle_incrementally_verifies_digest_layout_and_trailing_bytes() {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module("index.js", b"export default { fetch() {} };"),
            ModuleInput {
                name: "payload.bin".to_owned(),
                module_type: ModuleType::Data,
                bytes: vec![7; 2 * 1024 * 1024],
            },
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bundle.upload");
    fs::write(&path, bundle.bytes()).unwrap();
    let staged = StagedBundle::open(path.clone(), BundleLimits::default()).unwrap();
    assert_eq!(staged.manifest(), bundle.manifest());
    assert_eq!(staged.sha256(), bundle.sha256());
    assert_eq!(staged.size(), bundle.bytes().len() as u64);

    let mut trailing = bundle.bytes().to_vec();
    trailing.push(0);
    fs::write(&path, trailing).unwrap();
    assert_eq!(
        StagedBundle::open(path, BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
}
