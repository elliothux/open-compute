use super::*;

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
