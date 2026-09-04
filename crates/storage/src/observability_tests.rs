use crate::observability::*;
use open_compute_core::ErrorCode;
use serde_json::Value;
use tempfile::TempDir;

fn invocation(id: &str, timestamp_ms: i64) -> NewObservabilityInvocation {
    NewObservabilityInvocation {
        invocation_id: id.to_owned(),
        account_id: "account".to_owned(),
        script_name: "hello".to_owned(),
        version_id: "version".to_owned(),
        deployment_id: None,
        event_timestamp_ms: timestamp_ms,
        received_at_ms: timestamp_ms,
        event_type: "fetch".to_owned(),
        outcome: "ok".to_owned(),
        cpu_time_ms: 1.0,
        wall_time_ms: 2.0,
        truncated: false,
        event: serde_json::json!({"outcome":"ok"}),
        events: vec![NewObservabilityEvent {
            event_id: format!("{id}:0"),
            sequence: 0,
            timestamp_ms,
            metadata_type: "cf-worker-log".to_owned(),
            level: Some("log".to_owned()),
            source: serde_json::json!({"invoice": 7, "paid": true}),
            metadata: serde_json::json!({"service":"hello"}),
            fields: vec![
                ObservabilityField {
                    key: "source.invoice".to_owned(),
                    value: serde_json::json!(7),
                },
                ObservabilityField {
                    key: "source.paid".to_owned(),
                    value: serde_json::json!(true),
                },
                ObservabilityField {
                    key: "source.customer".to_owned(),
                    value: serde_json::json!("acme"),
                },
            ],
        }],
    }
}

#[test]
fn inserts_idempotently_and_queries_bounded_public_rows() {
    let dir = TempDir::new().unwrap();
    let store = ObservabilityStore::open(
        &dir.path().join("observability.sqlite"),
        100,
        10_000,
        1_000_000,
    )
    .unwrap();
    let value = invocation("invocation-a", 50_000);
    assert!(store.insert(&value).unwrap());
    assert!(!store.insert(&value).unwrap());
    let batch = [
        invocation("invocation-batch-1", 50_001),
        invocation("invocation-batch-2", 50_002),
    ];
    assert_eq!(store.insert_batch(&batch).unwrap(), 2);
    assert_eq!(store.insert_batch(&batch).unwrap(), 0);
    assert!(store.accounted_bytes().unwrap() > 0);
    let events = store
        .query_events("account", 40_000, 60_000, None, None, 10)
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].source,
        serde_json::json!({"invoice":7,"paid":true})
    );
}

#[test]
fn discovers_typed_keys_values_and_prunes_retention() {
    let dir = TempDir::new().unwrap();
    let store = ObservabilityStore::open(
        &dir.path().join("observability.sqlite"),
        100,
        1_000,
        1_000_000,
    )
    .unwrap();
    assert!(store.insert(&invocation("invocation-b", 50_000)).unwrap());
    let keys = store.keys("account", 40_000, 60_000, 10).unwrap();
    assert_eq!(keys.len(), 3);
    let values = store
        .values("account", "source.invoice", "number", 40_000, 60_000, 10)
        .unwrap();
    assert_eq!(values[0].value, serde_json::json!(7.0));
    assert_eq!(
        store
            .values("account", "source.customer", "string", 40_000, 60_000, 10,)
            .unwrap()[0]
            .value,
        serde_json::json!("acme")
    );
    assert_eq!(
        store
            .values("account", "source.paid", "boolean", 40_000, 60_000, 10,)
            .unwrap()[0]
            .value,
        serde_json::json!(true)
    );
    assert_eq!(store.oldest_event_ms().unwrap(), Some(50_000));
    assert_eq!(store.prune(52_000, 10).unwrap(), 1);
    assert_eq!(store.oldest_event_ms().unwrap(), None);
}

