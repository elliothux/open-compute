use super::super::tests::authorized_binding;
use super::*;
use open_compute_core::CanonicalPermissions;

#[test]
fn path_parser_is_exact_and_typed() {
    let id = BindingId::generate();
    for operation in [
        "get",
        "get-with-metadata",
        "get-many",
        "put",
        "delete",
        "list",
    ] {
        assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/{operation}")).is_some());
    }
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/get/extra")).is_none());
    assert!(parse_path("/internal/bindings/v1/kv/not-an-id/get").is_none());
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/unknown")).is_none());
    assert!(parse_path(&format!("/internal/bindings/v1/kv/{id}/echo")).is_none());
    assert_eq!(Operation::Get.metric(), BindingBackendOperation::Get);
    assert_eq!(Operation::Put.metric(), BindingBackendOperation::Put);
    assert_eq!(Operation::Delete.metric(), BindingBackendOperation::Delete);
    for (operation, expected) in [
        (Operation::Get, KvOperation::Get),
        (Operation::GetWithMetadata, KvOperation::GetWithMetadata),
        (Operation::GetMany, KvOperation::GetMany),
        (Operation::Put, KvOperation::Put),
        (Operation::Delete, KvOperation::Delete),
        (Operation::List, KvOperation::List),
    ] {
        assert_eq!(operation.kv_metric(), expected);
    }
}

#[test]
fn permission_matrix_is_operation_specific() {
    let mut binding = authorized_binding();
    binding.binding.permissions = CanonicalPermissions {
        read: true,
        write: false,
    };
    assert!(permission_allows(&binding, Operation::Get));
    assert!(!permission_allows(&binding, Operation::Put));
    assert!(!permission_allows(&binding, Operation::Delete));
}

