use super::*;
use crate::{
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRecord,
    ResourceRepository,
};
use open_compute_core::config::StorageConfig;
use open_compute_core::{BindingKind, ErrorCode, RequestId, ResourceId, SystemClock};
use open_compute_search::compile_filter;
use serde_json::json;
use std::os::unix::fs::symlink;

#[path = "coverage_tests.rs"]
mod coverage_tests;

fn engine(path: &std::path::Path) -> VectorizeEngine {
    VectorizeEngine::open(path, "resource-1", 32, "cosine", 100, 16 * 1024 * 1024, 500).unwrap()
}

fn input(id: &str, values: [f32; 2]) -> VectorMutationInput {
    VectorMutationInput {
        id: id.to_string(),
        namespace: None,
        values: Some(expected(values)),
        metadata: None,
    }
}

fn expected(values: [f32; 2]) -> Vec<f32> {
    values
        .into_iter()
        .chain(std::iter::repeat_n(0.0, 30))
        .collect()
}

fn catalog_fixture() -> (tempfile::TempDir, PlatformStorage) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let storage = PlatformStorage::bootstrap(
        &StorageConfig {
            data_dir: root.clone(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        },
        &SystemClock,
    )
    .unwrap();
    (temporary, storage)
}

fn reserve_index(storage: &PlatformStorage, name: &str) -> ResourceRecord {
    let account_id = storage.identity().default_account_id;
    let resource_id = ResourceId::generate();
    let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
    let reservation = ResourceRepository::new(storage.db())
        .reserve_create(
            &ReserveResourceCreate {
                account_id,
                kind: BindingKind::VectorizeIndex,
                name,
                idempotency_key: name,
                fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                request_fingerprint: &fingerprint,
                resource_id,
                driver_schema_version: 1,
                request_id: RequestId::generate(),
                now_ms: 10,
                expires_at_ms: 1_000,
            },
            100,
        )
        .unwrap();
    let ResourceCreateReservation::Reserved(resource) = reservation else {
        unreachable!()
    };
    resource
}

