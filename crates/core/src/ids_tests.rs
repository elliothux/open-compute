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

#[test]
fn sqlite_ids_preserve_canonical_text_and_parse_errors() {
    use rusqlite::types::{FromSql, FromSqlError, ToSql, ValueRef};

    fn check<T>(id: T)
    where
        T: ToSql + FromSql + Display + std::fmt::Debug + PartialEq,
    {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let (stored, kind, decoded): (String, String, T) = connection
            .query_row("SELECT ?1, typeof(?1), ?1", [&id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(stored.as_bytes(), id.to_string().as_bytes());
        assert_eq!(kind, "text");
        assert_eq!(decoded, id);
        for invalid in [
            "019C0000-0000-7000-8000-000000000001".to_owned(),
            "not-a-uuid".to_owned(),
        ] {
            let error = T::column_result(ValueRef::Text(invalid.as_bytes())).unwrap_err();
            let FromSqlError::Other(error) = error else {
                panic!("expected the existing parse error");
            };
            assert!(error.downcast_ref::<PlatformError>().is_some());
        }
        assert!(matches!(
            T::column_result(ValueRef::Blob(stored.as_bytes())),
            Err(FromSqlError::InvalidType)
        ));
        assert!(matches!(
            T::column_result(ValueRef::Null),
            Err(FromSqlError::InvalidType)
        ));
    }

    check(InstanceId::generate());
    check(StartupId::generate());
    check(RequestId::generate());
    check(WorkerId::generate());
    check(VersionId::generate());
    check(DeploymentId::generate());
    check(VersionUploadId::generate());
    check(ResourceId::generate());
    check(BindingId::generate());
    check(ArtifactRepoId::generate());
    check(ArtifactTokenId::generate());
    check(QueueId::generate());
    check(QueueMessageId::generate());
    check(QueueConsumerId::generate());
    check(QueueBatchId::generate());
    check(CronActivationId::generate());
    check(WorkflowId::generate());
    check(WorkflowVersionId::generate());
    check(WorkflowInstanceId::generate());
    check(WorkflowOperationId::generate());
    check(
        "550e8400-e29b-41d4-a716-446655440000"
            .parse::<WorkflowOperationId>()
            .unwrap(),
    );
    check(CronRunId::generate());

    for invalid in [
        "550e8400-e29b-41d4-a716-446655440000",
        "019c0000000070008000000000000001",
    ] {
        assert!(WorkerId::column_result(ValueRef::Text(invalid.as_bytes())).is_err());
    }
    assert!(
        InstanceId::column_result(ValueRef::Text(b"019c0000-0000-7000-8000-000000000001")).is_err()
    );
}
