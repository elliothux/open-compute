use super::*;

#[test]
fn staged_bundle_rejects_filesystem_framing_and_module_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    assert_eq!(
        StagedBundle::open(missing, BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
    assert_eq!(
        StagedBundle::open(temp.path().to_path_buf(), BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );

    let path = temp.path().join("bundle");
    fs::write(&path, b"too big").unwrap();
    assert_eq!(
        StagedBundle::open(
            path.clone(),
            BundleLimits {
                max_artifact_bytes: 1,
                ..BundleLimits::default()
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );
    for bytes in [
        Vec::new(),
        b"not-a-bundle".to_vec(),
        b"OCWB\0\x01\0\0\0\0\0\0".to_vec(),
        b"OCWB\0\x01\0\0\0\0\0\x10{}".to_vec(),
    ] {
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            StagedBundle::open(path.clone(), BundleLimits::default())
                .unwrap_err()
                .code(),
            ErrorCode::BundleInvalid
        );
    }

    let bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module("index.js", b"export default {}"),
            ModuleInput {
                name: "text.txt".to_owned(),
                module_type: ModuleType::Text,
                bytes: b"hello".to_vec(),
            },
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let corrupt = rewrite_bundle(bundle.bytes(), |_manifest, blob| {
        *blob.last_mut().unwrap() ^= 1;
    });
    fs::write(&path, corrupt).unwrap();
    assert_eq!(
        StagedBundle::open(path.clone(), BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    let invalid_text = rewrite_bundle(bundle.bytes(), |manifest, blob| {
        *blob.last_mut().unwrap() = 0xff;
        let text = manifest
            .modules
            .iter_mut()
            .find(|entry| entry.name == "text.txt")
            .unwrap();
        text.sha256 = hex::encode(sha2::Sha256::digest(&blob[blob.len() - 5..]));
    });
    fs::write(&path, invalid_text).unwrap();
    assert_eq!(
        StagedBundle::open(path.clone(), BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );

    let json_bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module("index.js", b"export default {}"),
            ModuleInput {
                name: "data.json".to_owned(),
                module_type: ModuleType::Json,
                bytes: b"{}".to_vec(),
            },
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let invalid_json = rewrite_bundle(json_bundle.bytes(), |manifest, blob| {
        let entry = manifest
            .modules
            .iter_mut()
            .find(|entry| entry.name == "data.json")
            .unwrap();
        let start = entry.offset as usize;
        blob[start..start + 2].copy_from_slice(b"xx");
        entry.sha256 = hex::encode(sha2::Sha256::digest(&blob[start..start + 2]));
    });
    fs::write(&path, invalid_json).unwrap();
    assert_eq!(
        StagedBundle::open(path.clone(), BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );

    fs::write(&path, bundle.bytes()).unwrap();
    let staged = StagedBundle::open(path, BundleLimits::default()).unwrap();
    assert!(!format!("{staged:?}").contains(temp.path().to_string_lossy().as_ref()));
}