#[test]
fn frame_protocol_round_trips_every_shape_and_rejects_ambiguous_inputs() {
    for operation in [Operation::Get, Operation::GetWithMetadata] {
        let KvCommand::Get { keys, cache_ttl } =
            parse_frame_command(operation, br#"{"keys":["one"],"cacheTtl":60}"#).unwrap()
        else {
            panic!("single get decoded to the wrong command")
        };
        assert_eq!(keys, ["one"]);
        assert_eq!(cache_ttl, Some(60));
    }
    let KvCommand::Get { keys, .. } = parse_frame_command(
        Operation::GetMany,
        br#"{"keys":["one","two"],"cacheTtl":null}"#,
    )
    .unwrap() else {
        panic!("multi get decoded to the wrong command")
    };
    assert_eq!(keys, ["one", "two"]);
    for (operation, body) in [
        (Operation::Get, br#"{"keys":[]}"#.as_slice()),
        (Operation::GetWithMetadata, br#"{"keys":["a","b"]}"#),
    ] {
        assert_eq!(
            parse_frame_command(operation, body).unwrap_err().code(),
            ErrorCode::KvTooManyKeys
        );
    }
    let too_many = serde_json::to_vec(&serde_json::json!({
        "keys": vec!["x"; open_compute_storage::KV_MAX_MULTI_GET_KEYS + 1]
    }))
    .unwrap();
    assert_eq!(
        parse_frame_command(Operation::GetMany, &too_many)
            .unwrap_err()
            .code(),
        ErrorCode::KvTooManyKeys
    );

    let header = serde_json::to_vec(&serde_json::json!({
        "key": "put",
        "expiration": 100,
        "expirationTtl": 60,
        "metadata": {"b": 2},
        "metadataPresent": true
    }))
    .unwrap();
    let mut put = u32::try_from(header.len()).unwrap().to_be_bytes().to_vec();
    put.extend_from_slice(&header);
    put.extend_from_slice(b"value");
    let KvCommand::Put {
        key,
        value,
        expiration,
        expiration_ttl,
        metadata_present,
        ..
    } = parse_frame_command(Operation::Put, &put).unwrap()
    else {
        panic!("put decoded to the wrong command")
    };
    assert_eq!(key, "put");
    assert_eq!(value, b"value");
    assert_eq!(expiration, Some(100));
    assert_eq!(expiration_ttl, Some(60));
    assert!(metadata_present);
    assert_eq!(
        parse_frame_command(Operation::Put, b"bad")
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );
    assert_eq!(
        parse_frame_command(Operation::Put, &[0, 0, 16, 1])
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );
    let oversized_header = serde_json::to_vec(&serde_json::json!({"key": "large"})).unwrap();
    let mut oversized_value = u32::try_from(oversized_header.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    oversized_value.extend_from_slice(&oversized_header);
    oversized_value.resize(
        oversized_value.len() + open_compute_storage::KV_MAX_VALUE_BYTES + 1,
        0,
    );
    assert_eq!(
        parse_frame_command(Operation::Put, &oversized_value)
            .unwrap_err()
            .code(),
        ErrorCode::KvValueTooLarge
    );

    assert!(matches!(
        parse_frame_command(Operation::Delete, br#"{"key":"gone"}"#).unwrap(),
        KvCommand::Delete { key } if key == "gone"
    ));
    assert!(matches!(
        parse_frame_command(
            Operation::List,
            br#"{"prefix":"pre","limit":10,"cursor":"next"}"#
        )
        .unwrap(),
        KvCommand::List { prefix, limit: 10, cursor: Some(cursor) }
            if prefix == "pre" && cursor == "next"
    ));
    assert!(matches!(
        parse_frame_command(
            Operation::List,
            br#"{"prefix":null,"limit":1000,"cursor":null}"#
        )
        .unwrap(),
        KvCommand::List {
            prefix,
            limit: 1000,
            cursor: None
        } if prefix.is_empty()
    ));
    assert!(matches!(
        parse_frame_command(Operation::List, br#"{"limit":5}"#).unwrap(),
        KvCommand::List { prefix, limit: 5, cursor: None } if prefix.is_empty()
    ));

    let entry = open_compute_storage::KvEntry {
        value: b"bytes".to_vec(),
        metadata_json: Some(br#"{"a":1}"#.to_vec()),
        expires_at_ms: Some(4_000),
    };
    for operation in [Operation::Get, Operation::GetWithMetadata] {
        let (content_type, bytes) = encode_frame_result(
            operation,
            KvCommandResult::Entries(vec![Some(entry.clone())]),
        )
        .unwrap();
        assert_eq!(content_type, FRAME_CONTENT_TYPE);
        assert!(bytes.starts_with(b"KVS1\x01"));
        assert!(bytes.ends_with(b"bytes"));
    }
    let (_, missing) =
        encode_frame_result(Operation::Get, KvCommandResult::Entries(vec![None])).unwrap();
    assert!(missing.starts_with(b"KVS1\x00"));
    let (_, without_metadata) = encode_frame_result(
        Operation::Get,
        KvCommandResult::Entries(vec![Some(open_compute_storage::KvEntry {
            value: b"plain".to_vec(),
            metadata_json: None,
            expires_at_ms: None,
        })]),
    )
    .unwrap();
    assert!(without_metadata.ends_with(b"plain"));
    let (_, many) = encode_frame_result(
        Operation::GetMany,
        KvCommandResult::Entries(vec![Some(entry), None]),
    )
    .unwrap();
    assert!(many.starts_with(b"KVB1\x00\x02"));
    for operation in [Operation::Put, Operation::Delete] {
        assert!(
            encode_frame_result(operation, KvCommandResult::Mutation)
                .unwrap()
                .1
                .is_empty()
        );
    }
    let (_, listed) = encode_frame_result(
        Operation::List,
        KvCommandResult::List {
            rows: vec![open_compute_storage::KvListRow {
                key: b"listed".to_vec(),
                metadata_json: Some(br#"{"x":true}"#.to_vec()),
                expires_at_ms: Some(9_000),
            }],
            complete: false,
            cursor: Some("cursor".to_owned()),
        },
    )
    .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&listed).unwrap();
    assert_eq!(listed["keys"][0]["name"], "listed");
    assert_eq!(listed["keys"][0]["expiration"], 9);
    assert_eq!(listed["cursor"], "cursor");
    for row in [
        open_compute_storage::KvListRow {
            key: vec![0xff],
            metadata_json: None,
            expires_at_ms: None,
        },
        open_compute_storage::KvListRow {
            key: b"valid".to_vec(),
            metadata_json: Some(b"not-json".to_vec()),
            expires_at_ms: None,
        },
    ] {
        assert_eq!(
            encode_frame_result(
                Operation::List,
                KvCommandResult::List {
                    rows: vec![row],
                    complete: true,
                    cursor: None,
                },
            )
            .unwrap_err()
            .code(),
            ErrorCode::KvCorrupt
        );
    }
    assert_eq!(
        encode_frame_result(Operation::List, KvCommandResult::Mutation)
            .unwrap_err()
            .code(),
        ErrorCode::KvInternalProtocolError
    );

    assert_eq!(encode_stream_header(None).unwrap().len(), 21);
    let stream_header = encode_stream_header(Some(open_compute_storage::KvEntryInfo {
        value_length: 5,
        metadata_json: Some(b"null".to_vec()),
        expires_at_ms: None,
    }))
    .unwrap();
    assert!(stream_header.starts_with(b"KVS1\x01"));
    assert!(stream_header.ends_with(&5_u32.to_be_bytes()));
    for (value_length, metadata_json, code) in [
        (
            open_compute_storage::KV_MAX_VALUE_BYTES + 1,
            None,
            ErrorCode::KvValueTooLarge,
        ),
        (
            1,
            Some(vec![b'x'; open_compute_storage::KV_MAX_METADATA_BYTES + 1]),
            ErrorCode::KvMetadataTooLarge,
        ),
    ] {
        assert_eq!(
            encode_stream_header(Some(open_compute_storage::KvEntryInfo {
                value_length,
                metadata_json,
                expires_at_ms: None,
            }))
            .unwrap_err()
            .code(),
            code
        );
    }
}
