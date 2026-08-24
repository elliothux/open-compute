use super::*;

#[test]
fn rejects_bad_digest_version_and_serde() {
    assert!(ArtifactRef::new(2, &"a".repeat(64), 1).is_err());
    assert!(ArtifactRef::new(1, &"A".repeat(64), 1).is_err());
    assert!(ArtifactRef::new(1, "abc", 1).is_err());
    let ok = ArtifactRef::new(1, &"ab".repeat(32), 12).expect("ref");
    assert_eq!(ok.version(), 1);
    assert_eq!(ok.size(), 12);
    let json = serde_json::to_string(&ok).expect("json");
    assert!(json.contains("\"version\":1"));
    assert!(!json.contains("s3.example"));
    assert!(!json.contains("bucket"));
    let back: ArtifactRef = serde_json::from_str(&json).expect("parse");
    assert_eq!(back, ok);
    assert!(
        serde_json::from_str::<ArtifactRef>("{\"version\":1,\"sha256\":\"FFFF\",\"size\":1}")
            .is_err()
    );
}

#[test]
fn physical_key_shape() {
    let digest = "ab".repeat(32);
    let r = ArtifactRef::new(1, &digest, 1).unwrap();
    let key = r.physical_key("system/");
    assert_eq!(
        key,
        format!(
            "system/artifacts/v1/sha256/{}/{}",
            &digest[..2],
            &digest[2..]
        )
    );
    assert_eq!(parse_physical_key("system/", &key).unwrap(), digest);
    assert!(parse_physical_key("system/", "tenant/foo").is_err());
    assert!(parse_physical_key("system/", "system/artifacts/v1/sha256/no-slash").is_err());
    assert!(parse_physical_key("system/", "system/artifacts/v1/sha256/a/short").is_err());
    assert!(
        parse_physical_key(
            "system/",
            &format!("system/artifacts/v1/sha256/zz/{}", "z".repeat(62))
        )
        .is_err()
    );

    assert_eq!(r.sha256_bytes(), &[0xab; 32]);
    assert_eq!(r.to_string(), format!("v1/sha256/{digest}#1"));
    let debug = format!("{r:?}");
    assert!(debug.contains(&digest));
    let json = serde_json::to_string(&r).unwrap();
    assert_eq!(json.parse::<ArtifactRef>().unwrap(), r);
    assert!("not json".parse::<ArtifactRef>().is_err());
}
