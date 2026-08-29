use super::*;
use crate::binding_backend::KvBindingExecutor;
use open_compute_core::config::StorageConfig;
use open_compute_core::{
    BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DeploymentId,
    DeterministicClock, RequestId, ResourceState, SystemClock,
};
use open_compute_storage::{
    AuthorizedBinding, DeploymentBindingRecord, KvNamespaceRepository, PlatformStorage,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, KvResourceDriver, ResourceController,
    ResourcePins,
};
use std::time::{Duration, UNIX_EPOCH};

fn fixture() -> (
    tempfile::TempDir,
    Arc<PlatformStorage>,
    AuthorizedBinding,
    Arc<DeterministicClock>,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let config = StorageConfig {
        data_dir: root.clone(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    };
    let storage = Arc::new(PlatformStorage::bootstrap(&config, &SystemClock).unwrap());
    let account = storage.identity().default_account_id;
    let result = ResourceController::new(
        &storage,
        ResourcePins::new(),
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::KvNamespace,
        name: "cache".to_owned(),
        idempotency_key: "create-cache".to_owned(),
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms: 1_000,
    })
    .unwrap();
    let resource_id = match result {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => unreachable!(),
    };
    let resource = KvNamespaceRepository::new(storage.db())
        .get(account, resource_id)
        .unwrap()
        .resource;
    assert_eq!(resource.state, ResourceState::Ready);
    let binding = AuthorizedBinding {
        binding: DeploymentBindingRecord {
            id: BindingId::generate(),
            deployment_id: DeploymentId::generate(),
            name: "CACHE".to_owned(),
            kind: BindingKind::KvNamespace,
            resource_id,
            resource_spec_generation: resource.spec_generation,
            capability_version: 1,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
            descriptor_sha256: [7; 32],
            created_at_ms: 1_000,
        },
        resource,
        account_id: account,
    };
    let clock = Arc::new(DeterministicClock::new(
        UNIX_EPOCH + Duration::from_secs(10),
    ));
    (temp, storage, binding, clock)
}

