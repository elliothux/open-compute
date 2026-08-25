use super::*;

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
