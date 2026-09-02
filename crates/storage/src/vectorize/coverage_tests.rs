use super::*;

#[test]
fn engine_public_limits_and_empty_frontier_are_stable() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    assert!(engine.apply_next(1).unwrap().is_none());
    assert!(engine.apply_claimed("unused", 1).unwrap().is_none());
    assert_eq!(
        engine.claim_next("", 1, 100).unwrap_err().code(),
        ErrorCode::BindingProtocolError
    );
    engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[input("queued", [1.0, 0.0])],
            2,
        )
        .unwrap();
    assert_eq!(
        engine.claim_next("worker", i64::MAX, 1).unwrap_err().code(),
        ErrorCode::BindingLimitExceeded
    );
    assert_eq!(
        engine.get_by_ids(&[String::new()]).unwrap_err().code(),
        ErrorCode::BindingProtocolError
    );
    assert_eq!(
        engine
            .scan_candidates(Some(""), None, |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::BindingProtocolError
    );
    assert_eq!(
        engine
            .enqueue(
                VectorMutationKind::Delete,
                &[VectorMutationInput {
                    id: "delete".into(),
                    namespace: None,
                    values: None,
                    metadata: Some(json!({"invalid": true})),
                }],
                3,
            )
            .unwrap_err()
            .code(),
        ErrorCode::BindingProtocolError
    );
    for index in 0..10 {
        engine
            .create_metadata_index(&format!("property{index}"), "boolean", 10 + index)
            .unwrap();
    }
    assert_eq!(
        engine
            .create_metadata_index("overflow", "boolean", 20)
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
    assert_eq!(
        engine
            .create_metadata_index("$invalid", "boolean", 21)
            .unwrap_err()
            .code(),
        ErrorCode::BindingProtocolError
    );
}

#[test]
fn metadata_sql_prefilter_covers_all_scalar_and_prefix_branches() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    let long = format!("{}界", "x".repeat(63));
    let mut alpha = input("alpha", [1.0, 0.0]);
    alpha.metadata = Some(json!({"kind": "alpha", "active": true, "score": 5}));
    let mut beta = input("beta", [0.0, 1.0]);
    beta.namespace = Some("scope".into());
    beta.metadata = Some(json!({"kind": "beta", "active": false, "score": 10}));
    let mut long_value = input("long", [0.5, 0.5]);
    long_value.metadata = Some(json!({"kind": long}));
    let mut missing = input("missing", [-1.0, 0.0]);
    missing.metadata = Some(json!({"other": "value"}));
    engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[alpha, beta, long_value, missing],
            1,
        )
        .unwrap();
    engine.apply_next(2).unwrap();
    engine.create_metadata_index("kind", "string", 3).unwrap();
    engine
        .create_metadata_index("active", "boolean", 4)
        .unwrap();
    engine.create_metadata_index("score", "number", 5).unwrap();
    let indexed = engine.indexed_properties().unwrap();
    let long = format!("{}界", "x".repeat(63));

    for (value, expected_ids) in [
        (
            json!({"kind": {"$in": ["alpha", "beta"]}}),
            vec!["alpha", "beta"],
        ),
        (
            json!({"kind": {"$in": ["alpha", null]}}),
            vec!["alpha", "missing"],
        ),
        (
            json!({"kind": {"$nin": ["alpha"]}}),
            vec!["beta", "long", "missing"],
        ),
        (json!({"active": true}), vec!["alpha"]),
        (json!({"score": {"$gt": 6}}), vec!["beta"]),
        (json!({"kind": null}), vec!["missing"]),
        (
            json!({"kind": {"$ne": "alpha"}}),
            vec!["beta", "long", "missing"],
        ),
        (
            json!({"kind": {"$nin": [long.clone()]}}),
            vec!["alpha", "beta", "long", "missing"],
        ),
        (
            json!({"kind": {"$lt": long.clone()}}),
            vec!["alpha", "beta", "long"],
        ),
        (json!({"kind": {"$gt": long}}), vec!["long"]),
        (json!({"kind": {"$lte": "beta"}}), vec!["alpha", "beta"]),
        (json!({"kind": {"$gte": "beta"}}), vec!["beta", "long"]),
    ] {
        let filter = compile_filter(&value, &indexed).unwrap();
        let mut actual = Vec::new();
        engine
            .scan_candidates(None, Some(&filter), |record| {
                actual.push(record.id);
                Ok(())
            })
            .unwrap();
        assert_eq!(actual, expected_ids, "{value}");
    }
    let mut scoped = Vec::new();
    engine
        .scan_candidates(Some("scope"), None, |record| {
            scoped.push(record.id);
            Ok(())
        })
        .unwrap();
    assert_eq!(scoped, ["beta"]);
    assert_eq!(engine.describe().unwrap().metadata_generation, 3);
    assert_eq!(
        engine
            .create_metadata_index("kind", "string", 6)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceNameConflict
    );
}

