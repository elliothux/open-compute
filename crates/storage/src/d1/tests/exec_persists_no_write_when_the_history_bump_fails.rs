use super::*;

#[test]
fn exec_persists_no_write_when_the_history_bump_fails() {
    let fixture = fixture();
    let connection = fixture.engine.open().unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_session_bump
             BEFORE UPDATE ON __open_compute_meta
             WHEN old.key = 'session_version'
             BEGIN
               SELECT RAISE(ABORT, 'injected bump failure');
             END;",
        )
        .unwrap();
    drop(connection);

    assert!(
        fixture
            .engine
            .exec("CREATE TABLE must_not_exist(value TEXT)", limits())
            .is_err()
    );
    let connection = fixture.engine.open().unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'must_not_exist'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
