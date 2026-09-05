use crate::pipeline::{
    idempotency_ref_id, stable_validation_code, validate_idempotency_key, validate_secret_set,
};
use crate::runtime_source::{invariant as source_invariant, map_artifact_error, not_ready};
use crate::*;
use open_compute_artifacts::{
    ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, MockS3, ObjectBackend,
    resolve_s3_credentials_with,
};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    AccountId, CacheConfig, DataConfig, ErrorCode, PlatformConfig, RequestId, SecretString,
    StartupId, VersionId, WorkerId,
};
use open_compute_storage::{PlatformStorage, VersionState, WorkerRepository};
use sha2::Digest as _;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

struct AcceptAllValidator;

impl RuntimeValidator for AcceptAllValidator {
    fn validate(
        &self,
        _candidate: ValidationCandidate,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<(), open_compute_core::PlatformError>> + Send + '_>,
    > {
        Box::pin(async { Ok(()) })
    }

    fn validate_entrypoint(
        &self,
        _candidate: ValidationCandidate,
        _entrypoint: String,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<(), open_compute_core::PlatformError>> + Send + '_>,
    > {
        Box::pin(async { Ok(()) })
    }
}

fn module(name: &str, bytes: &[u8]) -> ModuleInput {
    ModuleInput {
        name: name.to_owned(),
        module_type: ModuleType::EsModule,
        bytes: bytes.to_vec(),
    }
}

fn rewrite_bundle(
    bytes: &[u8],
    mutate: impl FnOnce(&mut WorkerBundleManifest, &mut Vec<u8>),
) -> Vec<u8> {
    let manifest_len = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut manifest: WorkerBundleManifest =
        serde_json::from_slice(&bytes[12..12 + manifest_len]).unwrap();
    let mut blob = bytes[12 + manifest_len..].to_vec();
    mutate(&mut manifest, &mut blob);
    let encoded = serde_json::to_vec(&manifest).unwrap();
    let mut out = b"OCWB\0\x01\0\0".to_vec();
    out.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    out.extend_from_slice(&encoded);
    out.extend_from_slice(&blob);
    out
}

fn staged_error(bytes: &[u8], limits: BundleLimits) -> ErrorCode {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("bundle.ocwb");
    fs::write(&path, bytes).unwrap();
    StagedBundle::open(path, limits).unwrap_err().code()
}

#[test]
fn bundle_is_canonical_and_round_trips() {
    let a = CanonicalBundle::build(
        "src/index.js",
        vec![
            module("src/z.js", b"export const z = 1;"),
            module("src/index.js", b"export default { fetch() {} };"),
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let b = CanonicalBundle::build(
        "src/index.js",
        vec![
            module("src/index.js", b"export default { fetch() {} };"),
            module("src/z.js", b"export const z = 1;"),
        ],
        BundleLimits::default(),
    )
    .unwrap();
    assert_eq!(a.bytes(), b.bytes());
    assert_eq!(a.sha256(), b.sha256());
    let parsed = CanonicalBundle::parse(a.bytes().to_vec(), BundleLimits::default()).unwrap();
    assert_eq!(parsed.manifest(), a.manifest());
    assert_eq!(parsed.manifest().modules[0].name, "src/index.js");
}

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

#[test]
fn bundle_rejects_path_digest_offset_and_trailing_attacks() {
    for name in [
        "../index.js",
        "/index.js",
        "a\\b.js",
        "a//b.js",
        "a/./b.js",
        "a/\0b.js",
    ] {
        let err = CanonicalBundle::build(
            name,
            vec![module(name, b"export default {}")],
            BundleLimits::default(),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::BundleInvalid);
    }
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![module("index.js", b"export default {}")],
        BundleLimits::default(),
    )
    .unwrap();
    let mut corrupted = bundle.bytes().to_vec();
    *corrupted.last_mut().unwrap() ^= 1;
    assert_eq!(
        CanonicalBundle::parse(corrupted, BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    let mut trailing = bundle.bytes().to_vec();
    trailing.push(0);
    assert_eq!(
        CanonicalBundle::parse(trailing, BundleLimits::default())
            .unwrap_err()
            .code(),
        ErrorCode::BundleInvalid
    );
}

#[test]
fn bundle_limits_and_main_type_are_enforced() {
    let limits = BundleLimits {
        max_module_bytes: 3,
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
    let (vars, _) = canonicalize_vars(vars, 16, 4096).unwrap();
    let descriptor = WorkerCodeDescriptorV1::new(
        account,
        worker,
        version,
        0,
        "2026-08-30".into(),
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
    assert_eq!(encoded["compatibilityDate"], "2026-08-30");
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

#[test]
fn vars_reject_reserved_names_and_prototype_keys() {
    let mut vars = BTreeMap::new();
    vars.insert("OPEN_COMPUTE_TOKEN".to_owned(), serde_json::json!(1));
    assert!(canonicalize_vars(vars, 10, 1000).is_err());
    let mut vars = BTreeMap::new();
    vars.insert(
        "SAFE".to_owned(),
        serde_json::json!({"__proto__": {"x": 1}}),
    );
    assert!(canonicalize_vars(vars, 10, 1000).is_err());
}

#[test]
fn descriptor_env_date_and_secret_validation_matrix() {
    for valid in ["A", "_A", "$SERVICE", "VALUE_123"] {
        validate_env_name(valid).unwrap();
    }
    for invalid in [
        "",
        "1A",
        "A-B",
        "OPEN_COMPUTE_TOKEN",
        "__proto__",
        &"A".repeat(129),
    ] {
        assert_eq!(
            validate_env_name(invalid).unwrap_err().code(),
            ErrorCode::BundleInvalid
        );
    }

    assert_eq!(
        canonicalize_vars(
            BTreeMap::from([("A".to_owned(), serde_json::json!(1))]),
            0,
            1024,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceLimitExceeded
    );
    assert_eq!(
        canonicalize_vars(
            BTreeMap::from([("LONG".to_owned(), serde_json::json!("value"))]),
            1,
            1,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceLimitExceeded
    );
    let mut deep = serde_json::Value::Null;
    for _ in 0..34 {
        deep = serde_json::Value::Array(vec![deep]);
    }
    assert_eq!(
        canonicalize_vars(BTreeMap::from([("DEEP".to_owned(), deep)]), 1, 4096)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimitExceeded
    );
    let (vars, encoded) = canonicalize_vars(
        BTreeMap::from([(
            "OBJECT".to_owned(),
            serde_json::json!({"z": 1, "a": [true, null]}),
        )]),
        1,
        4096,
    )
    .unwrap();
    assert_eq!(encoded["OBJECT"], br#"{"a":[true,null],"z":1}"#);
    assert_eq!(vars["OBJECT"]["z"], 1);

    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let version = VersionId::generate();
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![module("index.js", b"export default {}")],
        BundleLimits::default(),
    )
    .unwrap();
    let valid_secret = SecretDescriptor {
        name: "TOKEN".to_owned(),
        revision_id: "revision".to_owned(),
        ciphertext_sha256: "ab".repeat(32),
    };
    let build = |vars: BTreeMap<String, serde_json::Value>, secrets: Vec<SecretDescriptor>| {
        WorkerCodeDescriptorV1::new(
            account,
            worker,
            version,
            0,
            "2026-08-30".into(),
            Vec::new(),
            Some((bundle.sha256(), bundle.manifest())),
            None,
            vars,
            secrets,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            CachePolicyDescriptorV1::default(),
            Vec::new(),
            1,
        )
    };
    assert_eq!(
        build(
            BTreeMap::new(),
            vec![valid_secret.clone(), valid_secret.clone()]
        )
        .unwrap_err()
        .code(),
        ErrorCode::SecretInvalid
    );
    assert_eq!(
        build(
            BTreeMap::from([("TOKEN".to_owned(), serde_json::json!(1))]),
            vec![valid_secret.clone()],
        )
        .unwrap_err()
        .code(),
        ErrorCode::SecretInvalid
    );
    for secret in [
        SecretDescriptor {
            name: "TOKEN".to_owned(),
            revision_id: String::new(),
            ciphertext_sha256: "ab".repeat(32),
        },
        SecretDescriptor {
            name: "TOKEN".to_owned(),
            revision_id: "revision".to_owned(),
            ciphertext_sha256: "not-a-digest".to_owned(),
        },
        SecretDescriptor {
            name: "BAD-NAME".to_owned(),
            revision_id: "revision".to_owned(),
            ciphertext_sha256: "ab".repeat(32),
        },
    ] {
        assert!(build(BTreeMap::new(), vec![secret]).is_err());
    }
    assert_ne!(
        ciphertext_sha256(b"nonce", b"ciphertext"),
        ciphertext_sha256(b"nonce2", b"ciphertext")
    );
}

#[test]
fn loader_key_is_strict() {
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let version = VersionId::generate();
    let key = loader_key(account, worker, version);
    assert_eq!(parse_loader_key(&key).unwrap(), (account, worker, version));
    assert!(parse_loader_key(&format!("{key}/extra")).is_err());
    assert!(parse_loader_key(&key.replace('-', "%2d")).is_err());
    for invalid in ["", "a/b", "a/b/c", "a/b/c/d"] {
        assert_eq!(
            parse_loader_key(invalid).unwrap_err().code(),
            ErrorCode::VersionInvariantViolation
        );
    }
}

#[tokio::test]
async fn version_pins_timeout_unfence_retire_and_debug_paths() {
    let id = VersionId::generate();
    let pins = VersionPins::new();
    assert_eq!(pins.count(id), 0);
    pins.unfence(id);
    pins.retire_fence(id);

    let pin = pins.pin(id).unwrap();
    assert_eq!(pins.count(id), 1);
    assert!(format!("{pin:?}").contains(&id.to_string()));
    assert_eq!(
        pins.fence_and_wait(id, Duration::ZERO)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionReferenced
    );
    pins.unfence(id);
    let second = pins.pin(id).unwrap();
    assert_eq!(pins.count(id), 2);
    drop(second);
    assert_eq!(pins.count(id), 1);
    drop(pin);
    assert_eq!(pins.count(id), 0);

    pins.fence_and_wait(id, Duration::from_millis(50))
        .await
        .unwrap();
    pins.retire_fence(id);
    assert!(pins.pin(id).is_ok());
}

#[test]
fn version_pipeline_helper_contracts_cover_failure_code_matrix() {
    for key in ["", "contains space", "line\nbreak", &"x".repeat(129)] {
        assert_eq!(
            validate_idempotency_key(key).unwrap_err().code(),
            ErrorCode::IdempotencyConflict
        );
    }
    validate_idempotency_key("valid-key_123").unwrap();

    let account = AccountId::generate();
    let first = idempotency_ref_id(account, "version.create", "key");
    assert_eq!(first.len(), 64);
    assert_eq!(first, idempotency_ref_id(account, "version.create", "key"));
    assert_ne!(
        first,
        idempotency_ref_id(account, "version.create", "other")
    );

    let mut secrets = BTreeMap::new();
    assert!(validate_secret_set(&secrets, &BTreeMap::new()).is_ok());
    for index in 0..65 {
        secrets.insert(format!("SECRET_{index}"), SecretString::new("x"));
    }
    assert_eq!(
        validate_secret_set(&secrets, &BTreeMap::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretInvalid
    );
    for (name, value) in [
        ("BAD-NAME", "x".to_owned()),
        ("EMPTY", String::new()),
        ("LARGE", "x".repeat(16 * 1024 + 1)),
    ] {
        let values = BTreeMap::from([(name.to_owned(), SecretString::new(value))]);
        assert!(validate_secret_set(&values, &BTreeMap::new()).is_err());
    }
    let conflict = BTreeMap::from([("TOKEN".to_owned(), SecretString::new("x"))]);
    assert_eq!(
        validate_secret_set(
            &conflict,
            &BTreeMap::from([("TOKEN".to_owned(), serde_json::json!(1))]),
        )
        .unwrap_err()
        .code(),
        ErrorCode::SecretInvalid
    );
    let total = (0..5)
        .map(|index| {
            (
                format!("TOKEN_{index}"),
                SecretString::new("x".repeat(15 * 1024)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        validate_secret_set(&total, &BTreeMap::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretInvalid
    );

    for (input, expected) in [
        (ErrorCode::RuntimeUnavailable, ErrorCode::RuntimeUnavailable),
        (
            ErrorCode::RuntimeResultUnknown,
            ErrorCode::RuntimeResultUnknown,
        ),
        (
            ErrorCode::ResourceLimitExceeded,
            ErrorCode::ResourceLimitExceeded,
        ),
        (ErrorCode::ConfigInvalid, ErrorCode::BundleRuntimeInvalid),
    ] {
        assert_eq!(
            stable_validation_code(&open_compute_core::PlatformError::new(input, "safe")),
            expected
        );
    }
    let failure_codes = [
        ("ACCOUNT_NOT_FOUND", ErrorCode::AccountNotFound),
        ("WORKER_NOT_FOUND", ErrorCode::WorkerNotFound),
        ("WORKER_DELETED", ErrorCode::WorkerDeleted),
        ("VERSION_NOT_FOUND", ErrorCode::VersionNotFound),
        ("VERSION_NOT_READY", ErrorCode::VersionNotReady),
        (
            "VERSION_INVARIANT_VIOLATION",
            ErrorCode::VersionInvariantViolation,
        ),
        ("BUNDLE_INVALID", ErrorCode::BundleInvalid),
        ("BUNDLE_TOO_LARGE", ErrorCode::BundleTooLarge),
        ("BUNDLE_RUNTIME_INVALID", ErrorCode::BundleRuntimeInvalid),
        (
            "COMPATIBILITY_UNSUPPORTED",
            ErrorCode::CompatibilityUnsupported,
        ),
        ("ARTIFACT_UNAVAILABLE", ErrorCode::ArtifactUnavailable),
        (
            "ARTIFACT_INTEGRITY_ERROR",
            ErrorCode::ArtifactIntegrityError,
        ),
        ("SECRET_INVALID", ErrorCode::SecretInvalid),
        ("RESOURCE_LIMIT_EXCEEDED", ErrorCode::ResourceLimitExceeded),
        ("RUNTIME_UNAVAILABLE", ErrorCode::RuntimeUnavailable),
        ("RUNTIME_RESULT_UNKNOWN", ErrorCode::RuntimeResultUnknown),
        ("UNKNOWN", ErrorCode::Internal),
    ];
    for (code, expected) in failure_codes {
        assert_eq!(
            ErrorCode::from_stable_str(code).unwrap_or(ErrorCode::Internal),
            expected
        );
    }
}

fn storage_config(root: &std::path::Path) -> DataConfig {
    DataConfig {
        path: root.to_path_buf(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}

fn s3_config(endpoint: &str) -> open_compute_core::S3Config {
    PlatformConfig::from_toml_str(&format!(
        r#"
[data]
path = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[storage]
backend = "s3"
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 1500
"#
    ))
    .unwrap()
    .object_storage
    .as_s3()
    .expect("S3 config")
    .clone()
}

fn artifact_store(mock: &MockS3) -> ArtifactStore {
    let config = s3_config(&mock.endpoint);
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(ObjectBackend::connect_s3(&config, &credentials, 32 * 1024 * 1024).unwrap())
}

fn version_request(
    account_id: AccountId,
    worker_id: WorkerId,
    key: &str,
    secret: &str,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![
            module(
                "index.js",
                b"export default { fetch() { return new Response('hello'); } };",
            ),
            ModuleInput {
                name: "index.js.map".to_owned(),
                module_type: ModuleType::SourceMap,
                bytes: br#"{"version":3,"sources":["index.ts"],"names":[],"mappings":""}"#.to_vec(),
            },
        ],
        BundleLimits::default(),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("MODE".to_owned(), serde_json::json!("production"));
    let mut secrets = BTreeMap::new();
    secrets.insert("API_TOKEN".to_owned(), SecretString::new(secret));
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars,
        secrets,
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms: 10_000,
    }
}

#[tokio::test]
async fn version_pipeline_uploads_validates_promotes_and_replays() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(account, "pipeline", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let (target, _) = repo
        .create_worker(
            account,
            "pipeline-target",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let validator: Arc<dyn RuntimeValidator> = Arc::new(AcceptAllValidator);
    let controller = VersionController::new(
        &storage,
        artifact_store(&mock),
        validator,
        BundleLimits::default(),
    );
    let mut request = version_request(account, worker.id, "deploy-key", "pipeline-secret-value");
    request.services.insert(
        "CATALOG".to_owned(),
        VersionServiceInput {
            target_worker_id: target.id,
            entrypoint: Some("CatalogApi".to_owned()),
            props: Some(serde_json::json!({
                "constructor": {"enabled": true},
                "z": [1, {"__proto__": "ordinary JSON data"}],
            })),
        },
    );
    request.runtime_features.cache.entrypoints.insert(
        "CachedApi".to_owned(),
        VersionCachePolicyInput {
            enabled: true,
            cross_version_cache: false,
        },
    );
    let first = controller.create_version(request.clone()).await.unwrap();
    let (version_id, descriptor_hash) = match first {
        CreateVersionOutcome::Applied(result) => {
            assert!(result.deployment.is_some());
            assert_eq!(result.version.state, VersionState::Ready);
            (
                result.version.id,
                hex::encode(result.version.worker_code_sha256),
            )
        }
        CreateVersionOutcome::Replay(_) => panic!("first request cannot replay"),
    };
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        Some(version_id)
    );
    assert_eq!(mock.object_count(), 1);
    let replay = controller.create_version(request.clone()).await.unwrap();
    match replay {
        CreateVersionOutcome::Replay(bytes) => {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains(&version_id.to_string()));
            assert!(!text.contains("pipeline-secret-value"));
        }
        CreateVersionOutcome::Applied(_) => panic!("idempotency replay created a version"),
    }
    assert_eq!(repo.list_versions(account, worker.id).unwrap().len(), 1);
    assert_eq!(mock.object_count(), 1);

    let source = RuntimeSource::new(
        storage.clone(),
        artifact_store(&mock),
        BundleLimits::default(),
    );
    let snapshot = source
        .resolve(
            &loader_key(account, worker.id, version_id),
            &descriptor_hash,
            RuntimeScope::Runtime,
        )
        .await
        .unwrap();
    assert!(format!("{source:?}").contains("RuntimeSource"));
    assert!(format!("{snapshot:?}").contains("RuntimeSnapshot"));
    assert!(format!("{:?}", snapshot.modules[0]).contains("RuntimeModule"));
    assert_eq!(snapshot.modules.len(), 1);
    assert_eq!(snapshot.vars["MODE"], "production");
    let observability = snapshot.observability.as_ref().unwrap();
    assert_eq!(observability.account_id, account.to_string());
    assert_eq!(observability.worker_id, worker.id.to_string());
    assert_eq!(observability.version_id, version_id.to_string());
    assert_eq!(observability.script_name, worker.name);
    assert_eq!(observability.observability_generation, 1);
    assert!(observability.enabled && observability.logs_enabled && observability.persist);
    assert_eq!(snapshot.services.len(), 1);
    assert_eq!(snapshot.services[0].descriptor.name, "CATALOG");
    assert_eq!(snapshot.services[0].descriptor.target_worker_id, target.id);
    assert_eq!(
        snapshot.services[0].descriptor.entrypoint.as_deref(),
        Some("CatalogApi")
    );
    assert_eq!(
        snapshot.services[0].descriptor.props,
        request.services["CATALOG"].props
    );
    assert_eq!(
        snapshot.secrets["API_TOKEN"].expose(),
        "pipeline-secret-value"
    );
    assert!(!format!("{snapshot:?}").contains("pipeline-secret-value"));
    let payload = RuntimeSource::internal_payload(&snapshot).unwrap();
    assert!(
        std::str::from_utf8(payload.expose())
            .unwrap()
            .contains("pipeline-secret-value")
    );
    assert!(
        std::str::from_utf8(payload.expose())
            .unwrap()
            .contains("observabilityGeneration")
    );
    assert!(!format!("{payload:?}").contains("pipeline-secret-value"));
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                "bad-descriptor",
                RuntimeScope::Runtime,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );

    let cache = Arc::new(
        ArtifactCache::open(
            root.join("artifact-cache-test"),
            CacheConfig {
                max_bytes: 64 * 1024 * 1024,
                high_watermark_ratio: 0.9,
                low_watermark_ratio: 0.5,
                partial_grace_ms: 50,
                max_artifact_bytes: 32 * 1024 * 1024,
            },
            StartupId::generate(),
        )
        .unwrap(),
    );
    let cached_source = RuntimeSource::new(
        storage.clone(),
        artifact_store(&mock),
        BundleLimits::default(),
    )
    .with_cache(cache);
    let probe = cached_source
        .resolve(
            &loader_key(account, worker.id, version_id),
            &descriptor_hash,
            RuntimeScope::Probe,
        )
        .await
        .unwrap();
    assert!(probe.secrets.is_empty());
    assert!(probe.observability.is_none());
    assert_eq!(probe.modules, snapshot.modules);
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                &descriptor_hash,
                RuntimeScope::Validation,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionNotReady
    );

    let canonical_service_props =
        serde_json::to_vec(request.services["CATALOG"].props.as_ref().unwrap()).unwrap();
    let mut conflict = request;
    conflict.secrets.insert(
        "API_TOKEN".to_owned(),
        SecretString::new("different-secret"),
    );
    assert_eq!(
        controller
            .create_version(conflict)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(repo.version_referrers(version_id).unwrap().len(), 1);
    assert_eq!(
        repo.prune_expired_idempotency(10_000 + 24 * 60 * 60 * 1_000 + 1, 64)
            .unwrap(),
        1
    );
    assert!(repo.version_referrers(version_id).unwrap().is_empty());

    for entry in fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = fs::read(path).unwrap();
            assert!(
                !bytes
                    .windows(b"pipeline-secret-value".len())
                    .any(|window| { window == b"pipeline-secret-value" })
            );
        }
    }

    // Simulate out-of-band corruption by removing production guards. The
    // RuntimeSource descriptor checks must still fail closed.
    let conn = rusqlite::Connection::open(root.join("control.sqlite")).unwrap();
    conn.execute_batch("DROP TRIGGER version_services_update_guard;")
        .unwrap();
    conn.execute(
        "UPDATE version_services SET props_json = ?1 WHERE version_id = ?2 AND binding_name = 'CATALOG'",
        rusqlite::params![
            br#"{"z":[1,{"__proto__":"ordinary JSON data"}],"constructor":{"enabled":true}}"#,
            version_id.to_string()
        ],
    )
    .unwrap();
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                &descriptor_hash,
                RuntimeScope::Runtime,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );
    conn.execute(
        "UPDATE version_services SET props_json = ?1 WHERE version_id = ?2 AND binding_name = 'CATALOG'",
        rusqlite::params![
            canonical_service_props,
            version_id.to_string()
        ],
    )
    .unwrap();
    conn.execute_batch("DROP TRIGGER version_immutable_guard;")
        .unwrap();
    conn.execute(
        "UPDATE worker_versions SET worker_code_sha256 = zeroblob(32) WHERE id = ?1",
        [version_id.to_string()],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        source
            .resolve(
                &loader_key(account, worker.id, version_id),
                &descriptor_hash,
                RuntimeScope::Runtime,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::VersionInvariantViolation
    );
}

#[tokio::test]
async fn fixed_upload_finalize_resumes_one_cancelled_validating_version() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(
            account,
            "resume-upload",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap()
        .0;
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let version_id = VersionId::generate();
    let request = version_request(account, worker.id, "upload-resume", "secret");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let blocking_validator: Arc<dyn RuntimeValidator> = Arc::new({
        let started = started.clone();
        move |_: ValidationCandidate| {
            let started = started.lock().unwrap().take();
            async move {
                if let Some(started) = started {
                    let _ = started.send(());
                }
                std::future::pending::<Result<(), open_compute_core::PlatformError>>().await
            }
        }
    });
    let first_storage = storage.clone();
    let first_artifacts = artifacts.clone();
    let first_request = request.clone();
    let attempt = tokio::spawn(async move {
        VersionController::new(
            &first_storage,
            first_artifacts,
            blocking_validator,
            BundleLimits::default(),
        )
        .finalize_upload(first_request, version_id)
        .await
    });
    started_rx.await.unwrap();
    attempt.abort();
    assert!(attempt.await.unwrap_err().is_cancelled());
    let stranded = WorkerRepository::new(storage.db())
        .get_version(account, worker.id, version_id)
        .unwrap();
    assert_eq!(stranded.state, VersionState::Validating);
    let probe = RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default())
        .resolve(
            &loader_key(account, worker.id, version_id),
            &hex::encode(stranded.worker_code_sha256),
            RuntimeScope::Probe,
        )
        .await
        .unwrap();
    assert!(probe.secrets.is_empty());

    let recovered = VersionController::new(
        &storage,
        artifacts,
        Arc::new(AcceptAllValidator),
        BundleLimits::default(),
    )
    .finalize_upload(request, version_id)
    .await
    .unwrap();
    let CreateVersionOutcome::Applied(result) = recovered else {
        panic!("cancelled finalize must complete its fixed version");
    };
    assert_eq!(result.version.id, version_id);
    assert_eq!(result.version.state, VersionState::Ready);
    assert_eq!(
        WorkerRepository::new(storage.db())
            .list_versions(account, worker.id)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn assets_only_pipeline_commits_real_refs_without_fabricating_worker_code() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let storage =
        Arc::new(PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let worker = workers
        .create_worker(account, "static-site", RequestId::generate(), 1, 1_000_000)
        .unwrap()
        .0;
    let mock = MockS3::spawn("open-compute").await;
    let store = artifact_store(&mock);
    let bytes = bytes::Bytes::from_static(b"hello assets");
    let digest = sha2::Sha256::digest(&bytes);
    store
        .put_verified(
            futures::stream::once(async { Ok::<_, std::io::Error>(bytes) }),
            &hex::encode(digest),
            12,
        )
        .await
        .unwrap();
    let assets = VersionAssets {
        manifest: AssetManifestV1 {
            schema_version: 1,
            entries: vec![AssetEntryV1 {
                path: "/index.html".to_owned(),
                sha256: hex::encode(digest),
                size: 12,
                content_type: "text/html; charset=utf-8".to_owned(),
            }],
        },
        routing: AssetRoutingConfigV1 {
            schema_version: 1,
            binding: None,
            run_worker_first: RunWorkerFirst::All(false),
            html_handling: HtmlHandling::AutoTrailingSlash,
            not_found_handling: NotFoundHandling::Page404,
            headers: Vec::new(),
            redirects: Vec::new(),
        },
    };
    let controller = VersionController::new(
        &storage,
        store.clone(),
        Arc::new(AcceptAllValidator),
        BundleLimits::default(),
    );
    let request = CreateVersionRequest {
        account_id: account,
        worker_id: worker.id,
        idempotency_key: "assets-only".to_owned(),
        content: VersionContent::AssetsOnly {
            assets: assets.clone(),
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms: 10,
    };
    let result = match controller.create_version(request.clone()).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result,
        CreateVersionOutcome::Replay(_) => panic!("first assets version replayed"),
    };
    assert_eq!(
        result.version.content_kind,
        open_compute_storage::VersionContentKind::AssetsOnly
    );
    assert!(result.version.artifact_sha256.is_none());
    assert!(result.version.main_module.is_none());
    let stored = open_compute_storage::VersionAssetsRepository::new(storage.db())
        .get(result.version.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.logical_file_count, 1);
    assert_eq!(stored.logical_total_bytes, 12);
    assert_eq!(workers.referenced_artifacts().unwrap().len(), 2);
    assert_eq!(mock.object_count(), 2);
    let static_snapshot = RuntimeSource::new(storage.clone(), store, BundleLimits::default())
        .resolve(
            &loader_key(account, worker.id, result.version.id),
            &hex::encode(result.version.worker_code_sha256),
            RuntimeScope::Runtime,
        )
        .await
        .unwrap();
    assert_eq!(static_snapshot.main_module, None);
    assert!(static_snapshot.modules.is_empty());
    assert!(static_snapshot.assets.is_some());

    let mut invalid = request;
    invalid.idempotency_key = "assets-only-env".to_owned();
    invalid
        .vars
        .insert("MODE".to_owned(), serde_json::json!("x"));
    assert_eq!(
        controller.create_version(invalid).await.unwrap_err().code(),
        ErrorCode::AssetConfigUnsupported
    );
}

#[tokio::test]
async fn version_products_validate_ready_queue_dlq_entrypoint_counts_and_crons() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&tmp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "products", RequestId::generate(), 1, 1_000_000)
        .unwrap();
    let queues = open_compute_storage::QueueRepository::new(storage.db());
    let source = open_compute_core::QueueId::generate();
    let dlq = open_compute_core::QueueId::generate();
    let pending = open_compute_core::QueueId::generate();
    for (id, name, ready) in [
        (source, "product-source", true),
        (dlq, "product-dlq", true),
        (pending, "product-pending", false),
    ] {
        queues
            .insert_creating(
                account,
                id,
                name,
                open_compute_storage::QueueConfig::default(),
                2,
            )
            .unwrap();
        if ready {
            queues.mark_ready(account, id, 3).unwrap();
        }
    }
    let mock = MockS3::spawn("open-compute").await;
    let validator: Arc<dyn RuntimeValidator> = Arc::new(AcceptAllValidator);
    let controller = VersionController::new(
        &storage,
        artifact_store(&mock),
        validator,
        BundleLimits::default(),
    )
    .with_queue_consumer_limit(2);
    let consumer = QueueConsumerInput {
        queue: source,
        entrypoint: Some("Named_$1".to_owned()),
        config: open_compute_storage::QueueConsumerConfig {
            max_concurrency: 2,
            ..open_compute_storage::QueueConsumerConfig::default()
        },
        dead_letter_queue: Some(dlq),
    };
    let mut valid = version_request(account, worker.id, "products-valid", "secret");
    valid.deployment_source = None;
    valid.queue_consumers = vec![consumer.clone()];
    valid.crons = vec!["*/5 * * * *".to_owned(), "*/5 * * * *".to_owned()];
    let version = match controller.create_version(valid).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("product version replayed"),
    };
    let declarations = open_compute_storage::QueueConsumerRepository::new(storage.db())
        .version_declarations(version.id)
        .unwrap();
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].dlq_queue_id, Some(dlq));
    assert_eq!(declarations[0].dlq_lifecycle_generation, Some(1));
    let cron = open_compute_storage::CronRepository::new(storage.db())
        .version_config(version.id)
        .unwrap();
    assert_eq!(cron.declarations.len(), 1);
    assert_eq!(cron.declarations[0].expression, "*/5 * * * *");

    let mut cases = Vec::new();
    let mut duplicate = version_request(account, worker.id, "products-duplicate", "secret");
    duplicate.deployment_source = None;
    duplicate.queue_consumers = vec![consumer.clone(), consumer.clone()];
    cases.push((duplicate, ErrorCode::QueueConsumerConflict));

    let mut self_dlq = version_request(account, worker.id, "products-self-dlq", "secret");
    self_dlq.deployment_source = None;
    self_dlq.queue_consumers = vec![QueueConsumerInput {
        dead_letter_queue: Some(source),
        ..consumer.clone()
    }];
    cases.push((self_dlq, ErrorCode::QueueDlqInvalid));

    let mut pending_dlq = version_request(account, worker.id, "products-pending-dlq", "secret");
    pending_dlq.deployment_source = None;
    pending_dlq.queue_consumers = vec![QueueConsumerInput {
        dead_letter_queue: Some(pending),
        ..consumer.clone()
    }];
    cases.push((pending_dlq, ErrorCode::QueueDlqInvalid));

    let mut bad_entry = version_request(account, worker.id, "products-entry", "secret");
    bad_entry.deployment_source = None;
    bad_entry.queue_consumers = vec![QueueConsumerInput {
        entrypoint: Some("1-invalid".to_owned()),
        ..consumer.clone()
    }];
    cases.push((bad_entry, ErrorCode::EntrypointNotFound));

    let mut not_ready = version_request(account, worker.id, "products-not-ready", "secret");
    not_ready.deployment_source = None;
    not_ready.queue_consumers = vec![QueueConsumerInput {
        queue: pending,
        dead_letter_queue: None,
        ..consumer.clone()
    }];
    cases.push((not_ready, ErrorCode::QueueConsumerNotReady));

    let mut invalid_config = version_request(account, worker.id, "products-config", "secret");
    invalid_config.deployment_source = None;
    invalid_config.queue_consumers = vec![QueueConsumerInput {
        config: open_compute_storage::QueueConsumerConfig {
            max_concurrency: 3,
            ..consumer.config
        },
        ..consumer.clone()
    }];
    cases.push((invalid_config, ErrorCode::LimitInvalid));

    let mut invalid_cron = version_request(account, worker.id, "products-cron", "secret");
    invalid_cron.deployment_source = None;
    invalid_cron.crons = vec!["not a cron".to_owned()];
    cases.push((invalid_cron, ErrorCode::CronExpressionInvalid));

    let mut too_many = version_request(account, worker.id, "products-count", "secret");
    too_many.deployment_source = None;
    too_many.queue_consumers = vec![consumer; 65];
    cases.push((too_many, ErrorCode::QuotaExceeded));

    for (request, expected) in cases {
        assert_eq!(
            controller.create_version(request).await.unwrap_err().code(),
            expected
        );
    }
}

#[test]
fn runtime_source_error_mapping_is_stable_and_sanitized() {
    for input in [
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::CacheEntryCorrupt,
    ] {
        let mapped = map_artifact_error(open_compute_core::PlatformError::new(input, "raw path"));
        assert_eq!(mapped.code(), ErrorCode::ArtifactIntegrityError);
        assert!(!mapped.message().contains("raw path"));
    }
    let unavailable = map_artifact_error(open_compute_core::PlatformError::new(
        ErrorCode::ObjectStorageUnavailable,
        "signed URL",
    ));
    assert_eq!(unavailable.code(), ErrorCode::ArtifactUnavailable);
    assert!(!unavailable.message().contains("signed URL"));
    assert_eq!(not_ready().code(), ErrorCode::VersionNotReady);
    assert_eq!(
        source_invariant().code(),
        ErrorCode::VersionInvariantViolation
    );
}

#[tokio::test]
async fn shared_artifact_gc_waits_for_last_version_reference() {
    let temp = tempfile::tempdir().unwrap();
    let storage = PlatformStorage::bootstrap(&storage_config(temp.path()), &SystemClock).unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let request_id = RequestId::generate();
    let (first_worker, _) = repo
        .create_worker(account, "gc-first", request_id, 1, 1_000_000)
        .unwrap();
    let (second_worker, _) = repo
        .create_worker(account, "gc-second", request_id, 2, 1_000_000)
        .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_| async { Ok(()) });
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let mut first = version_request(account, first_worker.id, "gc-first", "same-secret");
    first.deployment_source = None;
    let mut second = version_request(account, second_worker.id, "gc-second", "same-secret");
    second.deployment_source = None;
    let first = match controller.create_version(first).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    };
    let second = match controller.create_version(second).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected replay"),
    };
    assert_eq!(first.artifact_sha256, second.artifact_sha256);
    assert_eq!(mock.object_count(), 1);
    let _ = repo.prune_expired_idempotency(i64::MAX, 100).unwrap();

    repo.begin_version_delete(account, first_worker.id, first.id)
        .unwrap();
    repo.finalize_version_delete(account, first_worker.id, first.id, request_id, 20)
        .unwrap();
    let referenced = repo
        .referenced_artifacts()
        .unwrap()
        .into_iter()
        .map(|(digest, size)| ArtifactRef::new(1, &hex::encode(digest), size).unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(
        artifacts
            .gc_unreferenced(
                &artifacts.fence_version_gc().await,
                &referenced,
                SystemTime::now() + Duration::from_secs(1),
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(mock.object_count(), 1);

    repo.begin_version_delete(account, second_worker.id, second.id)
        .unwrap();
    repo.finalize_version_delete(account, second_worker.id, second.id, request_id, 21)
        .unwrap();
    assert_eq!(
        artifacts
            .gc_unreferenced(
                &artifacts.fence_version_gc().await,
                &HashSet::new(),
                SystemTime::now() + Duration::from_secs(1),
            )
            .await
            .unwrap(),
        1
    );
    assert_eq!(mock.object_count(), 0);
}

#[tokio::test]
async fn validation_failure_is_rejected_replayed_and_never_promoted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let storage = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let account = storage.identity().default_account_id;
    let repo = WorkerRepository::new(storage.db());
    let (worker, _) = repo
        .create_worker(
            account,
            "invalid-runtime",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let mock = MockS3::spawn("open-compute").await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(move |_: ValidationCandidate| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async {
            Err(open_compute_core::PlatformError::new(
                ErrorCode::BundleRuntimeInvalid,
                "synthetic safe validator failure",
            ))
        }
    });
    let artifacts = artifact_store(&mock);
    let controller = VersionController::new(
        &storage,
        artifacts.clone(),
        validator.clone(),
        BundleLimits::default(),
    );
    let request = version_request(account, worker.id, "rejected-key", "rejected-secret");
    assert_eq!(
        controller
            .create_version(request.clone())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BundleRuntimeInvalid
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let versions = repo.list_versions(account, worker.id).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].state, VersionState::Rejected);
    assert_eq!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        None
    );

    drop(controller);
    drop(storage);
    let restarted = PlatformStorage::bootstrap(&storage_config(&root), &SystemClock).unwrap();
    let restarted_controller =
        VersionController::new(&restarted, artifacts, validator, BundleLimits::default());
    assert_eq!(
        restarted_controller
            .create_version(request)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::BundleRuntimeInvalid
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let restarted_repo = WorkerRepository::new(restarted.db());
    assert_eq!(
        restarted_repo
            .list_versions(account, worker.id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        restarted_repo
            .get_worker(account, worker.id)
            .unwrap()
            .active_version_id,
        None
    );
}
