use super::engine::{map_internal_error, map_open_error};
use super::*;
use crate::crypto::SecretCrypto;
use crate::master_key;
use open_compute_core::{AccountId, D1Config, ErrorCode, ResourceId, SecretBytes};
use sha2::{Digest, Sha256};

struct Fixture {
    _temp: tempfile::TempDir,
    engine: D1Engine,
    account: AccountId,
    resource: ResourceId,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let account = AccountId::generate();
    let resource = ResourceId::generate();
    let engine = D1Engine::create(
        &temp.path().join("data.sqlite"),
        account,
        resource,
        1_700_000_000_000,
        64 * 1024 * 1024,
    )
    .unwrap();
    Fixture {
        _temp: temp,
        engine,
        account,
        resource,
    }
}

fn limits() -> D1QueryLimits {
    D1QueryLimits::query(&D1Config::default()).unwrap()
}

fn statement(sql: &str, params: Vec<D1Value>) -> D1Statement {
    D1Statement {
        sql: sql.to_owned(),
        params,
    }
}

#[test]
fn create_crud_types_and_raw_column_order() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, enabled INTEGER, data BLOB)",
            limits(),
        )
        .unwrap();
    let insert = fixture
        .engine
        .query(
            &statement(
                "INSERT INTO users(name, enabled, data) VALUES (?1, ?2, ?3) RETURNING id",
                vec![
                    D1Value::Text("Ada".to_owned()),
                    D1Value::Integer(1),
                    D1Value::Blob(vec![0, 1, 255]),
                ],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(insert.rows, vec![vec![D1Value::Integer(1)]]);
    assert!(insert.meta.changed_db);
    assert_eq!(insert.meta.changes, 1);
    let selected = fixture
        .engine
        .query(
            &statement(
                "SELECT name AS duplicate, enabled AS duplicate, data, NULL, 1.5 FROM users",
                vec![],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(selected.columns[0..2], ["duplicate", "duplicate"]);
    assert_eq!(selected.rows[0][2], D1Value::Blob(vec![0, 1, 255]));
    assert_eq!(selected.rows[0][3], D1Value::Null);
    assert_eq!(selected.rows[0][4], D1Value::Real(1.5));
    assert_eq!(selected.meta.rows_read, 1);
    assert_eq!(selected.meta.changes, 0);
    assert_eq!(selected.meta.rows_written, 0);
    assert!(!selected.meta.changed_db);
}

#[test]
fn parameter_and_statement_limits_fail_closed() {
    let fixture = fixture();
    let mismatch = fixture
        .engine
        .query(&statement("SELECT ?1", vec![]), limits())
        .unwrap_err();
    assert_eq!(mismatch.code(), ErrorCode::D1ParameterMismatch);
    let multiple = fixture
        .engine
        .query(&statement("SELECT 1; SELECT 2", vec![]), limits())
        .unwrap_err();
    assert_eq!(multiple.code(), ErrorCode::D1SqlInvalid);
    let huge = "x".repeat(D1_MAX_SQL_BYTES + 1);
    assert_eq!(
        fixture
            .engine
            .query(&statement(&huge, vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid,
    );
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT ?1", vec![D1Value::Real(f64::NAN)]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError,
    );
}

#[test]
fn sqlite_and_materialization_limits_enforce_exact_boundaries() {
    let fixture = fixture();
    let mut exact_sql = "SELECT 1".to_owned();
    exact_sql.push_str(&" ".repeat(D1_MAX_SQL_BYTES - exact_sql.len()));
    fixture
        .engine
        .query(&statement(&exact_sql, vec![]), limits())
        .unwrap();

    let parameter_sql = format!(
        "SELECT {}",
        (1..=D1_MAX_BOUND_PARAMS)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    fixture
        .engine
        .query(
            &statement(
                &parameter_sql,
                vec![D1Value::Integer(1); D1_MAX_BOUND_PARAMS],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT 1", vec![D1Value::Null; D1_MAX_BOUND_PARAMS + 1]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );

    let too_many_columns = format!("SELECT {}", vec!["1"; D1_MAX_COLUMNS + 1].join(","));
    assert_eq!(
        fixture
            .engine
            .query(&statement(&too_many_columns, vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    fixture
        .engine
        .query(
            &statement("SELECT 'abc' LIKE ?1", vec![D1Value::Text("x".repeat(50))]),
            limits(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT 'abc' LIKE ?1", vec![D1Value::Text("x".repeat(51))]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    let function_args = |count: usize| format!("SELECT printf({})", vec!["'x'"; count].join(","));
    fixture
        .engine
        .query(&statement(&function_args(32), vec![]), limits())
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(&statement(&function_args(33), vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );

    let one_row = D1QueryLimits {
        max_result_rows: 1,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&statement("SELECT 1 UNION ALL SELECT 2", vec![]), one_row,)
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    let one_byte = D1QueryLimits {
        max_result_bytes: 1,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&statement("SELECT 'ab'", vec![]), one_byte)
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );

    fixture
        .engine
        .exec("CREATE TABLE bounded_write(value TEXT)", limits())
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement(
                    "INSERT INTO bounded_write VALUES ('x') RETURNING 'ab'",
                    vec![]
                ),
                one_byte,
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("SELECT count(*) FROM bounded_write", vec![]),
                limits(),
            )
            .unwrap()
            .rows,
        vec![vec![D1Value::Integer(0)]]
    );
}

#[test]
fn progress_handler_distinguishes_vm_budget_and_wall_timeout() {
    let fixture = fixture();
    let expensive = statement(
        "WITH RECURSIVE count(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM count WHERE x < 100000) SELECT sum(x) FROM count",
        vec![],
    );
    let vm_limited = D1QueryLimits {
        max_vm_steps: 1_000,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&expensive, vm_limited)
            .unwrap_err()
            .code(),
        ErrorCode::D1LimitError
    );
    let timed_out = D1QueryLimits {
        timeout: std::time::Duration::ZERO,
        ..limits()
    };
    assert_eq!(
        fixture
            .engine
            .query(&expensive, timed_out)
            .unwrap_err()
            .code(),
        ErrorCode::D1Timeout
    );
    assert_eq!(
        fixture
            .engine
            .batch(&[expensive], timed_out)
            .unwrap_err()
            .code(),
        ErrorCode::D1Timeout
    );
}

#[test]
fn frozen_database_quota_returns_database_full() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE quota_probe(id INTEGER PRIMARY KEY, payload BLOB)",
            limits(),
        )
        .unwrap();
    let mut successful = 0_u32;
    let terminal = loop {
        match fixture.engine.query(
            &statement(
                "INSERT INTO quota_probe(payload) VALUES (zeroblob(1000000))",
                vec![],
            ),
            limits(),
        ) {
            Ok(_) => successful += 1,
            Err(error) => break error,
        }
        assert!(successful < 100, "quota did not stop bounded growth");
    };
    assert_eq!(terminal.code(), ErrorCode::D1DatabaseFull);
    assert!(successful >= 50);
}

#[test]
fn session_version_is_monotonic_across_writes_reads_and_restore() {
    let fixture = fixture();
    assert_eq!(fixture.engine.session_version().unwrap(), 0);
    fixture
        .engine
        .query(&statement("SELECT 1", vec![]), limits())
        .unwrap();
    assert_eq!(fixture.engine.session_version().unwrap(), 0);
    fixture
        .engine
        .exec(
            "CREATE TABLE versions(id INTEGER PRIMARY KEY, value TEXT)",
            limits(),
        )
        .unwrap();
    let after_create = fixture.engine.session_version().unwrap();
    assert!(after_create >= 1);
    fixture
        .engine
        .query(
            &statement("INSERT INTO versions(value) VALUES ('one')", vec![]),
            limits(),
        )
        .unwrap();
    let after_insert = fixture.engine.session_version().unwrap();
    assert!(after_insert > after_create);
    fixture
        .engine
        .query(&statement("SELECT value FROM versions", vec![]), limits())
        .unwrap();
    assert_eq!(fixture.engine.session_version().unwrap(), after_insert);
    let snapshot = fixture._temp.path().join("snapshot.sqlite");
    fixture.engine.online_backup(&snapshot).unwrap();
    let restored = D1Engine::restore_as_new(
        &snapshot,
        &fixture._temp.path().join("restored.sqlite"),
        AccountId::generate(),
        ResourceId::generate(),
        30,
        256 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(restored.session_version().unwrap(), after_insert);
    restored
        .query(
            &statement("INSERT INTO versions(value) VALUES ('two')", vec![]),
            limits(),
        )
        .unwrap();
    assert!(restored.session_version().unwrap() > after_insert);
    let key = SecretBytes::new(vec![5_u8; 32]);
    let crypto = SecretCrypto::new(&key, &master_key::fingerprint_for_test(key.expose())).unwrap();
    let current = fixture.engine.session_version().unwrap();
    let future = crypto
        .seal_d1_bookmark(fixture.account, fixture.resource, current + 1)
        .unwrap();
    assert_eq!(
        crypto
            .open_d1_bookmark(fixture.account, fixture.resource, &future)
            .unwrap(),
        current + 1
    );
    assert!(
        crypto
            .open_d1_bookmark(fixture.account, fixture.resource, &future)
            .unwrap()
            > fixture.engine.session_version().unwrap()
    );
}

#[test]
fn batch_is_atomic_and_ordered() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT UNIQUE)",
            limits(),
        )
        .unwrap();
    let results = fixture
        .engine
        .batch(
            &[
                statement(
                    "INSERT INTO items(value) VALUES (?1)",
                    vec![D1Value::Text("one".to_owned())],
                ),
                statement(
                    "INSERT INTO items(value) VALUES (?1) RETURNING id",
                    vec![D1Value::Text("two".to_owned())],
                ),
                statement("SELECT value FROM items ORDER BY id", vec![]),
            ],
            limits(),
        )
        .unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[1].rows, vec![vec![D1Value::Integer(2)]]);
    assert_eq!(results[2].rows.len(), 2);
    assert_eq!(results[2].meta.changes, 0);
    assert_eq!(results[2].meta.rows_written, 0);
    let error = fixture
        .engine
        .batch(
            &[
                statement("INSERT INTO items(value) VALUES ('three')", vec![]),
                statement("INSERT INTO items(value) VALUES ('one')", vec![]),
            ],
            limits(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::D1SqlInvalid);
    let count = fixture
        .engine
        .query(
            &statement("SELECT count(*) FROM items WHERE value = 'three'", vec![]),
            limits(),
        )
        .unwrap();
    assert_eq!(count.rows, vec![vec![D1Value::Integer(0)]]);
}

#[test]
fn exec_uses_sqlite_tail_parser_and_versions_a_committed_prefix() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE events(value TEXT);\n
         CREATE TRIGGER mirror AFTER INSERT ON events WHEN new.value = 'source' BEGIN\n
           INSERT INTO events(value) VALUES ('trigger;body');\n
         END;\n
         INSERT INTO events(value) VALUES ('source');",
            limits(),
        )
        .unwrap();
    let rows = fixture
        .engine
        .query(
            &statement("SELECT value FROM events ORDER BY rowid", vec![]),
            limits(),
        )
        .unwrap();
    assert_eq!(rows.rows.len(), 2);
    let error = fixture.engine.exec(
        "INSERT INTO events(value) VALUES ('prefix'); SELECT * FROM missing_table; INSERT INTO events VALUES ('never')",
        limits(),
    ).unwrap_err();
    assert_eq!(error.code(), ErrorCode::D1SqlInvalid);
    let prefix = fixture
        .engine
        .query(
            &statement("SELECT count(*) FROM events WHERE value = 'prefix'", vec![]),
            limits(),
        )
        .unwrap();
    assert_eq!(prefix.rows, vec![vec![D1Value::Integer(1)]]);
}

#[test]
fn authorizer_blocks_cross_database_internal_and_connection_state_sql() {
    let fixture = fixture();
    for sql in [
        "ATTACH DATABASE ':memory:' AS other",
        "DETACH DATABASE other",
        "PRAGMA journal_mode = DELETE",
        "PRAGMA writable_schema = ON",
        "PRAGMA application_id = 7",
        "SELECT * FROM __open_compute_meta",
        "DROP TABLE __open_compute_migrations",
        "BEGIN",
        "SAVEPOINT tenant",
        "CREATE TEMP TABLE hidden(value)",
        "CREATE VIRTUAL TABLE spatial USING rtree(id, min_x, max_x)",
        "SELECT load_extension('x')",
        "VACUUM INTO 'copy.sqlite'",
    ] {
        assert_eq!(
            fixture.engine.exec(sql, limits()).unwrap_err().code(),
            ErrorCode::D1AuthorizerDenied,
            "{sql}",
        );
    }
    let harmless = fixture
        .engine
        .query(
            &statement("SELECT 'ATTACH; PRAGMA writable_schema'", vec![]),
            limits(),
        )
        .unwrap();
    assert_eq!(harmless.rows.len(), 1);
    fixture
        .engine
        .exec(
            "CREATE TABLE parent(id INTEGER PRIMARY KEY);\n
         CREATE TABLE child(parent_id INTEGER REFERENCES parent(id));\n
         CREATE INDEX child_parent ON child(parent_id);\n
         CREATE VIEW child_view AS SELECT parent_id FROM child;\n
         CREATE VIRTUAL TABLE searchable USING fts5(body)",
            limits(),
        )
        .unwrap();
    fixture
        .engine
        .query(
            &statement("SELECT json_extract('{\"value\":7}', '$.value')", vec![]),
            limits(),
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .query(
                &statement("INSERT INTO child(parent_id) VALUES (999)", vec![]),
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    fixture
        .engine
        .query(&statement("PRAGMA table_info(child)", vec![]), limits())
        .unwrap();
}

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

#[test]
fn online_backup_and_restore_rewrite_identity_but_keep_tenant_data() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE data(value TEXT); INSERT INTO data VALUES ('kept')",
            limits(),
        )
        .unwrap();
    let backup = fixture._temp.path().join("backup.sqlite");
    fixture.engine.online_backup(&backup).unwrap();
    let new_account = AccountId::generate();
    let new_resource = ResourceId::generate();
    let restored_path = fixture._temp.path().join("restored.sqlite");
    let restored = D1Engine::restore_as_new(
        &backup,
        &restored_path,
        new_account,
        new_resource,
        200,
        64 * 1024 * 1024,
    )
    .unwrap();
    restored.verify_identity().unwrap();
    let rows = restored
        .query(&statement("SELECT value FROM data", vec![]), limits())
        .unwrap();
    assert_eq!(rows.rows, vec![vec![D1Value::Text("kept".to_owned())]]);
    assert_ne!(fixture.account, new_account);
    assert_ne!(fixture.resource, new_resource);
}

#[test]
fn separate_database_files_are_isolated() {
    let first = fixture();
    let second = fixture();
    first
        .engine
        .exec("CREATE TABLE only_first(value)", limits())
        .unwrap();
    assert_eq!(
        second
            .engine
            .query(&statement("SELECT * FROM only_first", vec![]), limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid,
    );
}

#[test]
fn wal_recovery_keeps_committed_and_discards_uncommitted_transaction() {
    let fixture = fixture();
    fixture
        .engine
        .exec("CREATE TABLE recovery(value TEXT)", limits())
        .unwrap();
    {
        let connection = fixture.engine.open().unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        connection
            .execute("INSERT INTO recovery(value) VALUES ('uncommitted')", [])
            .unwrap();
    }
    fixture
        .engine
        .exec("INSERT INTO recovery(value) VALUES ('committed')", limits())
        .unwrap();
    drop(fixture.engine.open().unwrap());
    let result = fixture
        .engine
        .query(&statement("SELECT value FROM recovery", vec![]), limits())
        .unwrap();
    assert_eq!(
        result.rows,
        vec![vec![D1Value::Text("committed".to_owned())]]
    );
}

#[test]
fn corrupt_database_is_local_and_does_not_block_another_file() {
    let corrupt = fixture();
    let healthy = fixture();
    healthy
        .engine
        .exec("CREATE TABLE healthy(value INTEGER)", limits())
        .unwrap();
    drop(corrupt.engine.open().unwrap());
    std::fs::write(&corrupt.engine.path, b"not a sqlite database").unwrap();
    assert_eq!(
        corrupt.engine.quick_check().unwrap_err().code(),
        ErrorCode::D1DatabaseCorrupt,
    );
    healthy
        .engine
        .query(&statement("SELECT count(*) FROM healthy", vec![]), limits())
        .unwrap();
}

#[test]
fn wal_observation_accepts_owned_sidecar_and_rejects_symlink() {
    let fixture = fixture();
    fixture
        .engine
        .exec("CREATE TABLE wal_probe(value INTEGER)", limits())
        .unwrap();
    assert!(fixture.engine.wal_bytes().unwrap() <= fixture.engine.quota_bytes);

    fixture.engine.checkpoint(true).unwrap();
    let mut wal_name = fixture.engine.path.as_os_str().to_os_string();
    wal_name.push("-wal");
    let wal = std::path::PathBuf::from(wal_name);
    if wal.exists() {
        std::fs::remove_file(&wal).unwrap();
    }
    std::os::unix::fs::symlink(&fixture.engine.path, &wal).unwrap();
    assert_eq!(
        fixture.engine.wal_bytes().unwrap_err().code(),
        ErrorCode::D1IdentityMismatch
    );
}

#[test]
fn engine_rejects_invalid_creation_and_maps_sqlite_failures_stably() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        D1Engine::create(
            &temp.path().join("too-small.sqlite"),
            AccountId::generate(),
            ResourceId::generate(),
            0,
            1024,
        )
        .unwrap_err()
        .code(),
        ErrorCode::D1IdentityMismatch
    );

    for (sqlite_code, expected) in [
        (rusqlite::ffi::SQLITE_CORRUPT, ErrorCode::D1DatabaseCorrupt),
        (rusqlite::ffi::SQLITE_NOTADB, ErrorCode::D1DatabaseCorrupt),
        (rusqlite::ffi::SQLITE_FULL, ErrorCode::D1DatabaseFull),
        (rusqlite::ffi::SQLITE_BUSY, ErrorCode::D1Overloaded),
        (rusqlite::ffi::SQLITE_LOCKED, ErrorCode::D1Overloaded),
        (
            rusqlite::ffi::SQLITE_CANTOPEN,
            ErrorCode::ResourceUnavailable,
        ),
    ] {
        let error = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(sqlite_code), None);
        assert_eq!(map_open_error(&error).code(), expected);
    }
    assert_eq!(
        map_internal_error(rusqlite::Error::InvalidQuery).code(),
        ErrorCode::D1DatabaseCorrupt
    );
}
