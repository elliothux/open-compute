use super::*;

#[test]
fn migration_ledger_is_atomic_idempotent_and_detects_drift() {
    let fixture = fixture();
    let sql = "CREATE TABLE migrated(id INTEGER PRIMARY KEY); PRAGMA user_version = 1;";
    let migration = D1Migration {
        id: 1,
        name: "0001_init.sql".to_owned(),
        sha256: Sha256::digest(sql.as_bytes()).into(),
        sql: sql.to_owned(),
    };
    let applied = fixture
        .engine
        .apply_migrations(std::slice::from_ref(&migration), limits(), 101)
        .unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(fixture.engine.session_version().unwrap(), 1);
    assert_eq!(fixture.engine.user_version().unwrap(), 1);
    assert_eq!(
        fixture
            .engine
            .apply_migrations(std::slice::from_ref(&migration), limits(), 202)
            .unwrap(),
        applied
    );
    assert_eq!(fixture.engine.session_version().unwrap(), 1);
    let mut drift = migration;
    drift.sql = "CREATE TABLE different(id INTEGER)".to_owned();
    drift.sha256 = Sha256::digest(drift.sql.as_bytes()).into();
    assert_eq!(
        fixture
            .engine
            .apply_migrations(&[drift], limits(), 303)
            .unwrap_err()
            .code(),
        ErrorCode::D1MigrationDrift,
    );
}
