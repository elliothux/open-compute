use super::*;

#[test]
fn alter_table_rebuild_preserves_rows_and_protected_schema() {
    let fixture = fixture();
    let query = |sql| fixture.engine.query(&statement(sql, vec![]), limits());
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT, unused TEXT)").unwrap();
    query("INSERT INTO items VALUES (1, 'kept', 'discarded')").unwrap();
    query("ALTER TABLE items RENAME COLUMN value TO label").unwrap();
    query("ALTER TABLE items DROP COLUMN unused").unwrap();

    let rebuild = "CREATE TABLE replacement(id INTEGER PRIMARY KEY, label TEXT NOT NULL);
                   INSERT INTO replacement SELECT id, label FROM items;
                   DROP TABLE items;
                   ALTER TABLE replacement RENAME TO items";
    fixture
        .engine
        .query_batch(&[statement(rebuild, vec![])], limits())
        .unwrap();
    for sql in [
        "ALTER TABLE __open_compute_meta RENAME TO moved",
        "ALTER TABLE refinery_schema_history RENAME COLUMN name TO renamed",
        "CREATE TEMP TABLE scratch(id)",
    ] {
        assert_eq!(
            query(sql).unwrap_err().code(),
            ErrorCode::D1AuthorizerDenied,
            "{sql}"
        );
    }
    assert!(query("UPDATE temp.sqlite_schema SET sql = NULL").is_err());
    let denied = "ALTER TABLE items RENAME TO moved; SELECT sql FROM temp.sqlite_schema";
    assert_eq!(
        fixture
            .engine
            .query_batch(&[statement(denied, vec![])], limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1AuthorizerDenied,
    );
    assert!(query("ALTER TABLE items RENAME COLUMN missing TO renamed").is_err());
    query("ALTER TABLE items ADD COLUMN active INTEGER DEFAULT 1").unwrap();
    assert_eq!(
        query("SELECT * FROM items").unwrap().rows,
        vec![vec![
            D1Value::Integer(1),
            D1Value::Text("kept".into()),
            D1Value::Integer(1)
        ]]
    );
}

#[test]
fn deferred_foreign_keys_are_checked_at_commit() {
    let fixture = fixture();
    let run = |sql| {
        fixture
            .engine
            .query_batch(&[statement(sql, vec![])], limits())
    };
    assert_eq!(
        run("PRAGMA foreign_keys").unwrap()[0].rows,
        vec![vec![D1Value::Integer(1)]]
    );
    run("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(id INTEGER REFERENCES parent)").unwrap();
    run("PRAGMA defer_foreign_keys = ON; INSERT INTO child VALUES (1); INSERT INTO parent VALUES (1)").unwrap();
    assert_eq!(
        run("PRAGMA defer_foreign_keys = ON; INSERT INTO child VALUES (2)")
            .unwrap_err()
            .code(),
        ErrorCode::D1SqlInvalid
    );
    assert_eq!(
        run("SELECT id FROM child").unwrap()[0].rows,
        vec![vec![D1Value::Integer(1)]]
    );
    assert_eq!(
        run("PRAGMA foreign_keys = OFF").unwrap_err().code(),
        ErrorCode::D1AuthorizerDenied
    );
    assert_eq!(
        run("PRAGMA defer_foreign_keys = OFF").unwrap_err().code(),
        ErrorCode::D1AuthorizerDenied
    );
}
