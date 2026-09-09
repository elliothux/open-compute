use super::*;

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
