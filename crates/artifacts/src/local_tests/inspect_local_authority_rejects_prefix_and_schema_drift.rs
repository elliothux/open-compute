use super::*;

#[test]
fn inspect_local_authority_rejects_prefix_and_schema_drift() {
    let fixture = Fixture::new();
    let marker_path = fixture.config.path.join("format.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    value["prefix"] = serde_json::json!("drifted/");
    fs::write(&marker_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert_eq!(
        ObjectBackend::inspect_local_authority(&fixture.config)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageAuthorityMismatch
    );

    value["prefix"] = serde_json::json!(fixture.config.prefix);
    value["schemaVersion"] = serde_json::json!(99);
    fs::write(&marker_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert_eq!(
        ObjectBackend::inspect_local_authority(&fixture.config)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageAuthorityMismatch
    );

    value["schemaVersion"] = serde_json::json!(1);
    value["platformId"] = serde_json::json!("not-a-uuid");
    fs::write(&marker_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert_eq!(
        ObjectBackend::inspect_local_authority(&fixture.config)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectStorageIntegrityError
    );
}
