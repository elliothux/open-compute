use super::*;

#[test]
fn public_id_is_canonical_and_namespace_scoped() {
    let namespace = ResourceId::generate();
    let other = ResourceId::generate();
    let mut bytes = [7_u8; DURABLE_OBJECT_ID_BYTES];
    bytes[..8].copy_from_slice(&durable_object_namespace_prefix(namespace));
    let id = DurableObjectId::for_namespace(bytes, namespace).unwrap();
    assert_eq!(id.as_bytes(), &bytes);
    assert_eq!(id.to_string().len(), 64);
    assert!(id.belongs_to(namespace));
    assert!(!id.belongs_to(other));
    assert_eq!(id.to_string().parse::<DurableObjectId>().unwrap(), id);
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{id}\""));
    assert!(serde_json::from_str::<DurableObjectId>(&format!("\"{id}\"")).is_ok());
    assert_eq!(
        DurableObjectId::for_namespace(bytes, other)
            .unwrap_err()
            .code(),
        ErrorCode::DoIdInvalid
    );
}

#[test]
fn public_id_and_state_reject_noncanonical_values() {
    let invalid = [
        String::new(),
        "0".repeat(63),
        "0".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ];
    for value in invalid {
        assert!(value.parse::<DurableObjectId>().is_err(), "{value}");
    }
    for (raw, state) in [
        ("creating", DurableObjectState::Creating),
        ("ready", DurableObjectState::Ready),
        ("deleting", DurableObjectState::Deleting),
        ("tombstoned", DurableObjectState::Tombstoned),
    ] {
        assert_eq!(raw.parse::<DurableObjectState>().unwrap(), state);
        assert_eq!(state.as_str(), raw);
    }
    assert!("other".parse::<DurableObjectState>().is_err());
}
