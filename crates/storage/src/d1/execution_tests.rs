use super::*;
use open_compute_core::{AccountId, D1Config, ResourceId};

fn engine() -> (tempfile::TempDir, D1Engine) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("data.sqlite");
    let engine = D1Engine::create(
        &path,
        AccountId::generate(),
        ResourceId::generate(),
        10,
        256 * 1024 * 1024,
    )
    .unwrap();
    (temp, engine)
}

fn limits() -> D1QueryLimits {
    D1QueryLimits::query(&D1Config::default()).unwrap()
}

fn statement(sql: impl Into<String>, params: Vec<D1Value>) -> D1Statement {
    D1Statement {
        sql: sql.into(),
        params,
    }
}

#[test]
fn readonly_preflight_query_and_batch_reject_every_invalid_shape() {
    let (_temp, engine) = engine();
    assert_eq!(
        engine
            .statements_readonly(&[], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1InvalidBatch
    );
    let oversized = vec![statement("SELECT 1", vec![]); D1_MAX_BATCH_STATEMENTS + 1];
    assert_eq!(
        engine
            .statements_readonly(&oversized, limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1InvalidBatch
    );
    assert_eq!(
        engine
            .statements_readonly(&[statement("SELECT ?1", vec![])], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1ParameterMismatch
    );
    assert_eq!(
        engine
            .statements_readonly(&[statement("not valid SQL", vec![])], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    assert!(
        engine
            .statements_readonly(&[statement("SELECT 1", vec![])], limits())
            .unwrap()
    );
    assert!(
        !engine
            .statements_readonly(
                &[statement("CREATE TABLE shaped(id INTEGER)", vec![])],
                limits()
            )
            .unwrap()
    );

    for invalid in [
        statement("", vec![]),
        statement("SELECT 1; SELECT 2", vec![]),
        statement("SELECT ?1", vec![]),
        statement("SELECT ?1", vec![D1Value::Real(f64::INFINITY)]),
        statement(
            "SELECT ?1",
            vec![D1Value::Blob(vec![0; D1_MAX_VALUE_OR_ROW_BYTES + 1])],
        ),
    ] {
        assert!(engine.query(&invalid, limits()).is_err());
    }
    assert_eq!(
        engine.batch(&[], limits()).unwrap_err().code(),
        ErrorCode::D1InvalidBatch
    );
    assert_eq!(
        engine.batch(&oversized, limits()).unwrap_err().code(),
        ErrorCode::D1InvalidBatch
    );
    assert_eq!(
        engine
            .batch(&[statement("", vec![])], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1InvalidBatch
    );
    assert_eq!(
        engine
            .batch(&[statement("SELECT ?1", vec![])], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1ParameterMismatch
    );
    assert_eq!(
        engine
            .batch(&[statement("not valid SQL", vec![])], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    let readonly = engine
        .batch(
            &[statement("SELECT 1", vec![]), statement("SELECT 2", vec![])],
            limits(),
        )
        .unwrap();
    assert_eq!(readonly.len(), 2);
}

#[test]
fn query_batch_splits_query_objects_and_rolls_back_the_whole_sequence() {
    let (_temp, engine) = engine();
    let results = engine
        .query_batch(
            &[
                statement(
                    "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT NOT NULL);\
                     INSERT INTO items VALUES (1, ?1);\
                     INSERT INTO items VALUES (2, ?1);",
                    vec![D1Value::Text("alpha".into()), D1Value::Text("beta".into())],
                ),
                statement("SELECT value FROM items ORDER BY id", vec![]),
            ],
            limits(),
        )
        .unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(
        results[3].rows,
        vec![
            vec![D1Value::Text("alpha".into())],
            vec![D1Value::Text("beta".into())]
        ]
    );

    assert_eq!(
        engine
            .query_batch(
                &[statement(
                    "INSERT INTO items VALUES (3, 'rolled back');\
                     INSERT INTO items VALUES (1, 'duplicate');",
                    vec![],
                )],
                limits(),
            )
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    let count = engine
        .query(&statement("SELECT count(*) FROM items", vec![]), limits())
        .unwrap();
    assert_eq!(count.rows, vec![vec![D1Value::Integer(2)]]);
}

#[test]
fn exec_and_migration_validation_fail_closed_before_mutation() {
    let (_temp, engine) = engine();
    for sql in ["", " -- comment only", "SELECT ?1"] {
        assert!(engine.exec(sql, limits()).is_err());
    }
    let too_long = "x".repeat(D1_MAX_SQL_BYTES + 1);
    assert_eq!(
        engine.exec(&too_long, limits()).unwrap_err().code(),
        ErrorCode::D1SqlInvalid
    );
    assert_eq!(
        engine
            .apply_migrations(&[], limits(), 20)
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    let sql = "CREATE TABLE migrated(id INTEGER)";
    let migration = |id, name: &str, sql: &str| D1Migration {
        id,
        name: name.to_owned(),
        sha256: Sha256::digest(sql.as_bytes()).into(),
        sql: sql.to_owned(),
    };
    for migrations in [
        vec![migration(0, "zero.sql", sql)],
        vec![migration(1, "", sql)],
        vec![
            migration(1, "same.sql", sql),
            migration(1, "other.sql", sql),
        ],
        vec![migration(1, "same.sql", sql), migration(2, "same.sql", sql)],
        vec![D1Migration {
            sha256: [0; 32],
            ..migration(1, "hash.sql", sql)
        }],
    ] {
        assert_eq!(
            engine
                .apply_migrations(&migrations, limits(), 20)
                .unwrap_err()
                .code(),
            ErrorCode::D1MigrationDrift
        );
    }
    let long_name = "x".repeat(256);
    assert_eq!(
        engine
            .apply_migrations(&[migration(1, &long_name, sql)], limits(), 20)
            .unwrap_err()
            .code(),
        ErrorCode::D1MigrationDrift
    );
}
