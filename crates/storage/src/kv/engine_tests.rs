use super::*;
use open_compute_core::{AccountId, ErrorCode, ResourceId};
use serde_json::json;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt as _;

struct PausingReader {
    cursor: std::io::Cursor<Vec<u8>>,
    started: Option<std::sync::mpsc::Sender<()>>,
    resume: std::sync::mpsc::Receiver<()>,
}

impl Read for PausingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if let Some(started) = self.started.take() {
            started.send(()).unwrap();
            self.resume.recv().unwrap();
        }
        self.cursor.read(buffer)
    }
}

fn fixture() -> (tempfile::TempDir, KvEngine, AccountId, ResourceId) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), Permissions::from_mode(0o700)).unwrap();
    let account = AccountId::generate();
    let resource = ResourceId::generate();
    let engine = KvEngine::create(
        &dir.path().join("data.sqlite"),
        account,
        resource,
        1_000,
        256 * 1024 * 1024,
    )
    .unwrap();
    (dir, engine, account, resource)
}

#[test]
fn key_metadata_and_option_boundaries_are_authoritative() {
    assert_eq!(
        validate_key("").unwrap_err().code(),
        ErrorCode::KvKeyInvalid
    );
    assert_eq!(
        validate_key(".").unwrap_err().code(),
        ErrorCode::KvKeyInvalid
    );
    assert_eq!(
        validate_key("..").unwrap_err().code(),
        ErrorCode::KvKeyInvalid
    );
    assert_eq!(validate_key(&"x".repeat(512)).unwrap().len(), 512);
    assert_eq!(
        validate_key(&"x".repeat(513)).unwrap_err().code(),
        ErrorCode::KvKeyTooLarge
    );
    assert_eq!(
        canonical_metadata(&json!({"b": 1, "a": [true, null]})).unwrap(),
        br#"{"a":[true,null],"b":1}"#
    );
    let too_large = json!("x".repeat(1024));
    assert_eq!(
        canonical_metadata(&too_large).unwrap_err().code(),
        ErrorCode::KvMetadataTooLarge
    );

    let (_dir, engine, _, _) = fixture();
    assert!(engine.list(&"x".repeat(512), None, 1, 0).is_ok());
    assert_eq!(
        engine
            .list(&"x".repeat(513), None, 1, 0)
            .unwrap_err()
            .code(),
        ErrorCode::KvKeyTooLarge
    );
}

#[test]
fn crud_ttl_multi_get_and_list_are_snapshot_consistent() {
    let (_dir, engine, _, _) = fixture();
    let metadata = canonical_metadata(&json!({"kind": "fixture"})).unwrap();
    let options = KvPutOptions {
        expires_at_ms: Some(70_000),
        metadata_json: Some(metadata.clone()),
    };
    engine.put("a", b"one", &options, 1_000).unwrap();
    engine
        .put("a/\0", b"two", &KvPutOptions::default(), 1_000)
        .unwrap();
    engine
        .put("é", b"three", &KvPutOptions::default(), 1_000)
        .unwrap();
    engine
        .put("e\u{301}", b"four", &KvPutOptions::default(), 1_000)
        .unwrap();

    let entry = engine.get("a", 1_000).unwrap().unwrap();
    assert_eq!(entry.value, b"one");
    assert_eq!(entry.metadata_json, Some(metadata));
    assert!(engine.get("a", 70_000).unwrap().is_none());

    let keys = vec!["é".to_owned(), "missing".to_owned(), "é".to_owned()];
    let many = engine.get_many(&keys, 1_000).unwrap();
    assert_eq!(many.len(), 3);
    assert_eq!(many[0].as_ref().unwrap().value, b"three");
    assert!(many[1].is_none());
    assert_eq!(many[2], many[0]);

    let first = engine.list("", None, 2, 1_000).unwrap();
    assert!(!first.complete);
    assert_eq!(first.rows.len(), 2);
    assert!(first.rows[0].key < first.rows[1].key);
    let second = engine
        .list("", Some(&first.rows[1].key), 1000, 1_000)
        .unwrap();
    assert!(second.complete);
    assert!(second.rows.iter().all(|row| row.key > first.rows[1].key));
    let prefixed = engine.list("a", None, 1000, 1_000).unwrap();
    assert_eq!(prefixed.rows.len(), 2);

    engine.delete("missing").unwrap();
    engine.delete("é").unwrap();
    assert!(engine.get("é", 1_000).unwrap().is_none());
    assert_eq!(engine.gc_expired(70_000, 256).unwrap(), 1);
}

