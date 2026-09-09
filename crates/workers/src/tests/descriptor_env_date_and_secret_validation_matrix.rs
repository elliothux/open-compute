use super::*;

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

    let mut deep = serde_json::Value::Null;
    for _ in 0..34 {
        deep = serde_json::Value::Array(vec![deep]);
    }
    assert_eq!(
        canonicalize_vars(BTreeMap::from([("DEEP".to_owned(), deep)]))
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimitExceeded
    );
    let (vars, encoded) = canonicalize_vars(BTreeMap::from([(
        "OBJECT".to_owned(),
        serde_json::json!({"z": 1, "a": [true, null]}),
    )]))
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
            "2026-09-08".into(),
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
