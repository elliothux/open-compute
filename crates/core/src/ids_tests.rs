use super::*;
use crate::InstanceId;

#[test]
fn generated_instance_id_is_compact_lowercase_uuidv7() {
    let id = InstanceId::generate();
    let s = id.to_string();
    assert_eq!(s, s.to_ascii_lowercase());
    assert_eq!(s.len(), crate::INSTANCE_ID_LEN);
    assert!(s.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(InstanceId::from_str(&s).expect("parse"), id);
    assert_eq!(id.as_uuid().get_version(), Some(uuid::Version::SortRand));
}

#[test]
fn rejects_uppercase_and_non_v7() {
    let v7 = StartupId::generate().to_string().to_ascii_uppercase();
    assert!(StartupId::from_str(&v7).is_err());
    let v4 = Uuid::nil();
    assert!(InstanceId::from_uuid(v4).is_err());
    assert!(RequestId::from_str("not-a-uuid").is_err());
    assert!(RequestId::from_str("00000000000000000000000000000000").is_err());
}

#[test]
fn serde_rejects_non_canonical_and_non_v7_ids() {
    #[derive(Deserialize)]
    struct Wrap {
        id: InstanceId,
    }

    let v7 = InstanceId::generate().to_string();
    let parsed: InstanceId = serde_json::from_str(&format!("\"{v7}\"")).expect("json v7");
    assert_eq!(parsed.to_string(), v7);
    let wrapped: Wrap = toml::from_str(&format!("id = \"{v7}\"\n")).expect("toml v7");
    assert_eq!(wrapped.id.to_string(), v7);

    let v4 = "550e8400-e29b-41d4-a716-446655440000";
    assert!(serde_json::from_str::<InstanceId>(&format!("\"{v4}\"")).is_err());
    assert!(toml::from_str::<Wrap>(&format!("id = \"{v4}\"\n")).is_err());

    let uppercase = v7.to_ascii_uppercase();
    assert!(serde_json::from_str::<InstanceId>(&format!("\"{uppercase}\"")).is_err());
    assert!(toml::from_str::<Wrap>(&format!("id = \"{uppercase}\"\n")).is_err());

    let hyphenated = parsed.as_uuid().to_string();
    assert!(serde_json::from_str::<InstanceId>(&format!("\"{hyphenated}\"")).is_err());
    assert!(toml::from_str::<Wrap>(&format!("id = \"{hyphenated}\"\n")).is_err());
}

#[test]
fn all_id_kinds_round_trip() {
    for s in [
        InstanceId::generate().to_string(),
        InstanceId::generate().to_string(),
        StartupId::generate().to_string(),
        RequestId::generate().to_string(),
        WorkerId::generate().to_string(),
        VersionId::generate().to_string(),
        DeploymentId::generate().to_string(),
        ResourceId::generate().to_string(),
        BindingId::generate().to_string(),
    ] {
        assert_eq!(s, s.to_ascii_lowercase());
    }
}

#[test]
fn workflow_operations_accept_system_random_uuid_without_relaxing_resource_ids() {
    let wire = "550e8400-e29b-41d4-a716-446655440000";
    let operation: WorkflowOperationId = serde_json::from_value(serde_json::json!(wire)).unwrap();
    assert_eq!(operation.to_string(), wire);
    assert_eq!(
        WorkflowOperationId::from_uuid(operation.as_uuid()).unwrap(),
        operation
    );
    assert!(WorkflowInstanceId::from_str(wire).is_err());
    for invalid in [
        wire.to_uppercase(),
        wire.replace('-', ""),
        Uuid::nil().to_string(),
    ] {
        assert!(WorkflowOperationId::from_str(&invalid).is_err());
    }
    let generated = WorkflowOperationId::generate();
    assert_eq!(
        generated
            .to_string()
            .parse::<WorkflowOperationId>()
            .unwrap(),
        generated
    );
}
