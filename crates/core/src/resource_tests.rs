use super::*;

#[test]
fn resource_tokens_and_typed_json_are_strict() {
    for kind in [
        BindingKind::KvNamespace,
        BindingKind::R2Bucket,
        BindingKind::D1Database,
        BindingKind::DoNamespace,
    ] {
        assert_eq!(kind.to_string().parse::<BindingKind>().unwrap(), kind);
    }
    assert!("unknown".parse::<BindingKind>().is_err());

    for state in [
        ResourceState::Creating,
        ResourceState::Ready,
        ResourceState::Deleting,
        ResourceState::Tombstoned,
    ] {
        assert_eq!(state.as_str().parse::<ResourceState>().unwrap(), state);
    }
    assert!("corrupt".parse::<ResourceState>().is_err());

    for availability in [
        ResourceAvailability::Healthy,
        ResourceAvailability::Degraded,
        ResourceAvailability::Unavailable,
    ] {
        assert_eq!(
            availability
                .as_str()
                .parse::<ResourceAvailability>()
                .unwrap(),
            availability
        );
    }
    assert!("corrupt".parse::<ResourceAvailability>().is_err());

    let permissions: CanonicalPermissions =
        serde_json::from_str(r#"{"read":true,"write":false}"#).unwrap();
    assert!(permissions.read);
    assert!(!permissions.write);
    assert_eq!(
        CanonicalPermissions::default(),
        CanonicalPermissions {
            read: true,
            write: true
        }
    );
    assert!(
        serde_json::from_str::<CanonicalPermissions>(r#"{"read":true,"write":true,"x":1}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<CanonicalBindingConfig>(r#"{"x":1}"#).is_err());
    assert_eq!(
        serde_json::to_string(&CanonicalBindingConfig::default()).unwrap(),
        "{}"
    );
}
