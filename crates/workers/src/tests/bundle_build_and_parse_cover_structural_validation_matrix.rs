use super::*;

#[test]
fn bundle_build_and_parse_cover_structural_validation_matrix() {
    let limits = BundleLimits::default();
    assert_eq!(
        CanonicalBundle::build("index.js", Vec::new(), limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
    let too_many = vec![module("a.js", b""); 2];
    assert_eq!(
        CanonicalBundle::build(
            "a.js",
            too_many,
            BundleLimits {
                max_modules: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );
    assert_eq!(
        CanonicalBundle::build(
            "missing.js",
            vec![module("index.js", b"export default {}")],
            limits,
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleInvalid
    );
    assert_eq!(
        CanonicalBundle::build(
            "index.js",
            vec![
                module("index.js", b"export default {}"),
                module("index.js", b"export default {}"),
            ],
            limits,
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleInvalid
    );
    for (module_type, bytes) in [
        (ModuleType::Text, vec![0xff]),
        (ModuleType::Json, b"not-json".to_vec()),
    ] {
        let mut input = module("index.js", &bytes);
        input.module_type = module_type;
        assert_eq!(
            CanonicalBundle::build("index.js", vec![input], limits)
                .unwrap_err()
                .code(),
            ErrorCode::BundleInvalid
        );
    }
    assert_eq!(
        CanonicalBundle::build(
            "index.js",
            vec![module("index.js", b"a"), module("other.js", b"b"),],
            BundleLimits {
                max_total_module_bytes: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );

    let bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module("index.js", b"export default {}"),
            module("other.js", b"export const x = 1"),
        ],
        limits,
    )
    .unwrap();
    assert!(format!("{bundle:?}").contains("CanonicalBundle"));
    for mutation in [
        rewrite_bundle(bundle.bytes(), |manifest, _| manifest.schema_version = 2),
        rewrite_bundle(bundle.bytes(), |manifest, _| manifest.modules.clear()),
        rewrite_bundle(bundle.bytes(), |manifest, _| {
            manifest.main_module = "missing.js".to_owned();
        }),
        rewrite_bundle(bundle.bytes(), |manifest, _| manifest.modules[1].offset = 0),
        rewrite_bundle(bundle.bytes(), |manifest, _| manifest.modules.swap(0, 1)),
        rewrite_bundle(bundle.bytes(), |manifest, _| {
            manifest.modules[0].module_type = ModuleType::Text;
        }),
    ] {
        assert_eq!(
            CanonicalBundle::parse(mutation, limits).unwrap_err().code(),
            ErrorCode::BundleInvalid
        );
    }

    let noncanonical_manifest = {
        let manifest_len = u32::from_be_bytes(bundle.bytes()[8..12].try_into().unwrap()) as usize;
        let manifest: WorkerBundleManifest =
            serde_json::from_slice(&bundle.bytes()[12..12 + manifest_len]).unwrap();
        let encoded = serde_json::to_vec_pretty(&manifest).unwrap();
        let mut bytes = b"OCWB\0\x01\0\0".to_vec();
        bytes.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&encoded);
        bytes.extend_from_slice(&bundle.bytes()[12 + manifest_len..]);
        bytes
    };
    assert_eq!(
        CanonicalBundle::parse(noncanonical_manifest, limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
    assert_eq!(
        CanonicalBundle::parse(
            bundle.bytes().to_vec(),
            BundleLimits {
                max_module_bytes: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );
    assert_eq!(
        CanonicalBundle::parse(
            bundle.bytes().to_vec(),
            BundleLimits {
                max_total_module_bytes: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );
    let first = &bundle.manifest().modules[0];
    assert_eq!(bundle.module_bytes(first).unwrap(), b"export default {}");
    let mut outside = first.clone();
    outside.offset = u64::MAX;
    assert_eq!(
        bundle.module_bytes(&outside).unwrap_err().code(),
        ErrorCode::BundleTooLarge
    );
    assert!(!bundle.clone().into_bytes().is_empty());
}
