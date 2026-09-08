use super::*;

#[test]
fn exec_commit_failure_keeps_data_rolled_back_and_head_reconcilable() {
    let fixture = fixture();
    fixture
        .engine
        .exec(
            "CREATE TABLE parent(id INTEGER PRIMARY KEY);
             CREATE TABLE child(
               parent_id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED
             )",
            limits(),
        )
        .unwrap();
    let before = fixture.engine.session_version().unwrap();

    let error = fixture
        .engine
        .exec("INSERT INTO child(parent_id) VALUES (1)", limits())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::D1SqlInvalid);
    assert_eq!(fixture.engine.session_version().unwrap(), before + 1);
    let rows = fixture
        .engine
        .query(&statement("SELECT count(*) FROM child", vec![]), limits())
        .unwrap();
    assert_eq!(rows.rows, vec![vec![D1Value::Integer(0)]]);
}
