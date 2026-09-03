use super::*;
use crate::ResourceRecord;
use open_compute_core::{BindingKind, D1Config, ResourceAvailability, ResourceState};

fn limits() -> super::super::D1QueryLimits {
    super::super::D1QueryLimits::query(&D1Config::default()).unwrap()
}

fn record(account: AccountId, resource: ResourceId) -> D1DatabaseRecord {
    D1DatabaseRecord {
        resource: ResourceRecord {
            id: resource,
            account_id: account,
            kind: BindingKind::D1Database,
            name: "restore-in-place".to_owned(),
            state: ResourceState::Ready,
            availability: ResourceAvailability::Healthy,
            availability_code: None,
            spec_generation: 1,
            driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
            created_at_ms: 10,
            updated_at_ms: 10,
            deleted_at_ms: None,
        },
        storage_key: "private".to_owned(),
        schema_version: D1_DATABASE_SCHEMA_VERSION,
        quota_bytes: 256 * 1024 * 1024,
        last_opened_at_ms: None,
        last_quick_check_ms: None,
        last_backup_at_ms: None,
        restore_backup_id: None,
    }
}

#[test]
fn backup_and_restore_staging_rejects_existing_missing_and_corrupt_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.sqlite");
    let account = AccountId::generate();
    let resource = ResourceId::generate();
    let engine = D1Engine::create(&source, account, resource, 10, 256 * 1024 * 1024).unwrap();
    let existing = temp.path().join("existing.sqlite");
    std::fs::write(&existing, b"occupied").unwrap();
    assert_eq!(
        engine.online_backup(&existing).unwrap_err().code(),
        ErrorCode::D1InternalProtocolError
    );

    let snapshot = temp.path().join("snapshot.sqlite");
    engine.online_backup(&snapshot).unwrap();
    D1Engine::verify_completed_snapshot(&snapshot, &record(account, resource), 0).unwrap();
    for suffix in ["-wal", "-shm"] {
        assert!(
            !temp
                .path()
                .join(format!("snapshot.sqlite{suffix}"))
                .exists()
        );
    }
    assert_eq!(
        D1Engine::restore_as_new(
            &snapshot,
            &existing,
            account,
            ResourceId::generate(),
            20,
            256 * 1024 * 1024,
        )
        .unwrap_err()
        .code(),
        ErrorCode::D1InternalProtocolError
    );
    assert_eq!(
        D1Engine::restore_as_new(
            &snapshot,
            &temp.path().join("low-quota.sqlite"),
            account,
            ResourceId::generate(),
            20,
            1,
        )
        .unwrap_err()
        .code(),
        ErrorCode::D1InternalProtocolError
    );
    assert_eq!(
        D1Engine::restore_as_new(
            &temp.path().join("missing.sqlite"),
            &temp.path().join("missing-restore.sqlite"),
            account,
            ResourceId::generate(),
            20,
            256 * 1024 * 1024,
        )
        .unwrap_err()
        .code(),
        ErrorCode::D1InternalProtocolError
    );

    let corrupt = temp.path().join("corrupt.sqlite");
    std::fs::write(&corrupt, b"not sqlite").unwrap();
    fs::chmod(&corrupt, 0o600).unwrap();
    let code = D1Engine::restore_as_new(
        &corrupt,
        &temp.path().join("corrupt-restore.sqlite"),
        account,
        ResourceId::generate(),
        20,
        256 * 1024 * 1024,
    )
    .unwrap_err()
    .code();
    assert!(matches!(
        code,
        ErrorCode::D1DatabaseCorrupt | ErrorCode::ResourceUnavailable
    ));
}

#[test]
fn in_place_restore_retains_identity_and_advances_from_the_previous_head() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.sqlite");
    let account = AccountId::generate();
    let resource = ResourceId::generate();
    let engine = D1Engine::create(&source, account, resource, 10, 256 * 1024 * 1024).unwrap();
    engine
        .exec(
            "CREATE TABLE history(value TEXT); INSERT INTO history VALUES ('old')",
            limits(),
        )
        .unwrap();
    let snapshot = temp.path().join("snapshot.sqlite");
    engine.online_backup(&snapshot).unwrap();
    engine
        .exec("INSERT INTO history VALUES ('new')", limits())
        .unwrap();
    assert_eq!(engine.session_version().unwrap(), 2);

    engine
        .restore_in_place(&snapshot, &record(account, resource), 1, 3)
        .unwrap();
    assert_eq!(engine.session_version().unwrap(), 3);
    engine.verify_identity().unwrap();
    let connection = engine.open().unwrap();
    let values = connection
        .prepare("SELECT value FROM history ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(values, vec!["old"]);
}