#[test]
fn apply_detects_vector_and_byte_quota_races_as_permanent_failures() {
    let temporary = tempfile::tempdir().unwrap();
    let vector_path = temporary.path().join("vector-quota.sqlite");
    let vector_engine =
        VectorizeEngine::open(&vector_path, "resource-1", 32, "cosine", 1, 1_048_576, 500).unwrap();
    vector_engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[input("queued", [1.0, 0.0])],
            1,
        )
        .unwrap();
    let connection = rusqlite::Connection::open(&vector_path).unwrap();
    connection
        .execute(
            "INSERT INTO vectors(vector_id, values_f32le, updated_sequence) VALUES ('raced', ?1, 1)",
            [vec![0_u8; 128]],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        vector_engine.apply_next(2).unwrap_err().code(),
        ErrorCode::BindingLimitExceeded
    );

    let byte_path = temporary.path().join("byte-quota.sqlite");
    let byte_engine = VectorizeEngine::open(
        &byte_path,
        "resource-2",
        32,
        "cosine",
        1_000,
        1_048_576,
        500,
    )
    .unwrap();
    byte_engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[input("queued", [1.0, 0.0])],
            1,
        )
        .unwrap();
    let mut connection = rusqlite::Connection::open(&byte_path).unwrap();
    let transaction = connection.transaction().unwrap();
    for ordinal in 0..103 {
        transaction
            .execute(
                "INSERT INTO vectors(vector_id, values_f32le, metadata_json, updated_sequence)
                 VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![
                    format!("raced-{ordinal}"),
                    vec![0_u8; 128],
                    vec![b'x'; 10_240]
                ],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(connection);
    assert_eq!(
        byte_engine.apply_next(2).unwrap_err().code(),
        ErrorCode::BindingLimitExceeded
    );
}

#[test]
fn quick_check_reads_and_pending_projection_reject_durable_corruption() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("corrupt-read.sqlite");
    let reader = engine(&path);
    let mut item = input("stored", [1.0, 0.0]);
    item.metadata = Some(json!({"kind": "valid"}));
    reader
        .enqueue(VectorMutationKind::Upsert, &[item], 1)
        .unwrap();
    reader.apply_next(2).unwrap();
    reader.checkpoint(true).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute("UPDATE index_meta SET vector_count=0", [])
        .unwrap();
    assert_eq!(
        reader.quick_check().unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    connection
        .execute("UPDATE index_meta SET vector_count=1", [])
        .unwrap();
    connection
        .execute("UPDATE vectors SET metadata_json=X'FF'", [])
        .unwrap();
    assert_eq!(
        reader.get_by_ids(&["stored".into()]).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    connection
        .execute("UPDATE vectors SET metadata_json=NULL", [])
        .unwrap();
    let mut nonfinite = vec![0_u8; 128];
    nonfinite[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    connection
        .execute("UPDATE vectors SET values_f32le=?1", [nonfinite])
        .unwrap();
    assert_eq!(
        reader.get_by_ids(&["stored".into()]).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );

    let pending_path = temporary.path().join("pending.sqlite");
    let pending = engine(&pending_path);
    let mut connection = rusqlite::Connection::open(&pending_path).unwrap();
    let transaction = connection.transaction().unwrap();
    for sequence in 1..=1_025 {
        transaction
            .execute(
                "INSERT INTO vector_mutations
                 (mutation_id, sequence, kind, state, next_attempt_at_ms, item_count,
                  payload_bytes, created_at_ms)
                 VALUES (?1, ?2, 'delete', 'queued', 1, 1, 0, 1)",
                rusqlite::params![format!("mutation-{sequence}"), sequence],
            )
            .unwrap();
    }
    transaction
        .execute("UPDATE index_meta SET next_sequence=1026", [])
        .unwrap();
    transaction.commit().unwrap();
    drop(connection);
    assert_eq!(
        pending
            .enqueue(VectorMutationKind::Upsert, &[input("later", [1.0, 0.0])], 2)
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
}

#[test]
fn an_applied_row_ahead_of_the_frontier_is_rejected_as_corrupt() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("data.sqlite");
    let engine = engine(&path);
    engine
        .enqueue(VectorMutationKind::Upsert, &[input("ahead", [1.0, 0.0])], 1)
        .unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE vector_mutations SET state='applied', completed_at_ms=2 WHERE sequence=1",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        engine.claim_next("worker", 3, 100).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
}
