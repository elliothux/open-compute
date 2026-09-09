use super::*;

#[test]
fn exec_uses_sqlite_tail_parser_and_versions_a_committed_prefix() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE events(value TEXT);\n
         CREATE TABLE prefix_log(value TEXT UNIQUE);\n
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
    let before = fixture.engine.session_version().unwrap();
    let error = fixture
        .engine
        .exec(
            "INSERT INTO prefix_log(value) VALUES ('prefix');
             INSERT INTO prefix_log(value) VALUES ('prefix');
             INSERT INTO prefix_log(value) VALUES ('never')",
            limits(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::D1SqlInvalid);
    assert_eq!(fixture.engine.session_version().unwrap(), before + 1);
    let prefix = fixture
        .engine
        .query(
            &statement(
                "SELECT count(*) FROM prefix_log WHERE value = 'prefix'",
                vec![],
            ),
            limits(),
        )
        .unwrap();
    assert_eq!(prefix.rows, vec![vec![D1Value::Integer(1)]]);
}