#[test]
fn vectorize_catalog_round_trips_and_pages_ready_indexes() {
    let (_temporary, storage) = catalog_fixture();
    let repository = VectorizeIndexRepository::new(storage.db());
    let first = reserve_index(&storage, "alpha");
    assert_eq!(
        repository
            .ensure_index(&first, "bad", 1, 31, "cosine", 100, 1_048_576)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let inserted = repository
        .ensure_index(
            &first,
            "vectorize/v1/alpha",
            1,
            32,
            "cosine",
            100,
            1_048_576,
        )
        .unwrap();
    assert_eq!(
        repository.get(first.account_id, first.id).unwrap(),
        inserted
    );
    assert_eq!(repository.list(first.account_id).unwrap(), vec![inserted]);
    assert_eq!(
        repository
            .ensure_index(
                &first,
                "vectorize/v1/alpha",
                1,
                32,
                "dot-product",
                100,
                1_048_576,
            )
            .unwrap_err()
            .code(),
        ErrorCode::ResourceInvariantViolation
    );

    let second = reserve_index(&storage, "beta");
    repository
        .ensure_index(
            &second,
            "vectorize/v1/beta",
            1,
            64,
            "euclidean",
            200,
            2_097_152,
        )
        .unwrap();
    let resources = ResourceRepository::new(storage.db());
    resources.mark_ready(first.id, 20).unwrap();
    resources.mark_ready(second.id, 21).unwrap();
    let ready = repository.ready_indexes(10).unwrap();
    assert_eq!(ready.len(), 2);
    let after = (ready[0].resource.account_id, ready[0].resource.id);
    assert_eq!(
        repository
            .ready_indexes_after(Some(after), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        repository.ready_indexes(0).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn queued_mutation_is_invisible_until_atomic_apply() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    let receipt = engine
        .enqueue(VectorMutationKind::Insert, &[input("a", [1.0, 0.0])], 1)
        .unwrap();
    assert_eq!(receipt.sequence, 1);
    assert!(engine.get_by_ids(&["a".to_string()]).unwrap().is_empty());
    let applied = engine.apply_next(2).unwrap().unwrap();
    assert_eq!(applied.state, VectorMutationState::Applied);
    assert_eq!(
        engine.get_by_ids(&["a".to_string()]).unwrap()[0].values,
        expected([1.0, 0.0])
    );
    let wire = serde_json::to_value(&engine.get_by_ids(&["a".to_string()]).unwrap()[0]).unwrap();
    assert!(wire.get("namespace").is_none());
    assert!(wire.get("metadata").is_none());
    assert_eq!(engine.describe().unwrap().processed_sequence, 1);
}

#[test]
fn engine_rejects_symlink_authority_path() {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target.sqlite");
    std::fs::File::create(&target).unwrap();
    let link = temporary.path().join("data.sqlite");
    symlink(&target, &link).unwrap();
    assert_eq!(
        VectorizeEngine::open(
            &link,
            "resource-1",
            32,
            "cosine",
            100,
            16 * 1024 * 1024,
            500
        )
        .unwrap_err()
        .code(),
        ErrorCode::PathInvalid
    );
}

#[test]
fn engine_rejects_invalid_batches_and_mismatched_reopen_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("data.sqlite");
    assert_eq!(
        VectorizeEngine::open(&path, "resource-1", 31, "cosine", 100, 1_048_576, 500)
            .unwrap_err()
            .code(),
        ErrorCode::BindingProtocolError
    );
    let engine = engine(&path);
    assert_eq!(
        engine
            .enqueue(VectorMutationKind::Upsert, &[], 1)
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
    for invalid in [
        VectorMutationInput {
            id: String::new(),
            namespace: None,
            values: Some(expected([1.0, 0.0])),
            metadata: None,
        },
        VectorMutationInput {
            id: "missing-values".into(),
            namespace: None,
            values: None,
            metadata: None,
        },
        VectorMutationInput {
            id: "bad-metadata".into(),
            namespace: None,
            values: Some(expected([1.0, 0.0])),
            metadata: Some(json!({"nested": {"too": {"deep": true}}})),
        },
    ] {
        assert_eq!(
            engine
                .enqueue(VectorMutationKind::Upsert, &[invalid], 1)
                .unwrap_err()
                .code(),
            ErrorCode::BindingProtocolError
        );
    }
    let mut nan = input("nan", [1.0, 0.0]);
    nan.values.as_mut().unwrap()[0] = f32::NAN;
    assert_eq!(
        engine
            .enqueue(VectorMutationKind::Insert, &[nan], 1)
            .unwrap_err()
            .code(),
        ErrorCode::BindingProtocolError
    );
    engine.quick_check().unwrap();
    engine.checkpoint(false).unwrap();
    drop(engine);
    assert_eq!(
        VectorizeEngine::open(
            &path,
            "resource-2",
            32,
            "cosine",
            100,
            16 * 1024 * 1024,
            500
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

fn assert_persisted_mutation_rejected(
    root: &std::path::Path,
    name: &str,
    tamper: impl FnOnce(&rusqlite::Connection, &str),
) {
    let path = root.join(format!("{name}.sqlite"));
    let engine = engine(&path);
    let receipt = engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[input("target", [1.0, 0.0])],
            1,
        )
        .unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    tamper(&connection, &receipt.mutation_id);
    drop(connection);
    assert_eq!(
        engine.apply_next(2).unwrap_err().code(),
        ErrorCode::ResourceInvariantViolation
    );
    assert_eq!(
        engine.claim_next("retry", 3, 100).unwrap_err().code(),
        ErrorCode::ResourceUnavailable
    );
    assert_eq!(
        engine
            .enqueue(VectorMutationKind::Upsert, &[input("later", [0.0, 1.0])], 4)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceUnavailable
    );
}

#[test]
fn persisted_mutation_validation_fails_closed_and_blocks_the_frontier() {
    let temporary = tempfile::tempdir().unwrap();
    assert_persisted_mutation_rejected(temporary.path(), "empty", |connection, mutation| {
        connection
            .execute(
                "DELETE FROM vector_mutation_items WHERE mutation_id=?1",
                [mutation],
            )
            .unwrap();
    });
    assert_persisted_mutation_rejected(temporary.path(), "namespace", |connection, mutation| {
        connection
            .execute(
                "UPDATE vector_mutation_items SET namespace='' WHERE mutation_id=?1",
                [mutation],
            )
            .unwrap();
    });
    assert_persisted_mutation_rejected(temporary.path(), "identity", |connection, mutation| {
        connection
            .execute(
                "UPDATE vector_mutation_items SET vector_id='' WHERE mutation_id=?1",
                [mutation],
            )
            .unwrap();
    });
    assert_persisted_mutation_rejected(temporary.path(), "values", |connection, mutation| {
        connection
            .execute(
                "UPDATE vector_mutation_items SET values_f32le=NULL WHERE mutation_id=?1",
                [mutation],
            )
            .unwrap();
    });
    assert_persisted_mutation_rejected(temporary.path(), "metadata", |connection, mutation| {
        connection
            .execute(
                "UPDATE vector_mutation_items SET metadata_json=?1 WHERE mutation_id=?2",
                rusqlite::params![br#"{"a": 1}"#, mutation],
            )
            .unwrap();
    });
    assert_persisted_mutation_rejected(
        temporary.path(),
        "invalid-metadata",
        |connection, mutation| {
            connection
                .execute(
                    "UPDATE vector_mutation_items SET metadata_json=X'FF' WHERE mutation_id=?1",
                    [mutation],
                )
                .unwrap();
        },
    );
    assert_persisted_mutation_rejected(temporary.path(), "nonfinite", |connection, mutation| {
        let mut encoded = vec![0_u8; 32 * 4];
        encoded[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        connection
            .execute(
                "UPDATE vector_mutation_items SET values_f32le=?1 WHERE mutation_id=?2",
                rusqlite::params![encoded, mutation],
            )
            .unwrap();
    });
}

#[test]
fn insert_existing_id_preserves_old_record_and_advances_frontier() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    engine
        .enqueue(VectorMutationKind::Insert, &[input("a", [1.0, 0.0])], 1)
        .unwrap();
    engine.apply_next(2).unwrap();
    engine
        .enqueue(VectorMutationKind::Insert, &[input("a", [0.0, 1.0])], 3)
        .unwrap();
    assert_eq!(
        engine.apply_next(4).unwrap().unwrap().state,
        VectorMutationState::Applied
    );
    assert_eq!(
        engine.get_by_ids(&["a".to_string()]).unwrap()[0].values,
        expected([1.0, 0.0])
    );
    assert_eq!(engine.describe().unwrap().processed_sequence, 2);
}

#[test]
fn enqueue_reserves_quota_and_applied_payload_is_pruned() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("data.sqlite");
    let engine =
        VectorizeEngine::open(&path, "resource-1", 32, "cosine", 1, 1_048_576, 500).unwrap();
    engine
        .enqueue(VectorMutationKind::Upsert, &[input("a", [1.0, 0.0])], 1)
        .unwrap();
    let error = engine
        .enqueue(VectorMutationKind::Upsert, &[input("b", [0.0, 1.0])], 2)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BindingLimitExceeded);
    engine.apply_next(3).unwrap();

    let delete = VectorMutationInput {
        id: "a".to_string(),
        namespace: None,
        values: None,
        metadata: None,
    };
    engine
        .enqueue(VectorMutationKind::Delete, &[delete], 4)
        .unwrap();
    engine
        .enqueue(VectorMutationKind::Upsert, &[input("b", [0.0, 1.0])], 5)
        .unwrap();
    engine.apply_next(6).unwrap();
    engine.apply_next(7).unwrap();
    assert_eq!(engine.get_by_ids(&["b".to_string()]).unwrap().len(), 1);

    let connection = rusqlite::Connection::open(&path).unwrap();
    let payload_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM vector_mutation_items", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(payload_rows, 0);
}

#[test]
fn upsert_fully_replaces_and_delete_is_ordered() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    let mut first = input("a", [1.0, 0.0]);
    first.namespace = Some("old".to_string());
    first.metadata = Some(json!({"kind": "old"}));
    engine
        .enqueue(VectorMutationKind::Upsert, &[first], 1)
        .unwrap();
    engine.apply_next(2).unwrap();
    let mut replacement = input("a", [0.0, 1.0]);
    replacement.metadata = Some(json!({"kind": "new"}));
    engine
        .enqueue(VectorMutationKind::Upsert, &[replacement], 3)
        .unwrap();
    engine.apply_next(4).unwrap();
    let record = &engine.get_by_ids(&["a".to_string()]).unwrap()[0];
    assert_eq!(record.namespace, None);
    assert_eq!(record.metadata, Some(json!({"kind": "new"})));
    let delete = VectorMutationInput {
        id: "a".to_string(),
        namespace: None,
        values: None,
        metadata: None,
    };
    engine
        .enqueue(VectorMutationKind::Delete, &[delete], 5)
        .unwrap();
    engine.apply_next(6).unwrap();
    assert!(engine.get_by_ids(&["a".to_string()]).unwrap().is_empty());
}

#[test]
fn duplicate_ids_are_canonicalized_first_wins_before_enqueue() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    let receipt = engine
        .enqueue(
            VectorMutationKind::Insert,
            &[input("same", [1.0, 0.0]), input("same", [0.0, 1.0])],
            1,
        )
        .unwrap();
    assert_eq!(receipt.item_count, 1);
    engine.apply_next(2).unwrap();
    assert_eq!(
        engine.get_by_ids(&["same".to_string()]).unwrap()[0].values,
        expected([1.0, 0.0])
    );

    let receipt = engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[input("same", [0.5, 0.5]), input("same", [-1.0, 0.0])],
            3,
        )
        .unwrap();
    assert_eq!(receipt.item_count, 1);
    engine.apply_next(4).unwrap();
    assert_eq!(
        engine.get_by_ids(&["same".to_string()]).unwrap()[0].values,
        expected([0.5, 0.5])
    );
}

#[test]
fn metadata_index_materializes_terms_and_persisted_blob_is_revalidated() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("data.sqlite");
    let engine = engine(&path);
    let mut item = input("a", [1.0, 0.0]);
    item.metadata = Some(json!({"nested": {"year": 2026}, "tags": ["rust", "sqlite"]}));
    let mut old = input("old", [0.5, 0.5]);
    old.metadata = Some(json!({"nested": {"year": 2010}}));
    let mut missing = input("missing", [-1.0, 0.0]);
    missing.metadata = Some(json!({"topic": "unclassified"}));
    engine
        .enqueue(VectorMutationKind::Upsert, &[item, old, missing], 1)
        .unwrap();
    engine.apply_next(2).unwrap();
    engine
        .create_metadata_index("nested.year", "number", 3)
        .unwrap();
    assert_eq!(
        engine
            .indexed_properties()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["nested.year"]
    );
    let filter = compile_filter(
        &json!({"nested.year": {"$gte": 2020, "$lt": 2030}}),
        &engine.indexed_properties().unwrap(),
    )
    .unwrap();
    let mut candidates = Vec::new();
    assert_eq!(
        engine
            .scan_candidates(None, Some(&filter), |record| {
                candidates.push(record.id);
                Ok(())
            })
            .unwrap(),
        1
    );
    assert_eq!(candidates, ["a"]);
    let negative = compile_filter(
        &json!({"nested.year": {"$ne": 2026}}),
        &engine.indexed_properties().unwrap(),
    )
    .unwrap();
    let mut candidates = Vec::new();
    assert_eq!(
        engine
            .scan_candidates(None, Some(&negative), |record| {
                candidates.push(record.id);
                Ok(())
            })
            .unwrap(),
        2
    );
    assert_eq!(candidates, ["old", "missing"]);

    engine
        .enqueue(VectorMutationKind::Upsert, &[input("b", [0.0, 1.0])], 4)
        .unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE vector_mutation_items SET values_f32le = X'0000' WHERE vector_id = 'b'",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(engine.apply_next(5).is_err());
    assert!(engine.get_by_ids(&["b".to_string()]).unwrap().is_empty());
    assert_eq!(engine.describe().unwrap().processed_sequence, 1);
}