#[test]
fn concrete_executor_covers_types_metadata_ttl_and_signed_cursor_scope() {
    let (_temp, storage, binding, clock) = fixture();
    let metrics = Arc::new(
        MetricsRegistry::new(
            &open_compute_core::MetricsConfig::default(),
            "test",
            "workerd",
        )
        .unwrap(),
    );
    let executor =
        SqliteKvBindingExecutor::new(storage, clock.clone()).with_metrics(metrics.clone());
    executor
        .execute(
            &binding,
            KvCommand::Put {
                key: "a".to_owned(),
                value: br#"{"ok":true}"#.to_vec(),
                expiration: None,
                expiration_ttl: Some(60),
                metadata: Some(serde_json::json!({"z": 1, "a": 2})),
                metadata_present: true,
            },
        )
        .unwrap();
    executor
        .execute(
            &binding,
            KvCommand::Put {
                key: "b".to_owned(),
                value: b"second".to_vec(),
                expiration: None,
                expiration_ttl: None,
                metadata: None,
                metadata_present: false,
            },
        )
        .unwrap();
    let KvCommandResult::Entries(entries) = executor
        .execute(
            &binding,
            KvCommand::Get {
                keys: vec!["a".to_owned(), "missing".to_owned()],
                cache_ttl: Some(30),
            },
        )
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(entries[0].as_ref().unwrap().value, br#"{"ok":true}"#);
    assert_eq!(
        entries[0].as_ref().unwrap().metadata_json.as_deref(),
        Some(br#"{"a":2,"z":1}"#.as_slice())
    );
    assert!(entries[1].is_none());
    let mut streamed = Vec::new();
    executor
        .stream_get(&binding, "b", Some(30), &mut |part| {
            let active = metrics.render(&open_compute_core::PlatformStatus::starting());
            assert!(active.contains("kv_open_connections{role=\"reader\"} 1"));
            assert!(active.contains("kv_active_streams 1"));
            streamed.push(part);
            Ok(())
        })
        .unwrap();
    let idle = metrics.render(&open_compute_core::PlatformStatus::starting());
    assert!(idle.contains("kv_open_connections{role=\"reader\"} 0"));
    assert!(idle.contains("kv_active_streams 0"));
    assert!(matches!(
        &streamed[0],
        KvStreamPart::Entry(Some(info)) if info.value_length == 6
    ));
    assert_eq!(
        streamed
            .iter()
            .filter_map(|part| match part {
                KvStreamPart::Bytes(bytes) => Some(bytes.as_slice()),
                KvStreamPart::Entry(_) => None,
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>(),
        b"second"
    );
    let cancelled = executor.stream_get(&binding, "b", None, &mut |part| match part {
        KvStreamPart::Entry(_) => Ok(()),
        KvStreamPart::Bytes(_) => Err(PlatformError::new(
            ErrorCode::KvUnavailable,
            "test cancellation",
        )),
    });
    assert_eq!(cancelled.unwrap_err().code(), ErrorCode::KvUnavailable);

    let cursor = match executor
        .execute(
            &binding,
            KvCommand::List {
                prefix: "".to_owned(),
                limit: 1,
                cursor: None,
            },
        )
        .unwrap()
    {
        KvCommandResult::List {
            complete,
            cursor,
            rows,
        } => {
            assert!(!complete);
            assert_eq!(rows[0].key, b"a");
            cursor.unwrap()
        }
        _ => unreachable!(),
    };
    match executor
        .execute(
            &binding,
            KvCommand::List {
                prefix: "".to_owned(),
                limit: 1,
                cursor: Some(cursor.clone()),
            },
        )
        .unwrap()
    {
        KvCommandResult::List { complete, rows, .. } => {
            assert!(complete);
            assert_eq!(rows[0].key, b"b");
        }
        _ => unreachable!(),
    }
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::List {
                    prefix: "a".to_owned(),
                    limit: 1,
                    cursor: Some(cursor),
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );

    clock.advance(Duration::from_secs(61));
    let expired = executor
        .execute(
            &binding,
            KvCommand::Get {
                keys: vec!["a".to_owned()],
                cache_ttl: None,
            },
        )
        .unwrap();
    assert!(matches!(expired, KvCommandResult::Entries(values) if values == vec![None]));
}

#[test]
fn concrete_executor_rejects_option_boundaries_without_replay() {
    let (_temp, storage, binding, clock) = fixture();
    let executor = SqliteKvBindingExecutor::new(storage.clone(), clock);
    let both = executor
        .execute(
            &binding,
            KvCommand::Put {
                key: "x".to_owned(),
                value: Vec::new(),
                expiration: Some(1000),
                expiration_ttl: Some(60),
                metadata: None,
                metadata_present: false,
            },
        )
        .unwrap_err();
    assert_eq!(both.code(), ErrorCode::KvInvalidOptions);
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::Get {
                    keys: vec!["x".to_owned()],
                    cache_ttl: Some(29),
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvInvalidOptions
    );
    assert_eq!(
        executor
            .stream_get(&binding, "x", Some(29), &mut |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::KvInvalidOptions
    );
    assert_eq!(
        ensure_storage_headroom(&storage, usize::MAX)
            .unwrap_err()
            .code(),
        ErrorCode::KvStorageFull
    );
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::List {
                    prefix: "".to_owned(),
                    limit: 0,
                    cursor: None,
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvInvalidOptions
    );
}

#[test]
fn connection_gate_handle_lru_generation_and_corruption_fail_closed() {
    let (_temp, storage, binding, clock) = fixture();
    let executor =
        SqliteKvBindingExecutor::with_connection_limit(storage.clone(), clock.clone(), 0);
    assert!(format!("{executor:?}").contains("SqliteKvBindingExecutor"));
    assert!(format!("{:?}", executor.connections).contains("limit"));
    let permit = executor.connections.acquire(Duration::ZERO).unwrap();
    let Err(saturated) = executor.connections.acquire(Duration::ZERO) else {
        panic!("connection limit was not enforced")
    };
    assert_eq!(saturated.code(), ErrorCode::KvBusy);
    drop(permit);
    drop(executor.connections.acquire(Duration::ZERO).unwrap());

    let (first, _) = executor.open_handle(&binding).unwrap();
    let (second, _) = executor.open_handle(&binding).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let retired = Arc::downgrade(&first);
    drop(first);
    drop(second);
    clock.advance(Duration::from_secs(61));
    let (replacement, _) = executor.open_handle(&binding).unwrap();
    assert!(retired.upgrade().is_none());
    assert_eq!(Arc::strong_count(&replacement), 2);

    let created = ResourceController::new(
        &storage,
        ResourcePins::new(),
        KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .create(&CreateResourceRequest {
        account_id: binding.account_id,
        kind: BindingKind::KvNamespace,
        name: "second".to_owned(),
        idempotency_key: "create-second".to_owned(),
        driver_schema_version: 1,
        request_id: RequestId::generate(),
        now_ms: 2_000,
    })
    .unwrap();
    let second_id = match created {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => unreachable!(),
    };
    let second_resource = KvNamespaceRepository::new(storage.db())
        .get(binding.account_id, second_id)
        .unwrap()
        .resource;
    let mut second_binding = binding.clone();
    second_binding.binding.resource_id = second_id;
    second_binding.binding.resource_spec_generation = second_resource.spec_generation;
    second_binding.resource = second_resource;
    assert_eq!(
        executor.open_handle(&second_binding).unwrap_err().code(),
        ErrorCode::KvBusy
    );
    drop(replacement);
    let (second_handle, _) = executor.open_handle(&second_binding).unwrap();
    assert_eq!(Arc::strong_count(&second_handle), 2);
    drop(second_handle);

    let mut stale = binding.clone();
    stale.binding.resource_spec_generation += 1;
    assert_eq!(
        executor
            .execute(
                &stale,
                KvCommand::Get {
                    keys: vec!["x".to_owned()],
                    cache_ttl: None,
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::BindingTypeMismatch
    );

    let record = KvNamespaceRepository::new(storage.db())
        .get(binding.account_id, binding.resource.id)
        .unwrap();
    let path = KvPaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, binding.account_id, binding.resource.id)
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "UPDATE kv_meta SET value = ?1 WHERE key = 'resource_id'",
        [b"wrong".as_slice()],
    )
    .unwrap();
    drop(conn);
    clock.advance(Duration::from_secs(61));
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::Get {
                    keys: vec!["x".to_owned()],
                    cache_ttl: None,
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvCorrupt
    );
    let isolated = ResourceRepository::new(storage.db())
        .get(binding.account_id, binding.resource.id)
        .unwrap();
    assert_eq!(isolated.availability, ResourceAvailability::Unavailable);
    assert_eq!(isolated.availability_code.as_deref(), Some("KV_CORRUPT"));
}

#[test]
fn absolute_expiration_cursor_and_binary_values_are_stable() {
    let (_temp, storage, binding, clock) = fixture();
    let executor = SqliteKvBindingExecutor::new(storage.clone(), clock.clone());
    executor
        .execute(
            &binding,
            KvCommand::Put {
                key: "binary".to_owned(),
                value: vec![0xff],
                expiration: Some(70),
                expiration_ttl: None,
                metadata: Some(Value::Null),
                metadata_present: true,
            },
        )
        .unwrap();
    let KvCommandResult::Entries(entries) = executor
        .execute(
            &binding,
            KvCommand::Get {
                keys: vec!["binary".to_owned()],
                cache_ttl: None,
            },
        )
        .unwrap()
    else {
        panic!("expected binary result")
    };
    assert_eq!(entries[0].as_ref().unwrap().value, [0xff]);
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::Put {
                    key: "bad-expiration".to_owned(),
                    value: Vec::new(),
                    expiration: Some(u64::MAX),
                    expiration_ttl: None,
                    metadata: None,
                    metadata_present: false,
                },
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvInvalidOptions
    );

    executor
        .execute(
            &binding,
            KvCommand::Put {
                key: "z".to_owned(),
                value: b"z".to_vec(),
                expiration: None,
                expiration_ttl: None,
                metadata: None,
                metadata_present: false,
            },
        )
        .unwrap();
    let cursor = match executor
        .execute(
            &binding,
            KvCommand::List {
                prefix: "".to_owned(),
                limit: 1,
                cursor: None,
            },
        )
        .unwrap()
    {
        KvCommandResult::List { cursor, .. } => cursor.unwrap(),
        _ => unreachable!(),
    };
    let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .unwrap();
    bytes[0] = 9;
    let invalid = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::List {
                    prefix: "".to_owned(),
                    limit: 1,
                    cursor: Some(invalid),
                }
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );

    for command in [
        KvCommand::Put {
            key: "too-soon".to_owned(),
            value: Vec::new(),
            expiration: Some(69),
            expiration_ttl: None,
            metadata: None,
            metadata_present: false,
        },
        KvCommand::Put {
            key: "short-ttl".to_owned(),
            value: Vec::new(),
            expiration: None,
            expiration_ttl: Some(59),
            metadata: None,
            metadata_present: false,
        },
        KvCommand::Put {
            key: "large-metadata".to_owned(),
            value: Vec::new(),
            expiration: None,
            expiration_ttl: None,
            metadata: Some(serde_json::json!("x".repeat(1024))),
            metadata_present: true,
        },
    ] {
        let expected = if matches!(
            &command,
            KvCommand::Put {
                metadata_present: true,
                ..
            }
        ) {
            ErrorCode::KvMetadataTooLarge
        } else {
            ErrorCode::KvInvalidOptions
        };
        assert_eq!(
            executor.execute(&binding, command).unwrap_err().code(),
            expected
        );
    }
    assert_eq!(
        executor
            .execute(
                &binding,
                KvCommand::Put {
                    key: "too-large".to_owned(),
                    value: vec![0; open_compute_storage::KV_MAX_VALUE_BYTES + 1],
                    expiration: None,
                    expiration_ttl: None,
                    metadata: None,
                    metadata_present: false,
                },
            )
            .unwrap_err()
            .code(),
        ErrorCode::KvValueTooLarge
    );

    assert_eq!(
        executor
            .sign_cursor(&binding, "", &vec![b'x'; usize::from(u16::MAX) + 1], 10_000)
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );
    let scoped = executor.sign_cursor(&binding, "", b"x", 10_000).unwrap();
    let mut bad_signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&scoped)
        .unwrap();
    *bad_signature.last_mut().unwrap() ^= 1;
    let bad_signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bad_signature);
    assert_eq!(
        executor
            .verify_cursor(&binding, "", &bad_signature, 10_000)
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );

    for mutate in [
        |bytes: &mut Vec<u8>| bytes[73] = 9,
        |bytes: &mut Vec<u8>| bytes[74..76].copy_from_slice(&2_u16.to_be_bytes()),
    ] {
        let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&scoped)
            .unwrap();
        mutate(&mut bytes);
        let signature_offset = bytes.len() - CURSOR_SIGNATURE_BYTES;
        let signature = storage.crypto().sign_kv_cursor(&bytes[..signature_offset]);
        bytes[signature_offset..].copy_from_slice(&signature);
        let cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        assert_eq!(
            executor
                .verify_cursor(&binding, "", &cursor, 10_000)
                .unwrap_err()
                .code(),
            ErrorCode::KvCursorInvalid
        );
    }
    let invalid_utf8 = executor.sign_cursor(&binding, "", &[0xff], 10_000).unwrap();
    assert_eq!(
        executor
            .verify_cursor(&binding, "", &invalid_utf8, 10_000)
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );
    let mut other = binding.clone();
    other.resource.id = open_compute_core::ResourceId::generate();
    assert_eq!(
        executor
            .verify_cursor(&other, "", &scoped, 10_000)
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );
    assert_eq!(
        executor
            .verify_cursor(&binding, "", &scoped, 10_000 + CURSOR_TTL_MS + 1)
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );
    let empty_key = executor.sign_cursor(&binding, "", b"", 10_000).unwrap();
    assert_eq!(
        executor
            .verify_cursor(&binding, "", &empty_key, 10_000)
            .unwrap_err()
            .code(),
        ErrorCode::KvCursorInvalid
    );
}
