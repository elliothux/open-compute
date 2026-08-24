use super::*;
use rusqlite::Connection;

fn run_invariant_case(sql: &str, version: i64) -> ErrorCode {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection.execute_batch(sql).unwrap();
    let transaction = connection.transaction().unwrap();
    run_invariants(&transaction, version).unwrap_err().code()
}

fn strict_v1_schema(accounts_index: &str) -> String {
    format!(
        "CREATE TABLE schema_migrations(id INTEGER) STRICT;
         CREATE TABLE platform_meta(id INTEGER) STRICT;
         CREATE TABLE accounts(id INTEGER, name TEXT, deleted_at_ms INTEGER) STRICT;
         {accounts_index}"
    )
}

#[test]
fn migration_invariants_reject_missing_non_strict_and_invalid_indexes() {
    assert_eq!(run_invariant_case("", 1), ErrorCode::MigrationFailed);
    assert_eq!(
        run_invariant_case(
            "CREATE TABLE schema_migrations(id INTEGER);
             CREATE TABLE platform_meta(id INTEGER) STRICT;
             CREATE TABLE accounts(id INTEGER, name TEXT, deleted_at_ms INTEGER) STRICT;",
            1,
        ),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        run_invariant_case(&strict_v1_schema(""), 1),
        ErrorCode::MigrationFailed
    );
    assert_eq!(
        run_invariant_case(
            &strict_v1_schema("CREATE INDEX accounts_live_name ON accounts(name);"),
            1,
        ),
        ErrorCode::MigrationFailed
    );

    let mut version_two = strict_v1_schema(
        "CREATE UNIQUE INDEX accounts_live_name ON accounts(name) WHERE deleted_at_ms IS NULL;",
    );
    for table in [
        "workers",
        "worker_deployments",
        "deployment_vars",
        "deployment_secrets",
        "worker_routes",
        "control_idempotency",
        "deployment_referrers",
        "control_audit_events",
    ] {
        version_two.push_str(&format!("CREATE TABLE {table}(id INTEGER) STRICT;"));
    }
    assert_eq!(
        run_invariant_case(&version_two, 2),
        ErrorCode::MigrationFailed
    );
    version_two.push_str(
        "CREATE INDEX workers_live_name ON workers(id);
         CREATE UNIQUE INDEX live_exact_routes ON worker_routes(id);
         CREATE UNIQUE INDEX live_platform_routes ON worker_routes(id);",
    );
    assert_eq!(
        run_invariant_case(&version_two, 2),
        ErrorCode::MigrationFailed
    );
}

#[test]
fn applying_invalid_sql_is_transactional_and_typed() {
    let tmp = tempfile::tempdir().unwrap();
    let db = ControlDb::open(&tmp.path().join("control.sqlite"), 100).unwrap();
    let clock = open_compute_core::DeterministicClock::new(UNIX_EPOCH);
    let error = apply_one(
        &db,
        &clock,
        1,
        "invalid",
        "THIS IS NOT SQL",
        &MIGRATION_001_SHA256,
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::MigrationFailed);
    assert_eq!(db.user_version().unwrap(), 0);
}
