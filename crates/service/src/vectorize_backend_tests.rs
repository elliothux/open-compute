use super::*;

fn frame(ids: &[&str]) -> Vec<u8> {
    let header = br#"{"operation":"insert","schemaVersion":1}"#;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"OCVZ");
    bytes.extend_from_slice(&1_u16.to_be_bytes());
    bytes.extend_from_slice(&u32::try_from(header.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(header);
    bytes.extend_from_slice(&u32::try_from(ids.len()).unwrap().to_be_bytes());
    for id in ids {
        bytes.extend_from_slice(&u16::try_from(id.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(id.as_bytes());
        bytes.extend_from_slice(&u16::MAX.to_be_bytes());
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        bytes.extend_from_slice(&2_u16.to_be_bytes());
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.extend_from_slice(&0.0_f32.to_le_bytes());
    }
    bytes
}

#[test]
fn mutation_frame_is_binary_bounded_and_exact() {
    let decoded = decode_mutation_frame(&frame(&["a", "b"]), VectorMutationKind::Insert).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].id, "a");
    assert_eq!(decoded[0].values.as_deref(), Some([1.0, 0.0].as_slice()));
}

#[test]
fn mutation_frame_rejects_trailing_and_truncated_but_keeps_first_duplicate() {
    let valid = frame(&["a"]);
    assert!(decode_mutation_frame(&valid[..valid.len() - 1], VectorMutationKind::Insert).is_err());
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(decode_mutation_frame(&trailing, VectorMutationKind::Insert).is_err());
    let duplicate =
        decode_mutation_frame(&frame(&["same", "same"]), VectorMutationKind::Insert).unwrap();
    assert_eq!(duplicate.len(), 1);
    assert_eq!(duplicate[0].id, "same");
    assert!(decode_mutation_frame(&frame(&["a"]), VectorMutationKind::Upsert).is_err());
}

#[test]
fn query_options_reject_unknown_projection_and_default_top_k_is_five() {
    let options: QueryOptions = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(options.top_k, 5);
    assert!(
        serde_json::from_value::<QueryOptions>(serde_json::json!({"returnMetadata":"future"}))
            .is_err()
    );
}

#[test]
fn get_by_ids_payload_requires_only_ids_and_no_options_object() {
    assert_eq!(
        parse_ids(&serde_json::json!({"ids": ["present", "absent"]})).unwrap(),
        ["present".to_string(), "absent".to_string()]
    );
    assert!(parse_ids(&serde_json::json!({"ids": ["a"], "options": {}})).is_err());
}

#[test]
fn mutation_frame_and_helper_failure_matrix_is_bounded() {
    let header = br#"{"operation":"upsert","schemaVersion":1}"#;
    let namespace = b"docs";
    let metadata = br#"{"title":"guide"}"#;
    let mut rich = Vec::new();
    rich.extend_from_slice(b"OCVZ");
    rich.extend_from_slice(&1_u16.to_be_bytes());
    rich.extend_from_slice(&u32::try_from(header.len()).unwrap().to_be_bytes());
    rich.extend_from_slice(header);
    rich.extend_from_slice(&1_u32.to_be_bytes());
    rich.extend_from_slice(&2_u16.to_be_bytes());
    rich.extend_from_slice(b"id");
    rich.extend_from_slice(&u16::try_from(namespace.len()).unwrap().to_be_bytes());
    rich.extend_from_slice(namespace);
    rich.extend_from_slice(&u32::try_from(metadata.len()).unwrap().to_be_bytes());
    rich.extend_from_slice(metadata);
    rich.extend_from_slice(&2_u16.to_be_bytes());
    rich.extend_from_slice(&0.5_f32.to_le_bytes());
    rich.extend_from_slice(&(-0.25_f32).to_le_bytes());
    let decoded = decode_mutation_frame(&rich, VectorMutationKind::Upsert).unwrap();
    assert_eq!(decoded[0].namespace.as_deref(), Some("docs"));
    assert_eq!(
        decoded[0].metadata,
        Some(serde_json::json!({"title":"guide"}))
    );

    assert!(decode_mutation_frame(&[], VectorMutationKind::Insert).is_err());
    let mut bad_magic = frame(&["a"]);
    bad_magic[0] = b'X';
    assert!(decode_mutation_frame(&bad_magic, VectorMutationKind::Insert).is_err());
    let mut bad_version = frame(&["a"]);
    bad_version[5] = 2;
    assert!(decode_mutation_frame(&bad_version, VectorMutationKind::Insert).is_err());
    let mut non_finite = frame(&["a"]);
    let length = non_finite.len();
    non_finite[length - 8..length - 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(decode_mutation_frame(&non_finite, VectorMutationKind::Insert).is_err());

    let empty = serde_json::json!({"ids": []});
    assert!(parse_ids(&empty).is_err());
    let too_many = serde_json::json!({"ids": vec!["x"; 1_001]});
    assert!(parse_ids(&too_many).is_err());
    assert!(require_empty_object(&serde_json::json!({})).is_ok());
    assert!(require_empty_object(&serde_json::json!({"x": 1})).is_err());
}

#[test]
fn metadata_projection_and_private_error_shapes_are_exact() {
    assert_eq!(
        serde_json::from_value::<ReturnMetadata>(serde_json::json!(false)).unwrap(),
        ReturnMetadata::None
    );
    assert_eq!(
        serde_json::from_value::<ReturnMetadata>(serde_json::json!(true)).unwrap(),
        ReturnMetadata::All
    );
    assert_eq!(
        serde_json::from_value::<ReturnMetadata>(serde_json::json!("indexed")).unwrap(),
        ReturnMetadata::Indexed
    );
    let record = VectorRecord {
        id: "id".to_owned(),
        namespace: Some("docs".to_owned()),
        values: vec![1.0, 0.0],
        metadata: Some(serde_json::json!({
            "nested": {"title": "é".repeat(40)},
            "tags": ["a", "b"],
            "count": 2,
        })),
    };
    let indexed = ["nested.title", "tags", "count", "missing"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let projected = project_metadata(&record, ReturnMetadata::Indexed, &indexed).unwrap();
    assert_eq!(projected["nested.title"].as_str().unwrap().len(), 64);
    assert_eq!(projected["tags"], serde_json::json!(["a", "b"]));
    assert_eq!(projected["count"], 2);
    assert!(projected.get("missing").is_none());
    assert_eq!(
        project_metadata(&record, ReturnMetadata::All, &indexed),
        record.metadata
    );
    assert_eq!(
        project_metadata(&record, ReturnMetadata::None, &indexed),
        None
    );
    assert!(resolve_path(&serde_json::json!({"a": 1}), "a.b").is_none());

    for (error, status) in [
        (permission_denied(), StatusCode::FORBIDDEN),
        (not_found(), StatusCode::NOT_FOUND),
        (limit_error(), StatusCode::PAYLOAD_TOO_LARGE),
        (unavailable(), StatusCode::SERVICE_UNAVAILABLE),
        (protocol_error(), StatusCode::BAD_REQUEST),
    ] {
        let response = error_response(&error);
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()["x-open-compute-error-code"],
            error.code().as_str()
        );
    }
}

#[tokio::test]
async fn cpu_query_executor_returns_the_operation_result() {
    assert_eq!(
        run_query_cpu(|| Ok::<_, PlatformError>(42)).await.unwrap(),
        42
    );
}
