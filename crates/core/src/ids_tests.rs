use super::*;

#[test]
fn generated_ids_are_lowercase_uuidv7() {
    let id = PlatformId::generate();
    let s = id.to_string();
    assert_eq!(s, s.to_ascii_lowercase());
    assert_eq!(s.chars().filter(|c| *c == '-').count(), 4);
    assert_eq!(PlatformId::from_str(&s).expect("parse"), id);
    assert_eq!(id.as_uuid().get_version(), Some(uuid::Version::SortRand));
}

#[test]
fn rejects_uppercase_and_non_v7() {
    let v7 = StartupId::generate().to_string().to_ascii_uppercase();
    assert!(StartupId::from_str(&v7).is_err());
    let v4 = Uuid::nil();
    assert!(AccountId::from_uuid(v4).is_err());
    assert!(RequestId::from_str("not-a-uuid").is_err());
    assert!(RequestId::from_str("00000000000000000000000000000000").is_err());
}

#[test]
fn serde_rejects_non_canonical_and_non_v7_ids() {
    #[derive(Deserialize)]
    struct Wrap {
        id: PlatformId,
    }

    let v7 = PlatformId::generate().to_string();
    let parsed: PlatformId = serde_json::from_str(&format!("\"{v7}\"")).expect("json v7");
    assert_eq!(parsed.to_string(), v7);
    let wrapped: Wrap = toml::from_str(&format!("id = \"{v7}\"\n")).expect("toml v7");
    assert_eq!(wrapped.id.to_string(), v7);

    let v4 = "550e8400-e29b-41d4-a716-446655440000";
    assert!(serde_json::from_str::<PlatformId>(&format!("\"{v4}\"")).is_err());
    assert!(toml::from_str::<Wrap>(&format!("id = \"{v4}\"\n")).is_err());

    let uppercase = v7.to_ascii_uppercase();
    assert!(serde_json::from_str::<PlatformId>(&format!("\"{uppercase}\"")).is_err());
    assert!(toml::from_str::<Wrap>(&format!("id = \"{uppercase}\"\n")).is_err());

    let compact = v7.replace('-', "");
    assert!(serde_json::from_str::<PlatformId>(&format!("\"{compact}\"")).is_err());
    assert!(toml::from_str::<Wrap>(&format!("id = \"{compact}\"\n")).is_err());
}

#[test]
fn all_id_kinds_round_trip() {
    for s in [
        PlatformId::generate().to_string(),
        AccountId::generate().to_string(),
        StartupId::generate().to_string(),
        RequestId::generate().to_string(),
        WorkerId::generate().to_string(),
        DeploymentId::generate().to_string(),
    ] {
        assert_eq!(s, s.to_ascii_lowercase());
    }
}