#[test]
fn incremental_blob_is_atomic_and_backup_restores_as_new_identity() {
    let (dir, engine, account, resource) = fixture();
    let old = vec![7_u8; 128 * 1024];
    engine
        .put("blob", &old, &KvPutOptions::default(), 2_000)
        .unwrap();

    let mut short = std::io::Cursor::new(vec![9_u8; 7]);
    let error = engine
        .put_reader("blob", &mut short, 8, &KvPutOptions::default(), 2_001)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::KvInternalProtocolError);
    assert_eq!(engine.get("blob", 2_002).unwrap().unwrap().value, old);

    let backup = dir.path().join("backup.sqlite");
    engine.online_backup(&backup).unwrap();
    let restored_dir = dir.path().join("restored");
    std::fs::create_dir(&restored_dir).unwrap();
    std::fs::set_permissions(&restored_dir, Permissions::from_mode(0o700)).unwrap();
    let new_account = AccountId::generate();
    let new_resource = ResourceId::generate();
    let restored = KvEngine::restore(
        &backup,
        &restored_dir.join("data.sqlite"),
        account,
        resource,
        new_account,
        new_resource,
        "550e8400-e29b-41d4-a716-446655440000",
        3_000,
        256 * 1024 * 1024,
    )
    .unwrap();
    restored.quick_check().unwrap();
    assert_eq!(
        restored.restore_backup_id().unwrap().as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(restored.get("blob", 3_000).unwrap().unwrap().value, old);
    assert_eq!(
        KvEngine::restore(
            &backup,
            &restored_dir.join("data.sqlite"),
            account,
            resource,
            AccountId::generate(),
            ResourceId::generate(),
            "550e8400-e29b-41d4-a716-446655440001",
            3_001,
            256 * 1024 * 1024,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
}

#[test]
fn streamed_read_cancel_releases_snapshot_and_concurrent_put_is_atomic() {
    let (_dir, engine, _, _) = fixture();
    let old = vec![1_u8; 1024 * 1024];
    let new = vec![2_u8; 1024 * 1024];
    engine
        .put("atomic", &old, &KvPutOptions::default(), 1_000)
        .unwrap();

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let writer_engine = engine.clone();
    let writer = std::thread::spawn(move || {
        let mut reader = PausingReader {
            cursor: std::io::Cursor::new(new),
            started: Some(started_tx),
            resume: resume_rx,
        };
        writer_engine
            .put_reader(
                "atomic",
                &mut reader,
                1024 * 1024,
                &KvPutOptions::default(),
                1_001,
            )
            .unwrap();
    });
    started_rx.recv().unwrap();
    assert_eq!(engine.get("atomic", 1_001).unwrap().unwrap().value, old);
    resume_tx.send(()).unwrap();
    writer.join().unwrap();
    let committed = engine.get("atomic", 1_002).unwrap().unwrap().value;
    assert_eq!((committed.len(), committed[0]), (1024 * 1024, 2));

    let mut announced = None;
    let mut chunks = 0;
    let error = engine
        .stream_get(
            "atomic",
            1_003,
            |entry| {
                announced = entry;
                Ok(())
            },
            |_| {
                chunks += 1;
                Err(PlatformError::new(
                    ErrorCode::KvUnavailable,
                    "test cancellation",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::KvUnavailable);
    assert_eq!(announced.unwrap().value_length, 1024 * 1024);
    assert_eq!(chunks, 1);
    engine.delete("atomic").unwrap();
}

#[test]
fn value_multi_and_list_limits_fail_without_mutating_old_value() {
    let (_dir, engine, _, _) = fixture();
    engine
        .put("key", b"old", &KvPutOptions::default(), 1_000)
        .unwrap();
    let boundary = vec![3_u8; KV_MAX_VALUE_BYTES];
    engine
        .put("boundary", &boundary, &KvPutOptions::default(), 1_000)
        .unwrap();
    let read = engine.get("boundary", 1_001).unwrap().unwrap().value;
    assert_eq!(read.len(), KV_MAX_VALUE_BYTES);
    assert_eq!((read[0], read[KV_MAX_VALUE_BYTES - 1]), (3, 3));
    let oversized = vec![0_u8; KV_MAX_VALUE_BYTES + 1];
    assert_eq!(
        engine
            .put("key", &oversized, &KvPutOptions::default(), 1_001)
            .unwrap_err()
            .code(),
        ErrorCode::KvValueTooLarge
    );
    assert_eq!(engine.get("key", 1_002).unwrap().unwrap().value, b"old");
    let keys = vec!["key".to_owned(); KV_MAX_MULTI_GET_KEYS + 1];
    assert_eq!(
        engine.get_many(&keys, 1_000).unwrap_err().code(),
        ErrorCode::KvTooManyKeys
    );
    assert_eq!(
        engine.list("", None, 0, 1_000).unwrap_err().code(),
        ErrorCode::KvInvalidOptions
    );
    assert_eq!(
        engine.gc_expired(1_000, 0).unwrap_err().code(),
        ErrorCode::KvInvalidOptions
    );
    assert!(engine.wal_bytes().is_ok());
    engine.checkpoint(false).unwrap();
    engine.checkpoint(true).unwrap();
    assert_eq!(engine.wal_bytes().unwrap(), 0);
}

#[test]
fn private_validation_and_sqlite_error_classification_matrix_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        KvEngine::create(
            &dir.path().join("invalid.sqlite"),
            AccountId::generate(),
            ResourceId::generate(),
            -1,
            1,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceInvariantViolation
    );
    let (_owned, engine, _, _) = fixture();
    assert_eq!(
        engine
            .put("x", b"x", &KvPutOptions::default(), -1)
            .unwrap_err()
            .code(),
        ErrorCode::KvInvalidOptions
    );
    assert_eq!(
        engine.list("", Some(&[0xff]), 1, 0).unwrap_err().code(),
        ErrorCode::KvCursorInvalid
    );
    assert_eq!(
        validate_stored_metadata(Some(&vec![b'x'; KV_MAX_METADATA_BYTES + 1]))
            .unwrap_err()
            .code(),
        ErrorCode::KvCorrupt
    );
    assert_eq!(
        validate_stored_metadata(Some(b"not-json"))
            .unwrap_err()
            .code(),
        ErrorCode::KvCorrupt
    );
    assert_eq!(
        validate_stored_metadata(Some(br#"{ "a": 1 }"#))
            .unwrap_err()
            .code(),
        ErrorCode::KvCorrupt
    );
    assert_eq!(prefix_successor(b"a\xff"), Some(b"b".to_vec()));
    assert_eq!(prefix_successor(&[0xff]), None);

    for (sqlite_code, expected) in [
        (rusqlite::ffi::SQLITE_CORRUPT, ErrorCode::KvCorrupt),
        (rusqlite::ffi::SQLITE_NOTADB, ErrorCode::KvCorrupt),
        (rusqlite::ffi::SQLITE_BUSY, ErrorCode::KvBusy),
        (rusqlite::ffi::SQLITE_LOCKED, ErrorCode::KvBusy),
        (rusqlite::ffi::SQLITE_FULL, ErrorCode::KvStorageFull),
        (rusqlite::ffi::SQLITE_IOERR, ErrorCode::KvUnavailable),
    ] {
        assert_eq!(
            map_sql(SqlError::SqliteFailure(
                rusqlite::ffi::Error::new(sqlite_code),
                None,
            ))
            .code(),
            expected
        );
    }
    assert_eq!(
        map_sql(SqlError::InvalidQuery).code(),
        ErrorCode::KvUnavailable
    );
    assert_eq!(metadata_invalid().code(), ErrorCode::KvMetadataInvalid);
    assert_eq!(value_too_large().code(), ErrorCode::KvValueTooLarge);
    assert_eq!(response_too_large().code(), ErrorCode::KvResponseTooLarge);
    assert_eq!(cursor_invalid().code(), ErrorCode::KvCursorInvalid);
    assert_eq!(corrupt().code(), ErrorCode::KvCorrupt);
    assert_eq!(storage_unavailable().code(), ErrorCode::KvUnavailable);
    assert_eq!(invariant().code(), ErrorCode::ResourceInvariantViolation);
}