#[test]
fn rejects_invalid_shapes_and_hard_quota() {
    let dir = TempDir::new().unwrap();
    let store = ObservabilityStore::open(
        &dir.path().join("observability.sqlite"),
        100,
        10_000,
        1024 * 1024,
    )
    .unwrap();
    let mut oversized = invocation("invocation-c", 50_000);
    oversized.events[0].source = Value::String("x".repeat(1024 * 1024));
    assert_eq!(
        store.insert(&oversized).unwrap_err().code(),
        ErrorCode::QuotaExceeded
    );
    let mut invalid = invocation("invocation-d", 50_000);
    invalid.events[0].sequence = 1;
    assert_eq!(
        store.insert(&invalid).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );

    for mut invalid in [
        {
            let mut value = invocation("invalid-empty-id", 50_000);
            value.invocation_id.clear();
            value
        },
        {
            let mut value = invocation("invalid-account", 50_000);
            value.account_id.clear();
            value
        },
        {
            let mut value = invocation("invalid-script", 50_000);
            value.script_name = "x".repeat(64);
            value
        },
        {
            let mut value = invocation("invalid-version", 50_000);
            value.version_id.clear();
            value
        },
        {
            let mut value = invocation("invalid-event-type", 50_000);
            value.event_type.clear();
            value
        },
        {
            let mut value = invocation("invalid-outcome", 50_000);
            value.outcome.clear();
            value
        },
        {
            let mut value = invocation("invalid-cpu", 50_000);
            value.cpu_time_ms = -1.0;
            value
        },
        {
            let mut value = invocation("invalid-wall", 50_000);
            value.wall_time_ms = f64::INFINITY;
            value
        },
        {
            let mut value = invocation("invalid-event-id", 50_000);
            value.events[0].event_id.clear();
            value
        },
        {
            let mut value = invocation("invalid-event-id-long", 50_000);
            value.events[0].event_id = "x".repeat(161);
            value
        },
        {
            let mut value = invocation("invalid-type", 50_000);
            value.events[0].metadata_type = "private".to_owned();
            value
        },
        {
            let mut value = invocation("invalid-field-count", 50_000);
            value.events[0].fields = vec![
                ObservabilityField {
                    key: "key".to_owned(),
                    value: serde_json::json!(true),
                };
                257
            ];
            value
        },
    ] {
        invalid.deployment_id = Some("deployment".to_owned());
        assert_eq!(
            store.insert(&invalid).unwrap_err().code(),
            ErrorCode::LimitInvalid
        );
    }

    for field in [
        ObservabilityField {
            key: String::new(),
            value: serde_json::json!(true),
        },
        ObservabilityField {
            key: "x".repeat(513),
            value: serde_json::json!(true),
        },
        ObservabilityField {
            key: "field".to_owned(),
            value: serde_json::json!("x".repeat(16_385)),
        },
        ObservabilityField {
            key: "field".to_owned(),
            value: Value::Null,
        },
        ObservabilityField {
            key: "field".to_owned(),
            value: serde_json::json!([]),
        },
    ] {
        let mut invalid = invocation(&format!("invalid-field-{}", field.key.len()), 50_000);
        invalid.events[0].fields = vec![field];
        assert_eq!(
            store.insert(&invalid).unwrap_err().code(),
            ErrorCode::LimitInvalid
        );
    }

    assert_eq!(
        store.insert_batch(&[]).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        store
            .insert_batch(&vec![invocation("too-many", 50_000); 4_097])
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );

    for result in [
        store.query_events("", 1, 2, None, None, 1).map(|_| ()),
        store
            .query_events("account", 2, 2, None, None, 1)
            .map(|_| ()),
        store
            .query_events("account", 1, 2, None, None, 0)
            .map(|_| ()),
        store.keys("", 1, 2, 1).map(|_| ()),
        store.keys("account", 2, 1, 1).map(|_| ()),
        store.values("account", "", "string", 1, 2, 1).map(|_| ()),
        store
            .values("account", "field", "object", 1, 2, 1)
            .map(|_| ()),
        store.prune(1, 0).map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err().code(), ErrorCode::LimitInvalid);
    }
}

#[test]
fn hard_quota_evicts_oldest_log_without_affecting_new_ingest() {
    let dir = TempDir::new().unwrap();
    let store = ObservabilityStore::open(
        &dir.path().join("observability.sqlite"),
        100,
        10_000,
        1024 * 1024,
    )
    .unwrap();
    let mut first = invocation("invocation-old", 50_000);
    first.events[0].source = Value::String("a".repeat(540_000));
    let mut second = invocation("invocation-new", 50_001);
    second.events[0].source = Value::String("b".repeat(540_000));
    assert!(store.insert(&first).unwrap());
    assert!(store.insert(&second).unwrap());
    let rows = store
        .query_events("account", 40_000, 60_000, None, None, 10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_id, "invocation-new:0");
}

#[test]
fn rejects_a_cursor_after_its_retention_anchor_is_removed() {
    let dir = TempDir::new().unwrap();
    let store = ObservabilityStore::open(
        &dir.path().join("observability.sqlite"),
        100,
        1_000,
        1_000_000,
    )
    .unwrap();
    assert!(
        store
            .insert(&invocation("invocation-anchor", 50_000))
            .unwrap()
    );
    let cursor = ObservabilityEventCursor {
        timestamp_ms: 50_000,
        event_id: "invocation-anchor:0".to_owned(),
    };
    assert_eq!(store.prune(52_000, 10).unwrap(), 1);
    assert_eq!(
        store
            .query_events("account", 40_000, 60_000, None, Some(&cursor), 10)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNotFound
    );
}

#[test]
fn rejects_a_database_with_a_mismatched_schema_checksum() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("observability.sqlite");
    drop(ObservabilityStore::open(&path, 100, 10_000, 1_000_000).unwrap());
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE observability_meta SET value='tampered' WHERE key='schema_sha256'",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        ObservabilityStore::open(&path, 100, 10_000, 1_000_000)
            .unwrap_err()
            .code(),
        ErrorCode::PlatformUnavailable
    );
}

#[test]
fn rejects_invalid_limits_schema_versions_and_data_formats() {
    let dir = TempDir::new().unwrap();
    assert_eq!(
        ObservabilityStore::open(&dir.path().join("too-small.sqlite"), 100, 10_000, 1)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    assert_eq!(
        ObservabilityStore::open(
            &dir.path().join("retention.sqlite"),
            100,
            u64::MAX,
            1_000_000,
        )
        .unwrap_err()
        .code(),
        ErrorCode::LimitInvalid
    );

    for (name, sql) in [
        ("version.sqlite", "PRAGMA user_version=2"),
        (
            "format.sqlite",
            "UPDATE observability_meta SET value='obsolete' WHERE key='data_format'",
        ),
    ] {
        let path = dir.path().join(name);
        drop(ObservabilityStore::open(&path, 100, 10_000, 1_000_000).unwrap());
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute(sql, []).unwrap();
        drop(connection);
        assert_eq!(
            ObservabilityStore::open(&path, 100, 10_000, 1_000_000)
                .unwrap_err()
                .code(),
            ErrorCode::PlatformUnavailable
        );
    }
}
