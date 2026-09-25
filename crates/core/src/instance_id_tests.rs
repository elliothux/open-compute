use super::*;

#[test]
fn generated_identity_is_canonical_and_round_trips() {
    let id = InstanceId::generate();
    assert_eq!(id.as_str().len(), INSTANCE_ID_LEN);
    assert!(id.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(format!("{id}"), id.as_str());
    assert!(format!("{id:?}").contains(id.as_str()));
    assert_eq!(id.as_str().parse::<InstanceId>().unwrap(), id);
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{id}\""));
    assert_eq!(
        serde_json::from_str::<InstanceId>(&format!("\"{id}\"")).unwrap(),
        id
    );

    let selector = InstanceSelector::from(id);
    assert_eq!(selector.as_str(), id.as_str());
    assert_eq!(format!("{selector}"), id.as_str());
    let encoded = serde_json::to_string(&selector).unwrap();
    assert_eq!(
        serde_json::from_str::<InstanceSelector>(&encoded).unwrap(),
        selector
    );
}

#[test]
fn identity_round_trips_through_its_stored_uuid_not_a_path() {
    let instance_id = InstanceId::generate();
    let restored = InstanceId::from_uuid(instance_id.as_uuid()).unwrap();
    assert_eq!(restored, instance_id);
}

#[test]
fn selector_accepts_names_and_rejects_noncanonical_values() {
    let name: InstanceName = "dev".parse().unwrap();
    assert_eq!(name.as_str(), "dev");
    assert_eq!("dev".parse::<InstanceSelector>().unwrap().as_str(), "dev");
    assert_eq!(serde_json::to_string(&name).unwrap(), "\"dev\"");
    for invalid in [
        "0199ABCDEF0123456789abcdef012345",
        "0199abcdef01-2345-6789-abcdef012345",
        "00000000000000000000000000000000",
        "Dev",
        "dev_name",
        "",
    ] {
        assert!(invalid.parse::<InstanceSelector>().is_err(), "{invalid}");
        assert!(serde_json::from_str::<InstanceId>(&format!("\"{invalid}\"")).is_err());
    }
    let id = InstanceId::generate();
    assert!(id.as_str().parse::<InstanceName>().is_err());
}
