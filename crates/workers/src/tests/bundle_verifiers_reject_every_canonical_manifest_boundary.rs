use super::*;

#[test]
fn bundle_verifiers_reject_every_canonical_manifest_boundary() {
    let limits = BundleLimits::default();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module("index.js", b"export default {}"),
            module("other.js", b"export const other = 1"),
        ],
        limits,
    )
    .unwrap();

    assert_eq!(
        CanonicalBundle::build(
            "index.js",
            vec![module("index.js", b"x")],
            BundleLimits {
                max_manifest_bytes: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );
    assert_eq!(
        CanonicalBundle::build(
            "index.js",
            vec![module("index.js", b"x")],
            BundleLimits {
                max_artifact_bytes: 1,
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
                max_artifact_bytes: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );
    assert_eq!(
        CanonicalBundle::parse(vec![0_u8; 12], limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
    let mut empty_manifest = b"OCWB\0\x01\0\0".to_vec();
    empty_manifest.extend_from_slice(&0_u32.to_be_bytes());
    assert_eq!(
        CanonicalBundle::parse(empty_manifest, limits)
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
    assert_eq!(
        CanonicalBundle::parse(
            bundle.bytes().to_vec(),
            BundleLimits {
                max_manifest_bytes: 1,
                ..limits
            },
        )
        .unwrap_err()
        .code(),
        ErrorCode::BundleTooLarge
    );

    let noncanonical_main = rewrite_bundle(bundle.bytes(), |manifest, _| {
        manifest.main_module = "e\u{301}.js".to_owned();
    });
    let noncanonical_module = rewrite_bundle(bundle.bytes(), |manifest, _| {
        manifest.modules[1].name = "o\u{301}.js".to_owned();
    });
    let duplicate_module = rewrite_bundle(bundle.bytes(), |manifest, _| {
        manifest.modules[1].name = manifest.modules[0].name.clone();
    });
    let noncanonical_staged_manifest = {
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
    for mutation in [
        noncanonical_main.clone(),
        noncanonical_module.clone(),
        duplicate_module.clone(),
    ] {
        assert_eq!(
            CanonicalBundle::parse(mutation, limits).unwrap_err().code(),
            ErrorCode::BundleInvalid
        );
    }

    for (bytes, expected) in [
        (noncanonical_staged_manifest, ErrorCode::BundleInvalid),
        (
            rewrite_bundle(bundle.bytes(), |manifest, _| manifest.schema_version = 2),
            ErrorCode::BundleInvalid,
        ),
        (noncanonical_main, ErrorCode::BundleInvalid),
        (noncanonical_module, ErrorCode::BundleInvalid),
        (duplicate_module, ErrorCode::BundleInvalid),
        (
            rewrite_bundle(bundle.bytes(), |manifest, _| manifest.modules[1].offset = 0),
            ErrorCode::BundleInvalid,
        ),
        (
            rewrite_bundle(bundle.bytes(), |manifest, _| {
                manifest.modules[0].module_type = ModuleType::Text;
            }),
            ErrorCode::BundleInvalid,
        ),
        (
            rewrite_bundle(bundle.bytes(), |manifest, _| {
                manifest.main_module = "missing.js".to_owned();
            }),
            ErrorCode::BundleInvalid,
        ),
    ] {
        assert_eq!(staged_error(&bytes, limits), expected);
    }
    assert_eq!(
        staged_error(
            bundle.bytes(),
            BundleLimits {
                max_module_bytes: 1,
                ..limits
            }
        ),
        ErrorCode::BundleTooLarge
    );
    assert_eq!(
        staged_error(
            bundle.bytes(),
            BundleLimits {
                max_total_module_bytes: 1,
                ..limits
            }
        ),
        ErrorCode::BundleTooLarge
    );

    for module_type in [ModuleType::Text, ModuleType::Json] {
        let raw = if module_type == ModuleType::Text {
            vec![0xff]
        } else {
            b"not-json".to_vec()
        };
        let typed = CanonicalBundle::build(
            "index.js",
            vec![
                module("index.js", b"export default {}"),
                ModuleInput {
                    name: "raw.bin".to_owned(),
                    module_type: ModuleType::Data,
                    bytes: raw,
                },
            ],
            limits,
        )
        .unwrap();
        let invalid = rewrite_bundle(typed.bytes(), |manifest, _| {
            manifest
                .modules
                .iter_mut()
                .find(|module| module.name == "raw.bin")
                .unwrap()
                .module_type = module_type;
        });
        assert_eq!(
            CanonicalBundle::parse(invalid, limits).unwrap_err().code(),
            ErrorCode::BundleInvalid
        );
    }
}
