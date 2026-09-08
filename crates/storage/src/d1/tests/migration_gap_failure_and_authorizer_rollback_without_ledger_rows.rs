use super::*;

#[test]
fn migration_gap_failure_and_authorizer_rollback_without_ledger_rows() {
    let fixture = fixture();
    let migration = |id: u32, name: &str, sql: &str| D1Migration {
        id,
        name: name.to_owned(),
        sha256: Sha256::digest(sql.as_bytes()).into(),
        sql: sql.to_owned(),
    };
    assert_eq!(
        fixture
            .engine
            .apply_migrations(
                &[migration(2, "0002_gap.sql", "CREATE TABLE gap(value)")],
                limits(),
                1,
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1MigrationDrift
    );
    let failing = migration(
        1,
        "0001_failing.sql",
        "CREATE TABLE rolled_back(value); INSERT INTO missing_table VALUES (1);",
    );
    assert_eq!(
        fixture
            .engine
            .apply_migrations(&[failing], limits(), 2)
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    assert!(fixture.engine.migrations().unwrap().is_empty());
    assert_eq!(
        fixture
            .engine
            .query(&statement("SELECT * FROM rolled_back", vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );

    let denied = migration(
        1,
        "0001_denied.sql",
        "ATTACH DATABASE ':memory:' AS escaped",
    );
    assert_eq!(
        fixture
            .engine
            .apply_migrations(&[denied], limits(), 3)
            .unwrap_err()
            .code(),
        ErrorCode::D1AuthorizerDenied
    );
    assert!(fixture.engine.migrations().unwrap().is_empty());
}