#[test]
fn durable_claim_lease_fences_stale_worker_and_expires() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = engine(&temporary.path().join("data.sqlite"));
    engine
        .enqueue(
            VectorMutationKind::Upsert,
            &[input("leased", [1.0, 0.0])],
            1,
        )
        .unwrap();
    assert_eq!(
        engine
            .claim_next("worker-a", 10, 100)
            .unwrap()
            .unwrap()
            .state,
        VectorMutationState::Claimed
    );
    assert!(engine.frontier_is_claimed(20).unwrap());
    assert!(engine.claim_next("worker-b", 20, 100).unwrap().is_none());
    assert!(engine.apply_claimed("worker-b", 21).is_err());
    assert!(engine.apply_claimed("worker-a", 110).is_err());
    assert!(!engine.frontier_is_claimed(110).unwrap());
    assert_eq!(
        engine
            .claim_next("worker-b", 111, 100)
            .unwrap()
            .unwrap()
            .state,
        VectorMutationState::Claimed
    );
    engine.apply_claimed("worker-b", 112).unwrap();
    assert_eq!(engine.describe().unwrap().processed_sequence, 1);
}

#[test]
fn byte_quota_is_reserved_atomically_before_enqueue() {
    let temporary = tempfile::tempdir().unwrap();
    let engine = VectorizeEngine::open(
        &temporary.path().join("data.sqlite"),
        "resource-1",
        32,
        "cosine",
        200,
        1_048_576,
        500,
    )
    .unwrap();
    let metadata = json!({"text": "x".repeat(10_000)});
    let items = (0..110)
        .map(|ordinal| VectorMutationInput {
            id: format!("vector-{ordinal}"),
            namespace: None,
            values: Some(expected([1.0, 0.0])),
            metadata: Some(metadata.clone()),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        engine
            .enqueue(VectorMutationKind::Upsert, &items, 1)
            .unwrap_err()
            .code(),
        ErrorCode::BindingLimitExceeded
    );
    assert_eq!(engine.describe().unwrap().vector_count, 0);
    assert!(
        engine
            .get_by_ids(&["vector-0".to_string()])
            .unwrap()
            .is_empty()
    );
    engine
        .enqueue(VectorMutationKind::Upsert, &[input("small", [1.0, 0.0])], 2)
        .unwrap();
    engine.apply_next(3).unwrap();
    assert_eq!(engine.describe().unwrap().vector_count, 1);
}

#[test]
fn read_snapshot_keeps_scan_and_materialization_on_one_generation() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("data.sqlite");
    let reader = engine(&path);
    let writer = engine(&path);
    let mut stable = input("stable", [1.0, 0.0]);
    stable.metadata = Some(json!({"generation": "old"}));
    let mut removed = input("removed", [0.5, 0.5]);
    removed.metadata = Some(json!({"generation": "old"}));
    reader
        .enqueue(VectorMutationKind::Upsert, &[stable, removed], 1)
        .unwrap();
    reader.apply_next(2).unwrap();

    let (scan_complete_tx, scan_complete_rx) = std::sync::mpsc::sync_channel(0);
    let (mutation_complete_tx, mutation_complete_rx) = std::sync::mpsc::sync_channel(0);
    let writer_thread = std::thread::spawn(move || {
        scan_complete_rx.recv().unwrap();
        let mut replacement = input("stable", [0.0, 1.0]);
        replacement.metadata = Some(json!({"generation": "new"}));
        writer
            .enqueue(VectorMutationKind::Upsert, &[replacement], 3)
            .unwrap();
        writer.apply_next(4).unwrap();
        writer
            .enqueue(
                VectorMutationKind::Delete,
                &[VectorMutationInput {
                    id: "removed".to_string(),
                    namespace: None,
                    values: None,
                    metadata: None,
                }],
                5,
            )
            .unwrap();
        writer.apply_next(6).unwrap();
        mutation_complete_tx.send(()).unwrap();
    });

    let records = reader
        .with_read_snapshot(|snapshot| {
            let mut selected_ids = Vec::new();
            snapshot.scan_candidates(None, None, |record| {
                selected_ids.push(record.id);
                Ok(())
            })?;
            scan_complete_tx.send(()).unwrap();
            mutation_complete_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            snapshot.get_by_ids(&selected_ids)
        })
        .unwrap();
    writer_thread.join().unwrap();

    assert_eq!(records.len(), 2);
    let stable = records.iter().find(|record| record.id == "stable").unwrap();
    assert_eq!(stable.values, expected([1.0, 0.0]));
    assert_eq!(stable.metadata, Some(json!({"generation": "old"})));
    assert!(records.iter().any(|record| record.id == "removed"));

    let current = reader
        .get_by_ids(&["stable".to_string(), "removed".to_string()])
        .unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].values, expected([0.0, 1.0]));
    assert_eq!(current[0].metadata, Some(json!({"generation": "new"})));
}
