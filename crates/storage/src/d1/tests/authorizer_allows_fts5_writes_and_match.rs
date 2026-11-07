use super::*;

#[test]
fn authorizer_allows_fts5_writes_and_match() {
    let fixture = fixture();
    fixture
        .engine
        .exec("CREATE VIRTUAL TABLE notes USING fts5(body)", limits())
        .unwrap();
    fixture
        .engine
        .query(
            &statement(
                "INSERT INTO notes(body) VALUES (?1), (?2)",
                vec![
                    D1Value::Text("hello world".to_owned()),
                    D1Value::Text("goodbye".to_owned()),
                ],
            ),
            limits(),
        )
        .unwrap();
    let matched = fixture
        .engine
        .query(
            &statement("SELECT body FROM notes WHERE notes MATCH 'hello'", vec![]),
            limits(),
        )
        .unwrap();
    assert_eq!(
        matched.rows,
        vec![vec![D1Value::Text("hello world".to_owned())]]
    );
    fixture
        .engine
        .query(
            &statement("DELETE FROM notes WHERE notes MATCH 'goodbye'", vec![]),
            limits(),
        )
        .unwrap();
    let remaining = fixture
        .engine
        .query(&statement("SELECT count(*) FROM notes", vec![]), limits())
        .unwrap();
    assert_eq!(remaining.rows, vec![vec![D1Value::Integer(1)]]);
    for sql in ["PRAGMA data_version", "PRAGMA main.data_version"] {
        fixture
            .engine
            .query(&statement(sql, vec![]), limits())
            .unwrap();
    }
    assert_eq!(
        fixture
            .engine
            .exec("PRAGMA data_version = 7", limits())
            .unwrap_err()
            .code(),
        ErrorCode::D1AuthorizerDenied
    );
}
